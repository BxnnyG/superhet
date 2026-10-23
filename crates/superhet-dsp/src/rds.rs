//! RDS (IEC 62106) receiver: 57 kHz subcarrier baseband to decoded groups.
//!
//! Input is the complex baseband that [`crate::wfm::StereoMpx`] produces by mixing the FM
//! multiplex with the conjugate of the third harmonic of the recovered 19 kHz pilot, so the
//! RDS subcarrier lands at DC with a constant (unknown) phase. Signal path:
//!
//! 1. Low-pass + decimation, then resampling to exactly 19 kHz (16 samples per bit).
//! 2. Matched filter for the biphase (Manchester) symbol, evaluated at every sample; the bit
//!    clock phase is the one with the highest smoothed matched-filter energy.
//! 3. BPSK axis estimate (angle doubling, kept continuous), soft symbol, differential decoding
//!    (immune to the 180 degree ambiguity).
//! 4. Block synchronisation by syndrome: ten check bits, five offset words (A, B, C, C', D),
//!    sync after two valid blocks at the right distance, loss after too many bad blocks. Bursts
//!    of one or two bits are corrected in sync.
//! 5. Group decoding: PI (every group), PTY and TP (every group), groups 0A/0B (PS, TA, M/S) and
//!    2A/2B (RadioText). Content that was needed a correction is only taken over after it was
//!    received twice in a row.

use num_complex::Complex32;

use crate::fir::{DecimFir, design_lowpass};
use crate::resample::Resampler;

/// RDS bit rate in bit/s.
pub const BIT_RATE_HZ: f64 = 1187.5;
/// Samples per bit after resampling.
const SPB: usize = 16;
/// Baseband sample rate of the bit recovery in Hz.
const BASEBAND_RATE_HZ: f64 = BIT_RATE_HZ * SPB as f64;
/// Pass-band edge of the baseband low-pass in Hz.
const LPF_PASS_HZ: f64 = 2_700.0;
/// Stop-band edge of the baseband low-pass in Hz (the L-R sidebands start around 4 kHz).
const LPF_STOP_HZ: f64 = 5_500.0;
/// Stop-band attenuation of the baseband low-pass in dB.
const LPF_ATTEN_DB: f64 = 50.0;
/// Target rate of the low-pass output (before resampling to 19 kHz) in Hz.
const DECIM_TARGET_HZ: f64 = 38_000.0;
/// Smoothing of the per-phase matched-filter energy (per bit).
const ENERGY_ALPHA: f32 = 1.0 / 96.0;
/// A new bit-clock phase must beat the current one by this factor.
const PHASE_HYSTERESIS: f32 = 1.15;
/// Smoothing of the BPSK axis estimate (per bit).
const AXIS_ALPHA: f32 = 1.0 / 64.0;

/// Decoded data is dropped after this many bits (about 10 s) without block sync.
const STALE_BITS: u32 = 11_875;
/// Generator polynomial `x^10 + x^8 + x^7 + x^5 + x^4 + x^3 + 1`.
const GENERATOR: u32 = 0x5B9;
/// Offset words A, B, C, C', D.
pub(crate) const OFFSETS: [u16; 5] = [0x0FC, 0x198, 0x168, 0x350, 0x1B4];
/// Position in the group (0..=3) of each offset word.
const OFFSET_POS: [u8; 5] = [0, 1, 2, 2, 3];
const BLOCK_BITS: u64 = 26;
/// Sync search forgets a candidate block after this many blocks.
const SEARCH_BLOCKS: u64 = 6;
/// Blocks looked at for the loss-of-sync decision.
const LOSS_WINDOW: u32 = 40;
/// Sync is lost with more bad blocks than this in the window.
const LOSS_BAD_BLOCKS: u32 = 24;

