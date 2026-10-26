//! RDS (IEC 62106) receiver: 57 kHz subcarrier baseband to decoded groups.
//!
//! Input is the complex baseband that [`crate::wfm::StereoMpx`] produces by mixing the FM
//! multiplex with the conjugate of the third harmonic of the recovered 19 kHz pilot, so the
//! RDS subcarrier lands at DC with a constant (unknown) phase. Signal path:
//!
//! 1. Low-pass + decimation, then resampling to exactly 19 kHz (16 samples per bit).
//! 2. Carrier recovery: a decision-directed BPSK Costas loop with frequency-locked-loop assist
//!    derotates the samples, so the pilot-derived phase may be off, jitter or drift (residual
//!    frequency errors of a few hundred Hz are pulled in).
//! 3. Matched filter for the biphase (Manchester) symbol at every sample and a second-order
//!    early/late delay-locked loop with interpolation for the bit clock. It follows clock errors
//!    of several hundred ppm (cheap RTL-SDR crystals); the 16 sample phases are only used for
//!    the initial acquisition.
//! 4. Soft symbols, differential decoding (immune to the 180 degree ambiguity); every bit
//!    carries a reliability.
//! 5. Block synchronisation by syndrome: ten check bits, five offset words (A, B, C, C', D),
//!    sync after two valid blocks at the right distance, loss after too many bad blocks, quick
//!    re-sync after a bit slip. Error bursts of up to five bits are corrected in sync; a
//!    correction is only accepted when the flipped bits are among the least reliable ones.
//! 6. Group decoding: PI (every group), PTY and TP (every group), groups 0A/0B (PS, TA, M/S) and
//!    2A/2B (RadioText). Content that needed a correction, and every change of content that was
//!    already complete, is only taken over after it was received twice in a row.

use std::sync::OnceLock;

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

/// Matched-filter history length (samples, power of two).
const HIST: usize = 64;
/// Bits of matched-filter energy collected for the initial bit-clock phase.
const ACQ_BITS: u32 = 48;
/// Early/late spacing of the timing detector in samples.
const EARLY_LATE: f64 = 2.0;
/// Timing loop: proportional and integral gain (natural frequency about 0.025 bit^-1).
const TIMING_KP: f64 = 0.058;
const TIMING_KI: f64 = 1.0e-3;
/// Largest bit-clock error the timing loop follows, in samples per bit (about 3000 ppm).
const TIMING_MAX_RATE: f64 = 0.05;
/// Carrier loop natural frequency in rad per bit and damping.
const CARRIER_WN: f64 = 0.040;
const CARRIER_ZETA: f64 = 0.8;
/// Gain of the frequency-locked-loop assist (rad per bit of frequency per unit error).
const CARRIER_FLL: f64 = 0.001;
/// Carrier frequency limit in rad per bit (about 500 Hz).
const CARRIER_MAX_RATE: f64 = 2.6;
/// The carrier loop is faster for this many symbols after start-up.
const CARRIER_BOOST_SYMBOLS: u64 = 400;
/// Smoothing of the mean symbol magnitude (per bit).
const LEVEL_ALPHA: f32 = 1.0 / 128.0;

/// Decoded data is dropped after this many bits (about 10 s) without block sync.
const STALE_BITS: u32 = 11_875;
/// After this many unsynchronised bits the bit clock is acquired anew.
const REACQUIRE_BITS: u32 = 4_750;
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
/// While synced, a search for a shifted block grid starts after this many bad blocks in a row.
const RESYNC_BAD_RUN: u32 = 3;
/// Longest error burst that the block code corrects.
const MAX_BURST: u32 = 5;

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

/// A burst-error pattern of the block code (`pattern` has its lowest and highest set bit at the
/// ends of the burst) at bit position `shift` from the newest bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Burst {
    pattern: u8,
    shift: u8,
}

impl Burst {
    fn len(self) -> u8 {
        (8 - self.pattern.leading_zeros()) as u8
    }
}