/// Decoded RDS information of one station.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RdsInfo {
    /// Programme identification code.
    pub pi: Option<u16>,
    /// Programme type (0..=31).
    pub pty: Option<u8>,
    /// Traffic programme flag.
    pub tp: Option<bool>,
    /// Traffic announcement flag.
    pub ta: Option<bool>,
    /// Music (true) or speech (false) switch.
    pub music: Option<bool>,
    /// Programme service name (8 characters), once all of them were received.
    pub ps: Option<String>,
    /// RadioText (up to 64 characters).
    pub rt: Option<String>,
}

impl RdsInfo {
    /// True if nothing was decoded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Remainder of the 26-bit `word` divided by the generator polynomial.
fn remainder(word: u32) -> u16 {
    let mut v = word & 0x03FF_FFFF;
    for i in (10..26).rev() {
        if v >> i & 1 == 1 {
            v ^= GENERATOR << (i - 10);
        }
    }
    (v & 0x3FF) as u16
}

/// Builds a block: 16 information bits, ten check bits XOR the offset word.
#[cfg(any(test, feature = "testsig"))]
pub(crate) fn encode_block(info: u16, offset: u16) -> u32 {
    let m = u32::from(info) << 10;
    m | (u32::from(remainder(m)) ^ u32::from(offset))
}

/// A block with its information word delivered by the framer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockEvent {
    /// A valid block at position `pos` (0..=3); `c_prime` marks offset C' at position 2.
    Good {
        pos: u8,
        info: u16,
        corrected: bool,
        c_prime: bool,
    },
    /// A block that failed the check.
    Bad { pos: u8 },
    /// Synchronisation was lost.
    Lost,
}

/// Bit stream to blocks.
#[derive(Debug, Clone, Default)]
struct Framer {
    reg: u32,
    bits: u64,
    synced: bool,
    /// Search: end bit index and position of the last valid block.
    last_hit: Option<(u64, u8)>,
    /// Synced: bits until the end of the current block and its position.
    left: u64,
    pos: u8,
    /// Bit i set = block i blocks ago was bad.
    history: u64,
}

impl Framer {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Tries to turn a block that failed the check into one that passes it with a burst of one
    /// or two flipped bits.
    fn correct(reg: u32, offsets: &[u16]) -> Option<u32> {
        for &burst in &[0b1u32, 0b11] {
            let width = if burst == 1 { 26 } else { 25 };
            for shift in 0..width {
                let fixed = reg ^ (burst << shift);
                let syn = remainder(fixed);
                if offsets.contains(&syn) {
                    return Some(fixed);
                }
            }
        }
        None
    }

    fn push(&mut self, bit: bool) -> Option<BlockEvent> {
        self.reg = ((self.reg << 1) | u32::from(bit)) & 0x03FF_FFFF;
        self.bits += 1;
        if self.synced {
            self.push_synced()
        } else {
            self.push_search()
        }
    }

    fn push_search(&mut self) -> Option<BlockEvent> {
        if self.bits < BLOCK_BITS {
            return None;
        }
        let syn = remainder(self.reg);
        let kind = OFFSETS.iter().position(|&o| o == syn);
        let Some(kind) = kind else {
            if let Some((end, _)) = self.last_hit
                && self.bits - end > SEARCH_BLOCKS * BLOCK_BITS
            {
                self.last_hit = None;
            }
            return None;
        };
        let pos = OFFSET_POS[kind];
        let c_prime = kind == 3;
        if let Some((end, last_pos)) = self.last_hit {
            let dist = self.bits - end;
            if dist.is_multiple_of(BLOCK_BITS) && dist / BLOCK_BITS <= SEARCH_BLOCKS {
                let expect = (u64::from(last_pos) + dist / BLOCK_BITS) % 4;
                if u64::from(pos) == expect {
                    self.synced = true;
                    self.left = BLOCK_BITS;
                    self.pos = (pos + 1) % 4;
                    self.history = 0;
                    self.last_hit = None;
                    return Some(BlockEvent::Good {
                        pos,
                        info: (self.reg >> 10) as u16,
                        corrected: false,
                        c_prime,
                    });
                }
            }
        }
        self.last_hit = Some((self.bits, pos));
        None
    }