/// Burst pattern per error syndrome: the code corrects every burst of up to [`MAX_BURST`]
/// bits and these bursts have distinct syndromes. Shorter bursts win on a collision.
fn burst_table() -> &'static [Option<Burst>; 1024] {
    static TABLE: OnceLock<Box<[Option<Burst>; 1024]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = Box::new([None; 1024]);
        let mut patterns: Vec<u8> = (1..(1u16 << MAX_BURST)).map(|p| p as u8).collect();
        patterns.retain(|p| p & 1 == 1);
        patterns.sort_by_key(|p| 8 - p.leading_zeros());
        for pattern in patterns {
            let len = 8 - pattern.leading_zeros();
            for shift in 0..=(BLOCK_BITS as u32 - len) {
                let syn = usize::from(remainder(u32::from(pattern) << shift));
                if syn != 0 && table[syn].is_none() {
                    table[syn] = Some(Burst {
                        pattern,
                        shift: shift as u8,
                    });
                }
            }
        }
        table
    })
}

/// Largest mean reliability (relative to the block average) that the flipped bits of a burst
/// may have for the correction to be accepted, by burst length 1 to 5.
const GATE_1: f32 = 3.0;
const GATE_2: f32 = 1.2;
const GATE_3: f32 = 0.6;
const GATE_4: f32 = 0.45;
const GATE_5: f32 = 0.35;

fn burst_gate(len: u8) -> f32 {
    match len {
        1 => GATE_1,
        2 => GATE_2,
        3 => GATE_3,
        4 => GATE_4,
        _ => GATE_5,
    }
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
    /// Reliability of the last 32 bits, indexed by `bits & 31`.
    rel: [f32; 32],
    bits: u64,
    synced: bool,
    /// Search: end bit index and position of the last valid block.
    last_hit: Option<(u64, u8)>,
    /// Synced: bits until the end of the current block and its position.
    left: u64,
    pos: u8,
    /// Bit i set = block i blocks ago was bad.
    history: u64,
    /// Blocks in a row that were bad.
    bad_run: u32,
}

/// A block found by the sync search.
struct Hit {
    pos: u8,
    info: u16,
    c_prime: bool,
}

impl Framer {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Reliability of the bit `j` places before the newest one (0 = newest).
    fn rel_of(&self, j: u32) -> f32 {
        self.rel[(self.bits.wrapping_sub(1 + u64::from(j)) & 31) as usize]
    }

    /// Tries to turn a block that failed the check into one that passes it by flipping a burst
    /// of up to [`MAX_BURST`] bits that are unreliable. Returns the word and the burst length.
    fn correct(&self, wanted: &[u16]) -> Option<(u32, u8)> {
        let syn = remainder(self.reg);
        let table = burst_table();
        let mean = (0..BLOCK_BITS as u32).map(|j| self.rel_of(j)).sum::<f32>() / BLOCK_BITS as f32;
        let mut best: Option<(f32, u32, u8)> = None;
        for &offset in wanted {
            let Some(burst) = table[usize::from(syn ^ offset)] else {
                continue;
            };
            let len = burst.len();
            let mut cost = 0.0;
            for j in 0..u32::from(len) {
                if burst.pattern >> j & 1 == 1 {
                    cost += self.rel_of(u32::from(burst.shift) + j);
                }
            }
            let flipped = burst.pattern.count_ones() as f32;
            if cost / flipped > burst_gate(len) * mean {
                continue;
            }
            if best.is_none_or(|(c, ..)| cost < c) {
                let fixed = self.reg ^ (u32::from(burst.pattern) << burst.shift);
                best = Some((cost, fixed, len));
            }
        }
        best.map(|(_, word, len)| (word, len))
    }

    fn push(&mut self, bit: bool, reliability: f32) -> Option<BlockEvent> {
        self.reg = ((self.reg << 1) | u32::from(bit)) & 0x03FF_FFFF;
        self.rel[(self.bits & 31) as usize] = reliability;
        self.bits += 1;
        if !self.synced {
            return self.search().map(|hit| self.lock(hit));
        }
        let event = self.push_synced();
        if self.bad_run >= RESYNC_BAD_RUN
            && !matches!(event, Some(BlockEvent::Good { .. }))
            && let Some(hit) = self.search()
        {
            // The block grid moved (bit slip): continue on the new one.
            return Some(self.lock(hit));
        }
        event
    }