    fn push_synced(&mut self) -> Option<BlockEvent> {
        self.left -= 1;
        if self.left > 0 {
            return None;
        }
        self.left = BLOCK_BITS;
        let pos = self.pos;
        self.pos = (pos + 1) % 4;
        let wanted: &[u16] = match pos {
            0 => &OFFSETS[0..1],
            1 => &OFFSETS[1..2],
            2 => &OFFSETS[2..4],
            _ => &OFFSETS[4..5],
        };
        let syn = remainder(self.reg);
        let (word, corrected) = if wanted.contains(&syn) {
            (Some(self.reg), false)
        } else {
            (Self::correct(self.reg, wanted), true)
        };
        let window = (1u64 << LOSS_WINDOW) - 1;
        self.history = (self.history << 1) & window;
        let Some(word) = word else {
            self.history |= 1;
            if self.history.count_ones() > LOSS_BAD_BLOCKS {
                self.synced = false;
                self.last_hit = None;
                self.history = 0;
                return Some(BlockEvent::Lost);
            }
            return Some(BlockEvent::Bad { pos });
        };
        let c_prime = pos == 2 && remainder(word) == OFFSETS[3];
        Some(BlockEvent::Good {
            pos,
            info: (word >> 10) as u16,
            corrected,
            c_prime,
        })
    }
}

/// Maps an RDS character code to Unicode (IEC 62106 annex E, basic character set).
#[must_use]
pub fn rds_char(code: u8) -> char {
    const TABLE_80: [char; 128] = [
        'á', 'à', 'é', 'è', 'í', 'ì', 'ó', 'ò', 'ú', 'ù', 'Ñ', 'Ç', 'Ş', 'ß', '¡', 'Ĳ', //
        'â', 'ä', 'ê', 'ë', 'î', 'ï', 'ô', 'ö', 'û', 'ü', 'ñ', 'ç', 'ş', 'ǧ', 'ı', 'ĳ', //
        'ª', 'α', '©', '‰', 'Ǧ', 'ě', 'ň', 'ő', 'π', '€', '£', '$', '←', '↑', '→', '↓', //
        'º', '¹', '²', '³', '±', 'İ', 'ń', 'ű', 'µ', '¿', '÷', '°', '¼', '½', '¾', '§', //
        'Á', 'À', 'É', 'È', 'Í', 'Ì', 'Ó', 'Ò', 'Ú', 'Ù', 'Ř', 'Č', 'Š', 'Ž', 'Ð', 'Ŀ', //
        'Â', 'Ä', 'Ê', 'Ë', 'Î', 'Ï', 'Ô', 'Ö', 'Û', 'Ü', 'ř', 'č', 'š', 'ž', 'đ', 'ŀ', //
        'Ã', 'Å', 'Æ', 'Œ', 'ŷ', 'Ý', 'Õ', 'Ø', 'Þ', 'Ŋ', 'Ŕ', 'Ć', 'Ś', 'Ź', 'Ŧ', 'ð', //
        'ã', 'å', 'æ', 'œ', 'ŵ', 'ý', 'õ', 'ø', 'þ', 'ŋ', 'ŕ', 'ć', 'ś', 'ź', 'ŧ', ' ',
    ];
    match code {
        0x24 => '¤',
        0x5E => '―',
        0x60 => '║',
        0x7E => '¯',
        0x20..=0x7E => char::from(code),
        0x80..=0xFF => TABLE_80[usize::from(code - 0x80)],
        _ => ' ',
    }
}

const PS_LEN: usize = 8;
const RT_LEN: usize = 64;
pub(crate) const RT_END: u8 = 0x0D;