    fn lock(&mut self, hit: Hit) -> BlockEvent {
        self.synced = true;
        self.left = BLOCK_BITS;
        self.pos = (hit.pos + 1) % 4;
        self.history = 0;
        self.bad_run = 0;
        self.last_hit = None;
        BlockEvent::Good {
            pos: hit.pos,
            info: hit.info,
            corrected: false,
            c_prime: hit.c_prime,
        }
    }

    /// Looks at the last 26 bits: two valid blocks at the right distance make a sync.
    fn search(&mut self) -> Option<Hit> {
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
        let hit = Hit {
            pos,
            info: (self.reg >> 10) as u16,
            c_prime: kind == 3,
        };
        if let Some((end, last_pos)) = self.last_hit {
            let dist = self.bits - end;
            if dist.is_multiple_of(BLOCK_BITS) && dist / BLOCK_BITS <= SEARCH_BLOCKS {
                let expect = (u64::from(last_pos) + dist / BLOCK_BITS) % 4;
                if u64::from(pos) == expect {
                    return Some(hit);
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
        // Corrected word and the length of the burst that was flipped (0 = none).
        let (word, burst) = if wanted.contains(&syn) {
            (Some(self.reg), 0)
        } else {
            match self.correct(wanted) {
                Some((word, len)) => (Some(word), len),
                None => (None, 0),
            }
        };
        let window = (1u64 << LOSS_WINDOW) - 1;
        self.history = (self.history << 1) & window;
        // Long corrections are too often wrong to keep a failing sync alive.
        let weak = word.is_none() || burst > 2;
        if weak {
            self.history |= 1;
        }
        let Some(word) = word else {
            self.bad_run += 1;
            if self.history.count_ones() > LOSS_BAD_BLOCKS {
                self.synced = false;
                self.last_hit = None;
                self.history = 0;
                self.bad_run = 0;
                return Some(BlockEvent::Lost);
            }
            return Some(BlockEvent::Bad { pos });
        };
        if weak {
            self.bad_run += 1;
        } else {
            self.bad_run = 0;
            self.last_hit = None;
        }
        let c_prime = pos == 2 && remainder(word) == OFFSETS[3];
        Some(BlockEvent::Good {
            pos,
            info: (word >> 10) as u16,
            corrected: burst > 0,
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
    rt_pending_b: [Option<[u8; 2]>; RT_LEN / 4],
    rt_ab: Option<bool>,
    rt_ab_pending: Option<bool>,
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
            rt_pending_b: [None; RT_LEN / 4],
            rt_ab: None,
            rt_ab_pending: None,
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
                if pos == 0 || c_prime {
                    self.accept_pi(info);
                }
                if pos == 3 {
                    self.finish_group();
                }
            }
        }
    }

    /// PI from block A (or the repeat in block C' of version B groups). A PI is only taken
    /// over when it was received twice (PI is repeated in every group, so this costs next to
    /// nothing): one lucky false block can neither show a bogus station nor wipe the decoded
    /// text of the right one.
    fn accept_pi(&mut self, pi: u16) {
        if self.info.pi == Some(pi) {
            self.pi_pending = None;
            return;
        }
        if self.pi_pending != Some(pi) {
            self.pi_pending = Some(pi);
            return;
        }
        self.pi_pending = None;
        if self.info.pi.is_some() {
            // A different station: forget everything that belonged to the previous one.
            self.clear_station();
        }
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
        let known = self.ps[seg * 2].zip(self.ps[seg * 2 + 1]);
        // A segment that was never received is taken from an error-free group at once; a
        // correction or a change of what is shown needs a second, identical reception.
        let trusted = clean && known.is_none_or(|k| k == (chars[0], chars[1]));
        if trusted || self.ps_pending[seg] == Some(chars) {
            self.ps[seg * 2] = Some(chars[0]);
            self.ps[seg * 2 + 1] = Some(chars[1]);
            self.ps_pending[seg] = None;
        } else {
            self.ps_pending[seg] = Some(chars);
        }
    }

    /// Handles the A/B flag of RadioText: a toggle starts a new message. A flag from a corrected
    /// block only counts when it is received twice in a row.
    fn rt_flag(&mut self, ab: bool, clean: bool) {
        if self.rt_ab == Some(ab) {
            self.rt_ab_pending = None;
            return;
        }
        if self.rt_ab.is_some() && !clean && self.rt_ab_pending != Some(ab) {
            self.rt_ab_pending = Some(ab);
            return;
        }
        self.rt_ab_pending = None;
        if self.rt_ab.is_some() {
            self.rt = [None; RT_LEN];
            self.rt_pending = [None; RT_LEN / 4];
            self.rt_pending_b = [None; RT_LEN / 4];
        }
        self.rt_ab = Some(ab);
    }

    fn group_rt(&mut self, b: u16, c: Option<u16>, d: u16, version_b: bool, clean: bool) {
        let ab = b & 0x0010 != 0;
        self.rt_flag(ab, clean);
        if self.rt_ab != Some(ab) {
            // The flag could not be confirmed yet: do not mix the segment into the old text.
            return;
        }
        let seg = usize::from(b & 0x0F);
        if version_b {
            // 2B: 32 characters, two per group.
            let chars = [(d >> 8) as u8, (d & 0xFF) as u8];
            let known = self.rt[seg * 2].zip(self.rt[seg * 2 + 1]);
            let trusted = clean && known.is_none_or(|k| k == (chars[0], chars[1]));
            if trusted || self.rt_pending_b[seg] == Some(chars) {
                self.rt[seg * 2] = Some(chars[0]);
                self.rt[seg * 2 + 1] = Some(chars[1]);
                self.rt_pending_b[seg] = None;
            } else {
                self.rt_pending_b[seg] = Some(chars);
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
        let known = (0..4)
            .map(|i| self.rt[seg * 4 + i])
            .collect::<Option<Vec<u8>>>();
        let trusted = clean && known.is_none_or(|k| k == chars);
        if trusted || self.rt_pending[seg] == Some(chars) {
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

/// Length of the matched filter in samples (two bits).
const MF_LEN: usize = 2 * SPB;

/// The matched-filter kernel (to be applied time-reversed): the pulse of one bit as shaped by
/// IEC 62106, a Manchester pair of the impulse response of the `cos(pi f T / 4)` spectrum
/// (band-limited to 2.375 kHz), over a window that starts half a bit before the bit.
/// Normalised to an energy of `SPB`.
fn matched_kernel() -> [f32; MF_LEN] {
    let mut q = [0.0f32; MF_LEN];
    let tb = 1.0 / BIT_RATE_HZ;
    let fc = 2.0 / tb;
    let a = std::f64::consts::PI * tb / 4.0;
    let r = |t: f64| {
        let b = std::f64::consts::TAU * t;
        let term = |d: f64| {
            if d.abs() < 1e-9 {
                fc
            } else {
                (d * fc).sin() / d
            }
        };
        term(a - b) + term(a + b)
    };
    let mut energy = 0.0f64;
    let mut raw = [0.0f64; MF_LEN];
    for (j, v) in raw.iter_mut().enumerate() {
        let t = (j as f64 - SPB as f64 / 2.0) / SPB as f64 * tb;
        *v = r(t) - r(t - tb / 2.0);
        energy += *v * *v;
    }
    let scale = (SPB as f64 / energy).sqrt();
    for (v, r) in q.iter_mut().zip(raw) {
        *v = (r * scale) as f32;
    }
    q
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
    // Carrier recovery: phase of the derotation in rad, frequency in rad per bit.
    theta: f64,
    omega: f64,
    prev_axis: Complex32,
    // Matched filter over the derotated samples and its history.
    ring: [Complex32; MF_LEN],
    kernel: [f32; MF_LEN],
    hist: [Complex32; HIST],
    /// Samples processed so far.
    n: u64,
    // Bit clock: matched-filter energy per phase while acquiring, then the strobe position (in
    // samples, fractional) with the timing loop's integrator.
    acq_energy: [f32; SPB],
    acquired: bool,
    strobe: f64,
    timing_rate: f64,
    sym_count: u64,
    // Soft symbols.
    prev_soft: f32,
    level: f32,
    framer: Framer,
    groups: GroupDecoder,
    /// Bits since the block sync was last held.
    unsynced_bits: u32,
    /// Blocks seen in sync since the last reset (for diagnostics).
    good_blocks: u64,
    bad_blocks: u64,
    /// Every decided bit (test builds only).
    #[cfg(test)]
    pub(crate) bit_log: Vec<bool>,
    /// Every valid block: position, information word, whether it was corrected (test builds
    /// only).
    #[cfg(test)]
    pub(crate) block_log: Vec<(u8, u16, bool)>,
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
            theta: 0.0,
            omega: 0.0,
            prev_axis: Complex32::new(1.0, 0.0),
            ring: [Complex32::new(0.0, 0.0); MF_LEN],
            kernel: matched_kernel(),
            hist: [Complex32::new(0.0, 0.0); HIST],
            n: 0,
            acq_energy: [0.0; SPB],
            acquired: false,
            strobe: 0.0,
            timing_rate: 0.0,
            sym_count: 0,
            prev_soft: 0.0,
            level: 0.0,
            framer: Framer::default(),
            groups: GroupDecoder::new(),
            unsynced_bits: 0,
            good_blocks: 0,
            bad_blocks: 0,
            #[cfg(test)]
            bit_log: Vec::new(),
            #[cfg(test)]
            block_log: Vec::new(),
        }
    }

    /// Forgets the filter state and everything decoded (call on retune).
    pub fn reset(&mut self) {
        self.lpf.reset();
        self.res_i.reset();
        self.res_q.reset();
        self.reset_recovery();
        self.framer.reset();
        self.groups.reset();
        self.unsynced_bits = 0;
        self.good_blocks = 0;
        self.bad_blocks = 0;
    }

    /// Restarts carrier and bit-clock recovery.
    fn reset_recovery(&mut self) {
        self.theta = 0.0;
        self.omega = 0.0;
        self.prev_axis = Complex32::new(1.0, 0.0);
        self.ring = [Complex32::new(0.0, 0.0); MF_LEN];
        self.hist = [Complex32::new(0.0, 0.0); HIST];
        self.n = 0;
        self.acq_energy = [0.0; SPB];
        self.acquired = false;
        self.strobe = 0.0;
        self.timing_rate = 0.0;
        self.sym_count = 0;
        self.prev_soft = 0.0;
        self.level = 0.0;
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
        // A non-finite sample would poison the loops for good.
        let z = if z.re.is_finite() && z.im.is_finite() {
            z
        } else {
            Complex32::new(0.0, 0.0)
        };
        // Derotate with the carrier estimate.
        let (s, c) = self.theta.sin_cos();
        let zr = z * Complex32::new(c as f32, -s as f32);
        self.theta += self.omega / SPB as f64;
        if self.theta > std::f64::consts::PI {
            self.theta -= std::f64::consts::TAU;
        } else if self.theta < -std::f64::consts::PI {
            self.theta += std::f64::consts::TAU;
        }
        let ring_slot = (self.n % MF_LEN as u64) as usize;
        self.ring[ring_slot] = zr;
        // Matched filter for the biphase symbol, time-reversed kernel over the last MF_LEN
        // samples.
        let mut m = Complex32::new(0.0, 0.0);
        for (j, &q) in self.kernel.iter().enumerate() {
            m += self.ring[(ring_slot + 1 + j) % MF_LEN] * q;
        }
        let slot = (self.n % SPB as u64) as usize;
        self.hist[(self.n % HIST as u64) as usize] = m;
        self.n += 1;
        if !self.acquired {
            self.acquire(slot, m);
        } else {
            // Strobe once the late sample of the next position exists.
            // (A symbol can end in a restart of the recovery, which leaves nothing to do.)
            while self.acquired && self.strobe + EARLY_LATE + 2.0 <= self.n.saturating_sub(1) as f64
            {
                self.symbol();
            }
        }
    }

    /// Initial bit-clock phase: the sample phase with the most matched-filter energy.
    fn acquire(&mut self, slot: usize, m: Complex32) {
        self.acq_energy[slot] += m.norm_sqr();
        if self.n < u64::from(ACQ_BITS) * SPB as u64 {
            return;
        }
        let e = &self.acq_energy;
        let top = (0..SPB).fold(0, |t, i| if e[i] > e[t] { i } else { t });
        // Parabolic refinement between the neighbouring phases.
        let (a, b, c) = (e[(top + SPB - 1) % SPB], e[top], e[(top + 1) % SPB]);
        let denom = a - 2.0 * b + c;
        let frac = if denom < 0.0 {
            (0.5 * f64::from((a - c) / denom)).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        // First position at or after the middle of the history that has that phase.
        let base = self.n - (HIST as u64) / 2;
        let aligned = base - base % SPB as u64 + top as u64;
        let aligned = if aligned < base {
            aligned + SPB as u64
        } else {
            aligned
        };
        self.strobe = aligned as f64 + frac;
        self.timing_rate = 0.0;
        self.acquired = true;
        self.sym_count = 0;
        self.prev_soft = 0.0;
        self.level = 0.0;
    }

    /// Matched-filter output at a fractional sample position (cubic Lagrange interpolation).
    fn matched_at(&self, pos: f64) -> Complex32 {
        let i = pos.floor();
        let x = (pos - i) as f32;
        let i = i as u64;
        let at = |k: u64| self.hist[((i + k - 1) % HIST as u64) as usize];
        let (p0, p1, p2, p3) = (at(0), at(1), at(2), at(3));
        // Lagrange weights for the points -1, 0, 1, 2.
        let w0 = -x * (x - 1.0) * (x - 2.0) / 6.0;
        let w1 = (x + 1.0) * (x - 1.0) * (x - 2.0) / 2.0;
        let w2 = -(x + 1.0) * x * (x - 2.0) / 2.0;
        let w3 = (x + 1.0) * x * (x - 1.0) / 6.0;
        p0 * w0 + p1 * w1 + p2 * w2 + p3 * w3
    }

    /// One bit: timing and carrier loops, soft symbol, differential decision.
    fn symbol(&mut self) {
        let prompt = self.matched_at(self.strobe);
        let early = self.matched_at(self.strobe - EARLY_LATE).norm_sqr();
        let late = self.matched_at(self.strobe + EARLY_LATE).norm_sqr();
        self.sym_count += 1;

        // Timing: the peak of the matched-filter output is where early and late are equal.
        let total = early + late;
        let timing_err = if total > 1e-20 {
            f64::from((late - early) / total)
        } else {
            0.0
        };
        self.timing_rate =
            (self.timing_rate + TIMING_KI * timing_err).clamp(-TIMING_MAX_RATE, TIMING_MAX_RATE);
        self.strobe += SPB as f64 + TIMING_KP * timing_err + self.timing_rate;

        // Carrier: BPSK Costas detector plus a frequency detector on the doubled angle.
        let power = prompt.norm_sqr();
        if power > 1e-20 {
            let boost = if self.sym_count < CARRIER_BOOST_SYMBOLS {
                3.0
            } else {
                1.0
            };
            let phase_err = f64::from(prompt.re * prompt.im / power);
            let axis = prompt * prompt / power;
            let freq_err = f64::from((axis * self.prev_axis.conj()).im);
            let kp = 2.0 * CARRIER_ZETA * CARRIER_WN * boost;
            let ki = (CARRIER_WN * boost).powi(2);
            self.omega = (self.omega + ki * phase_err + CARRIER_FLL * freq_err)
                .clamp(-CARRIER_MAX_RATE, CARRIER_MAX_RATE);
            let step = kp * phase_err;
            self.theta += step;
            // The phase step is not frequency: take it out of the next frequency measurement.
            self.prev_axis = axis * Complex32::from_polar(1.0, (-2.0 * step) as f32);
        }

        // Soft decision on the derotated symbol; the sign flips between successive bits are
        // the data (differential coding), reliability is the smaller of the two magnitudes.
        let soft = prompt.re;
        let mag = soft.abs();
        self.level = if self.level == 0.0 {
            mag
        } else {
            self.level + LEVEL_ALPHA * (mag - self.level)
        };
        let reliability = if self.level > 1e-20 {
            mag.min(self.prev_soft.abs()) / self.level
        } else {
            0.0
        };
        let bit = soft * self.prev_soft < 0.0;
        self.prev_soft = soft;
        self.push_bit_rel(bit, reliability);
    }

    fn push_bit_rel(&mut self, bit: bool, reliability: f32) {
        #[cfg(test)]
        self.bit_log.push(bit);
        if self.framer.synced {
            self.unsynced_bits = 0;
        } else {
            self.unsynced_bits = self.unsynced_bits.saturating_add(1);
            if self.unsynced_bits == REACQUIRE_BITS {
                // Nothing decodable for a while: look for the bit clock and carrier anew.
                self.reset_recovery();
            }
            if self.unsynced_bits == STALE_BITS && !self.groups.info.is_empty() {
                // The station is gone (or the pilot): do not show its text forever.
                self.groups.reset();
            }
        }
        if let Some(event) = self.framer.push(bit, reliability) {
            match event {
                BlockEvent::Good {
                    pos,
                    info,
                    corrected,
                    ..
                } => {
                    self.good_blocks += 1;
                    #[cfg(test)]
                    self.block_log.push((pos, info, corrected));
                    #[cfg(not(test))]
                    let _ = (pos, info, corrected);
                }
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
            self.push_bit_rel(b, 1.0);
        }
    }

    /// Feeds already differentially decoded bits with their reliabilities (test hook).
    #[cfg(test)]
    pub(crate) fn push_soft_bits(&mut self, bits: &[(bool, f32)]) {
        for &(b, rel) in bits {
            self.push_bit_rel(b, rel);
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

    #[test]
    fn every_burst_of_up_to_five_bits_has_its_own_syndrome() {
        let table = burst_table();
        let mut seen = 0;
        for len in 1..=MAX_BURST {
            for pattern in (1u32 << (len - 1)..1 << len).filter(|p| p & 1 == 1) {
                for shift in 0..=(26 - len) {
                    let syn = usize::from(remainder(pattern << shift));
                    let burst = table[syn].expect("burst has a syndrome");
                    assert_eq!(
                        (u32::from(burst.pattern), u32::from(burst.shift)),
                        (pattern, shift),
                        "burst {pattern:#b} at {shift} collides"
                    );
                    seen += 1;
                }
            }
        }
        assert_eq!(seen, 26 + 25 + 2 * 24 + 4 * 23 + 8 * 22);
        assert_eq!(table.iter().flatten().count(), seen);
    }

    /// Bits of `repeat` cycles with a burst of `len` bits (all ones) flipped at `at` in every
    /// block listed; the flipped bits get reliability `burst_rel`, the others 1.
    fn soft_stream(
        groups: &[[u16; 4]],
        repeat: usize,
        bursts: &[(usize, usize)],
        burst_rel: f32,
    ) -> Vec<(bool, f32)> {
        let mut bits = Vec::new();
        for _ in 0..repeat {
            bits.extend(group_bits(groups));
        }
        let mut soft: Vec<(bool, f32)> = bits.into_iter().map(|b| (b, 1.0)).collect();
        for &(at, len) in bursts {
            for bit in soft.iter_mut().skip(at).take(len) {
                bit.0 = !bit.0;
                bit.1 = burst_rel;
            }
        }
        soft
    }

    #[test]
    fn corrects_unreliable_bursts_of_up_to_five_bits() {
        let groups = cycle(0xD3C2, 10, "SUPERHET", "Hello RDS world");
        let len = group_bits(&groups).len();
        // Bursts of 1 to 5 bits in different blocks of cycles two and three.
        let bursts = [
            (len + 30, 1),
            (len + 160, 2),
            (len + 300, 3),
            (len + 470, 4),
            (2 * len + 61, 5),
            (2 * len + 400, 5),
            (2 * len + 700, 3),
        ];
        let soft = soft_stream(&groups, 4, &bursts, 0.1);
        let mut dec = RdsDecoder::new(256_000.0);
        dec.push_soft_bits(&soft[11..]);
        assert!(dec.synced());
        assert_eq!(dec.block_counts().1, 0, "all bursts corrected");
        let info = dec.info();
        assert_eq!(info.ps.as_deref(), Some("SUPERHET"));
        assert_eq!(info.rt.as_deref(), Some("Hello RDS world"));
    }

    #[test]
    fn does_not_correct_long_bursts_that_look_reliable() {
        let groups = cycle(0xD3C2, 10, "SUPERHET", "Hello RDS world");
        let len = group_bits(&groups).len();
        // A five-bit burst of confidently received bits is far more likely a miscorrection.
        let soft = soft_stream(&groups, 4, &[(len + 300, 5)], 1.0);
        let mut dec = RdsDecoder::new(256_000.0);
        dec.push_soft_bits(&soft[11..]);
        assert_eq!(dec.block_counts().1, 1, "the block is dropped");
        let info = dec.info();
        assert_eq!(info.ps.as_deref(), Some("SUPERHET"));
        assert_eq!(info.rt.as_deref(), Some("Hello RDS world"));
    }

    #[test]
    fn finds_the_block_grid_again_after_a_bit_slip() {
        let groups = cycle(0xD3C2, 10, "SUPERHET", "Hello RDS world");
        let len = group_bits(&groups).len();
        let mut bits = Vec::new();
        for _ in 0..6 {
            bits.extend(group_bits(&groups));
        }
        // The bit clock gains a bit: one bit more than was sent.
        bits.insert(2 * len + 333, true);
        let mut dec = RdsDecoder::new(256_000.0);
        dec.push_bits(&bits);
        assert!(dec.synced());
        let (good, bad) = dec.block_counts();
        assert!(bad < 12, "only a few blocks lost, got {bad}");
        assert!(good > 6 * (len as u64 / 26) - 30, "good {good}");
        assert_eq!(dec.info().ps.as_deref(), Some("SUPERHET"));
    }

    #[test]
    fn a_single_reception_does_not_change_a_complete_ps() {
        let a = cycle(0xD3C2, 10, "STATIONA", "Text A");
        let b = cycle(0xD3C2, 10, "STATIONB", "Text A");
        let mut bits = Vec::new();
        for _ in 0..3 {
            bits.extend(group_bits(&a));
        }
        // One cycle of something else, e.g. a block that passed the check by accident.
        bits.extend(group_bits(&b));
        for _ in 0..2 {
            bits.extend(group_bits(&a));
        }
        let mut dec = RdsDecoder::new(256_000.0);
        dec.push_bits(&bits);
        assert_eq!(dec.info().ps.as_deref(), Some("STATIONA"));
        // The same change received twice in a row is real.
        let mut bits = Vec::new();
        for _ in 0..2 {
            bits.extend(group_bits(&b));
        }
        dec.push_bits(&bits);
        assert_eq!(dec.info().ps.as_deref(), Some("STATIONB"));
    }

    #[test]
    fn toggling_the_radiotext_flag_starts_a_new_message() {
        let text = |pi: u16, ab: bool, msg: &str| -> Vec<[u16; 4]> {
            let mut bytes = msg.as_bytes().to_vec();
            bytes.push(RT_END);
            while !bytes.len().is_multiple_of(4) {
                bytes.push(b' ');
            }
            bytes
                .chunks(4)
                .enumerate()
                .map(|(seg, c)| group_2a(pi, 10, ab, seg as u16, [c[0], c[1], c[2], c[3]]))
                .collect()
        };
        let mut bits = Vec::new();
        for _ in 0..3 {
            bits.extend(group_bits(&text(0xD3C2, false, "A long first message")));
        }
        for _ in 0..3 {
            bits.extend(group_bits(&text(0xD3C2, true, "Short")));
        }
        let mut dec = RdsDecoder::new(256_000.0);
        dec.push_bits(&bits);
        assert_eq!(dec.info().rt.as_deref(), Some("Short"));
    }

    #[test]
    fn a_corrected_ab_flag_does_not_wipe_the_text() {
        let groups = cycle(0xD3C2, 10, "SUPERHET", "Hello RDS world");
        let len = group_bits(&groups).len();
        // A burst in block B of the first RadioText group (flips the A/B bit among others);
        // the 2A group follows the four 0A groups of the cycle: group 4, block B.
        let at = len + 4 * 104 + 26 + 8;
        let soft = soft_stream(&groups, 4, &[(at, 4)], 0.1);
        let mut dec = RdsDecoder::new(256_000.0);
        dec.push_soft_bits(&soft);
        assert_eq!(dec.info().rt.as_deref(), Some("Hello RDS world"));
        assert_eq!(dec.block_counts().1, 0);
    }
}