/// Collects blocks into groups and groups into [`RdsInfo`].
#[derive(Debug, Clone)]
struct GroupDecoder {
    blocks: [Option<(u16, bool)>; 4],
    c_prime: bool,
    info: RdsInfo,
    revision: u64,
    pi_pending: Option<u16>,
    pty_pending: Option<u8>,
    ps: [Option<u8>; PS_LEN],
    ps_pending: [Option<[u8; 2]>; PS_LEN / 2],
    rt: [Option<u8>; RT_LEN],
    rt_pending: [Option<[u8; 4]>; RT_LEN / 4],
    rt_ab: Option<bool>,
}

impl GroupDecoder {
    fn new() -> Self {
        Self {
            blocks: [None; 4],
            c_prime: false,
            info: RdsInfo::default(),
            revision: 0,
            pi_pending: None,
            pty_pending: None,
            ps: [None; PS_LEN],
            ps_pending: [None; PS_LEN / 2],
            rt: [None; RT_LEN],
            rt_pending: [None; RT_LEN / 4],
            rt_ab: None,
        }
    }

    fn reset(&mut self) {
        let revision = self.revision;
        *self = Self::new();
        self.revision = revision + 1;
    }

    fn clear_station(&mut self) {
        let pi = self.info.pi;
        let revision = self.revision;
        *self = Self::new();
        self.info.pi = pi;
        self.revision = revision;
    }

    fn event(&mut self, event: BlockEvent) {
        match event {
            BlockEvent::Lost => self.blocks = [None; 4],
            BlockEvent::Bad { pos } => {
                if pos == 0 {
                    self.blocks = [None; 4];
                }
                self.blocks[usize::from(pos)] = None;
            }
            BlockEvent::Good {
                pos,
                info,
                corrected,
                c_prime,
            } => {
                if pos == 0 {
                    self.blocks = [None; 4];
                }
                self.blocks[usize::from(pos)] = Some((info, corrected));
                if pos == 2 {
                    self.c_prime = c_prime;
                }
                if pos == 0 {
                    self.accept_pi(info, corrected);
                }
                if pos == 3 {
                    self.finish_group();
                }
            }
        }
    }

    /// PI from block A (or the repeat in block C' of version B groups).
    fn accept_pi(&mut self, pi: u16, corrected: bool) {
        if self.info.pi == Some(pi) {
            self.pi_pending = None;
            return;
        }
        if corrected && self.pi_pending != Some(pi) {
            self.pi_pending = Some(pi);
            return;
        }
        // A different station: forget everything that belonged to the previous one.
        self.pi_pending = None;
        self.clear_station();
        self.info.pi = Some(pi);
        self.revision += 1;
    }

    fn finish_group(&mut self) {
        let [_, Some((b, cb)), c, Some((d, cd))] = self.blocks else {
            self.blocks = [None; 4];
            return;
        };
        let clean = !cb && !cd && c.is_none_or(|(_, corrected)| !corrected);
        self.blocks = [None; 4];

        let group = (b >> 12) as u8;
        let version_b = b & 0x0800 != 0;
        let before = self.info.clone();

        let pty = ((b >> 5) & 0x1F) as u8;
        if self.info.pty != Some(pty) {
            if clean || self.pty_pending == Some(pty) {
                self.info.pty = Some(pty);
                self.pty_pending = None;
            } else {
                self.pty_pending = Some(pty);
            }
        }
        if clean {
            self.info.tp = Some(b & 0x0400 != 0);
        }
        match group {
            0 => self.group_ps(b, d, clean),
            2 => self.group_rt(b, c.map(|(v, _)| v), d, version_b, clean),
            _ => {}
        }
        self.info.ps = self.ps_text();
        self.info.rt = self.rt_text();
        if self.info != before {
            self.revision += 1;
        }
    }

    fn group_ps(&mut self, b: u16, d: u16, clean: bool) {
        if clean {
            self.info.ta = Some(b & 0x0010 != 0);
            self.info.music = Some(b & 0x0008 != 0);
        }
        let seg = usize::from(b & 3);
        let chars = [(d >> 8) as u8, (d & 0xFF) as u8];
        if clean || self.ps_pending[seg] == Some(chars) {
            self.ps[seg * 2] = Some(chars[0]);
            self.ps[seg * 2 + 1] = Some(chars[1]);
            self.ps_pending[seg] = None;
        } else {
            self.ps_pending[seg] = Some(chars);
        }
    }

    fn group_rt(&mut self, b: u16, c: Option<u16>, d: u16, version_b: bool, clean: bool) {
        let ab = b & 0x0010 != 0;
        if self.rt_ab != Some(ab) {
            // The sender starts a new message: drop the old text.
            if self.rt_ab.is_some() {
                self.rt = [None; RT_LEN];
                self.rt_pending = [None; RT_LEN / 4];
            }
            self.rt_ab = Some(ab);
        }
        let seg = usize::from(b & 0x0F);
        if version_b {
            let chars = [(d >> 8) as u8, (d & 0xFF) as u8];
            // 2B (32 characters, two per group) is rare; only error-free groups are used.
            if clean {
                self.rt[seg * 2] = Some(chars[0]);
                self.rt[seg * 2 + 1] = Some(chars[1]);
            }
            return;
        }
        let Some(c) = c else { return };
        let chars = [
            (c >> 8) as u8,
            (c & 0xFF) as u8,
            (d >> 8) as u8,
            (d & 0xFF) as u8,
        ];
        if clean || self.rt_pending[seg] == Some(chars) {
            for (i, ch) in chars.iter().enumerate() {
                self.rt[seg * 4 + i] = Some(*ch);
            }
            self.rt_pending[seg] = None;
        } else {
            self.rt_pending[seg] = Some(chars);
        }
    }

    fn ps_text(&self) -> Option<String> {
        if self.ps.iter().any(Option::is_none) {
            return None;
        }
        let text: String = self.ps.iter().flatten().map(|&c| rds_char(c)).collect();
        Some(text)
    }

    fn rt_text(&self) -> Option<String> {
        let mut text = String::new();
        for slot in &self.rt {
            match slot {
                Some(RT_END) | None => break,
                Some(c) => text.push(rds_char(*c)),
            }
        }
        let trimmed = text.trim_end();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }
}

/// Streaming RDS decoder; see the module docs.
pub struct RdsDecoder {
    lpf: DecimFir<Complex32>,
    res_i: Resampler,
    res_q: Resampler,
    // Scratch buffers.
    dec: Vec<Complex32>,
    i_in: Vec<f32>,
    q_in: Vec<f32>,
    i_out: Vec<f32>,
    q_out: Vec<f32>,
    // Bit recovery.
    ring: [Complex32; SPB],
    n: usize,
    energy: [f32; SPB],
    best: usize,
    sym_count: u64,
    axis_acc: Complex32,
    axis: Option<Complex32>,
    prev_soft: f32,
    framer: Framer,
    groups: GroupDecoder,
    /// Bits since the block sync was last held.
    unsynced_bits: u32,
    /// Blocks seen in sync since the last reset (for diagnostics).
    good_blocks: u64,
    bad_blocks: u64,
}

impl RdsDecoder {
    /// Creates a decoder for baseband input at `rate_hz` (the FM channel rate).
    #[must_use]
    pub fn new(rate_hz: f64) -> Self {
        let d = ((rate_hz / DECIM_TARGET_HZ).floor() as usize).max(1);
        let rate_d = rate_hz / d as f64;
        let taps = design_lowpass(rate_hz, LPF_PASS_HZ, LPF_STOP_HZ, LPF_ATTEN_DB);
        Self {
            lpf: DecimFir::new(taps, d),
            res_i: Resampler::new(rate_d, BASEBAND_RATE_HZ),
            res_q: Resampler::new(rate_d, BASEBAND_RATE_HZ),
            dec: Vec::new(),
            i_in: Vec::new(),
            q_in: Vec::new(),
            i_out: Vec::new(),
            q_out: Vec::new(),
            ring: [Complex32::new(0.0, 0.0); SPB],
            n: 0,
            energy: [0.0; SPB],
            best: 0,
            sym_count: 0,
            axis_acc: Complex32::new(0.0, 0.0),
            axis: None,
            prev_soft: 0.0,
            framer: Framer::default(),
            groups: GroupDecoder::new(),
            unsynced_bits: 0,
            good_blocks: 0,
            bad_blocks: 0,
        }
    }

    /// Forgets the filter state and everything decoded (call on retune).
    pub fn reset(&mut self) {
        self.lpf.reset();
        self.res_i.reset();
        self.res_q.reset();
        self.ring = [Complex32::new(0.0, 0.0); SPB];
        self.n = 0;
        self.energy = [0.0; SPB];
        self.best = 0;
        self.sym_count = 0;
        self.axis_acc = Complex32::new(0.0, 0.0);
        self.axis = None;
        self.prev_soft = 0.0;
        self.framer.reset();
        self.groups.reset();
        self.unsynced_bits = 0;
        self.good_blocks = 0;
        self.bad_blocks = 0;
    }

    /// What has been decoded so far.
    #[must_use]
    pub fn info(&self) -> &RdsInfo {
        &self.groups.info
    }

    /// Counter that increases whenever [`Self::info`] changes.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.groups.revision
    }

    /// True while the block synchronisation holds.
    #[must_use]
    pub fn synced(&self) -> bool {
        self.framer.synced
    }

    /// Valid and invalid blocks seen while in sync since the last reset.
    #[must_use]
    pub fn block_counts(&self) -> (u64, u64) {
        (self.good_blocks, self.bad_blocks)
    }

    /// Processes baseband samples (57 kHz subcarrier mixed to DC).
    pub fn process(&mut self, mixed: &[Complex32]) {
        self.dec.clear();
        self.lpf.process(mixed, &mut self.dec);
        self.i_in.clear();
        self.q_in.clear();
        self.i_in.extend(self.dec.iter().map(|z| z.re));
        self.q_in.extend(self.dec.iter().map(|z| z.im));
        self.i_out.clear();
        self.q_out.clear();
        self.res_i.process(&self.i_in, &mut self.i_out);
        self.res_q.process(&self.q_in, &mut self.q_out);
        let count = self.i_out.len().min(self.q_out.len());
        for k in 0..count {
            let z = Complex32::new(self.i_out[k], self.q_out[k]);
            self.sample(z);
        }
    }

    /// One baseband sample at 19 kHz.
    fn sample(&mut self, z: Complex32) {
        let slot = self.n % SPB;
        self.ring[slot] = z;
        // Biphase matched filter: first half of the bit minus the second half.
        let mut m = Complex32::new(0.0, 0.0);
        for k in 0..SPB {
            let v = self.ring[(slot + SPB - k) % SPB];
            if k < SPB / 2 {
                m += v;
            } else {
                m -= v;
            }
        }
        self.energy[slot] += ENERGY_ALPHA * (m.norm_sqr() - self.energy[slot]);
        if slot == SPB - 1 {
            self.pick_phase();
        }
        if slot == self.best {
            self.symbol(m);
        }
        self.n = self.n.wrapping_add(1);
    }

    fn pick_phase(&mut self) {
        let (top, top_e) =
            self.energy.iter().enumerate().fold(
                (0, 0.0f32),
                |acc, (i, &e)| if e > acc.1 { (i, e) } else { acc },
            );
        if top != self.best && top_e > self.energy[self.best] * PHASE_HYSTERESIS {
            self.best = top;
        }
    }

    /// One matched-filter output at the bit clock.
    fn symbol(&mut self, m: Complex32) {
        self.sym_count += 1;
        // BPSK axis by angle doubling, kept continuous so that an axis near +-90 degrees does
        // not flip the sign of the soft symbols.
        let sq = m * m;
        let norm = sq.norm();
        if norm > 0.0 {
            self.axis_acc +=
                (sq / norm - self.axis_acc) * AXIS_ALPHA.max(1.0 / self.sym_count as f32);
        }
        let dn = self.axis_acc.norm();
        if dn < 1e-6 {
            return;
        }
        // Principal square root of the unit vector `u`: `(u + 1) / |u + 1|`.
        let u = self.axis_acc / dn;
        let root = u + Complex32::new(1.0, 0.0);
        let mut axis = if root.norm() < 1e-6 {
            Complex32::new(0.0, 1.0)
        } else {
            root / root.norm()
        };
        if let Some(prev) = self.axis
            && (axis * prev.conj()).re < 0.0
        {
            axis = -axis;
        }
        self.axis = Some(axis);
        let soft = (m * axis.conj()).re;
        let bit = soft * self.prev_soft < 0.0;
        self.prev_soft = soft;
        self.push_bit(bit);
    }

    fn push_bit(&mut self, bit: bool) {
        if self.framer.synced {
            self.unsynced_bits = 0;
        } else {
            self.unsynced_bits = self.unsynced_bits.saturating_add(1);
            if self.unsynced_bits == STALE_BITS && !self.groups.info.is_empty() {
                // The station is gone (or the pilot): do not show its text forever.
                self.groups.reset();
            }
        }
        if let Some(event) = self.framer.push(bit) {
            match event {
                BlockEvent::Good { .. } => self.good_blocks += 1,
                BlockEvent::Bad { .. } => self.bad_blocks += 1,
                BlockEvent::Lost => {}
            }
            self.groups.event(event);
        }
    }

    /// Feeds already differentially decoded bits (test hook).
    #[cfg(test)]
    pub(crate) fn push_bits(&mut self, bits: &[bool]) {
        for &b in bits {
            self.push_bit(b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsig::*;

    #[test]
    fn offset_words_have_the_standard_syndromes() {
        // The syndrome register of common decoders holds the remainder of word * x^10; for the
        // offset words that gives the widely published values 383, 14, 303, 748 and 663
        // (A, B, C, C', D). This ties the polynomial and the offset words to the standard.
        let expect = [0x17F, 0x00E, 0x12F, 0x2EC, 0x297];
        for (offset, syn) in OFFSETS.iter().zip(expect) {
            assert_eq!(remainder(u32::from(*offset) << 10), syn, "{offset:#x}");
        }
    }

    #[test]
    fn encoded_blocks_pass_the_syndrome_check() {
        for (i, &offset) in OFFSETS.iter().enumerate() {
            for info in [0u16, 1, 0x1234, 0xFFFF, 0xD3C2] {
                let block = encode_block(info, offset);
                assert_eq!(remainder(block), offset, "kind {i}, info {info:#x}");
            }
        }
    }

    #[test]
    fn rds_characters_map_to_unicode() {
        assert_eq!(rds_char(b'A'), 'A');
        assert_eq!(rds_char(0x24), '¤');
        assert_eq!(rds_char(0x80), 'á');
        assert_eq!(rds_char(0xDE), 'đ');
        assert_eq!(rds_char(0x05), ' ');
    }

    fn decode_groups(groups: &[[u16; 4]], repeat: usize, flip: &[usize]) -> RdsDecoder {
        let mut bits = Vec::new();
        for _ in 0..repeat {
            bits.extend(group_bits(groups));
        }
        for &f in flip {
            bits[f] = !bits[f];
        }
        let mut dec = RdsDecoder::new(256_000.0);
        // Start in the middle of a block to exercise the search.
        dec.push_bits(&bits[7..]);
        dec
    }

    #[test]
    fn decodes_ps_rt_pi_and_pty_from_bits() {
        let groups = cycle(0xD3C2, 10, "SUPERHET", "Hello RDS world");
        let dec = decode_groups(&groups, 3, &[]);
        assert!(dec.synced());
        let info = dec.info();
        assert_eq!(info.pi, Some(0xD3C2));
        assert_eq!(info.pty, Some(10));
        assert_eq!(info.ps.as_deref(), Some("SUPERHET"));
        assert_eq!(info.rt.as_deref(), Some("Hello RDS world"));
        assert_eq!(info.tp, Some(true));
        assert_eq!(info.music, Some(true));
    }

    #[test]
    fn corrects_single_bit_errors() {
        let groups = cycle(0xD3C2, 10, "SUPERHET", "Hello RDS world");
        let len = group_bits(&groups).len();
        // One bit error in several different blocks of the second and third cycle.
        let flips = [
            len + 30,
            len + 200,
            2 * len + 61,
            2 * len + 500,
            2 * len + 777,
        ];
        let dec = decode_groups(&groups, 4, &flips);
        let info = dec.info();
        assert_eq!(info.ps.as_deref(), Some("SUPERHET"));
        assert_eq!(info.rt.as_deref(), Some("Hello RDS world"));
        assert!(dec.synced());
    }

    #[test]
    fn a_new_station_replaces_the_old_text() {
        let a = cycle(0xD3C2, 10, "STATIONA", "Text A");
        let b = cycle(0xD3C3, 5, "STATIONB", "Text B");
        let mut bits = Vec::new();
        for _ in 0..3 {
            bits.extend(group_bits(&a));
        }
        for _ in 0..3 {
            bits.extend(group_bits(&b));
        }
        let mut dec = RdsDecoder::new(256_000.0);
        dec.push_bits(&bits);
        let info = dec.info();
        assert_eq!(info.pi, Some(0xD3C3));
        assert_eq!(info.ps.as_deref(), Some("STATIONB"));
        assert_eq!(info.rt.as_deref(), Some("Text B"));
        assert_eq!(info.pty, Some(5));
    }

    #[test]
    fn text_is_dropped_when_the_signal_is_gone() {
        let groups = cycle(0xD3C2, 10, "SUPERHET", "Hello RDS world");
        let mut dec = decode_groups(&groups, 3, &[]);
        assert!(dec.info().ps.is_some());
        let revision = dec.revision();
        dec.push_bits(&vec![false; 5_000]);
        assert!(!dec.synced());
        assert!(dec.info().ps.is_some(), "kept for a while");
        dec.push_bits(&vec![false; 8_000]);
        assert!(dec.info().is_empty());
        assert!(dec.revision() > revision);
    }

    #[test]
    fn noise_does_not_produce_text() {
        let mut state = 0x1234_5678_u32;
        let bits: Vec<bool> = (0..20_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state & 0x100 != 0
            })
            .collect();
        let mut dec = RdsDecoder::new(256_000.0);
        dec.push_bits(&bits);
        assert!(dec.info().ps.is_none());
        assert!(dec.info().rt.is_none());
    }

    #[test]
    fn recovers_bits_from_the_baseband_waveform() {
        let groups = cycle(0xD3C2, 10, "SUPERHET", "Hello RDS world");
        let bits = differential(&group_bits(&groups));
        let fs = 256_000.0;
        let mut gen_ = Baseband::new(bits, fs);
        let mut dec = RdsDecoder::new(fs);
        // 3.5 s; a constant carrier phase of 0.7 rad and some additive noise.
        let rot = Complex32::from_polar(1.0, 0.7);
        let mut state = 99u32;
        let mut noise = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (f64::from(state) / f64::from(u32::MAX) - 0.5) as f32
        };
        let total = (fs * 3.5) as usize;
        let block: Vec<Complex32> = (0..total)
            .map(|_| {
                let s = gen_.sample() as f32 * 0.02;
                Complex32::new(s, 0.0) * rot + Complex32::new(noise(), noise()) * 0.02
            })
            .collect();
        for c in block.chunks(4099) {
            dec.process(c);
        }
        let info = dec.info();
        assert_eq!(info.pi, Some(0xD3C2));
        assert_eq!(info.ps.as_deref(), Some("SUPERHET"));
        assert_eq!(info.rt.as_deref(), Some("Hello RDS world"));
    }
}
