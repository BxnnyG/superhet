//! Per-VFO channelizer and demodulator.
//!
//! Signal path (all streaming, no I/O):
//!
//! 1. [`Nco`] shifts the VFO to DC (`offset = vfo - centre`; retuning only changes the phase
//!    increment).
//! 2. Multi-stage FIR decimation (computing only kept outputs) down to a channel rate
//!    `input_rate / D` that suits the mode (WFM >= 240 kHz, NFM/AM >= 2.5x bandwidth, >= 24 kHz).
//!    `D` is chosen 5-smooth where possible, so stages stay small; the channel rate is
//!    generally not an integer multiple of 48 kHz.
//! 3. Channel filter (Kaiser FIR, passband = `bandwidth / 2`) at the channel rate; channel
//!    power (dBFS) and squelch are measured here.
//! 4. Demodulator ([`FmDemod`] / [`AmDemod`] / [`SsbDemod`]). USB / LSB shift the wanted sideband
//!    to DC before the channel filter (so the filter keeps only that sideband) and shift it back
//!    in the product detector; CW filters around the carrier and shifts it to the BFO pitch.
//! 5. Audio decimating FIR (low-pass at the audio bandwidth, integer factor towards 48 kHz),
//!    DC block and de-emphasis for FM.
//! 6. Arbitrary-ratio [`Resampler`] to exactly [`OUTPUT_RATE`], then the squelch gate.
//!
//! WFM additionally runs the multiplex through a pilot PLL ([`StereoMpx`]): the L-R signal is
//! demodulated with the doubled pilot phase, filtered like the L+R path, de-emphasised and
//! matrixed to left/right; the matrix gain follows the pilot lock, so a weak or missing pilot
//! fades to mono. The 57 kHz RDS subcarrier is mixed to DC with the tripled pilot phase and fed
//! to an [`RdsDecoder`]. Stereo output is opt-in ([`Channel::set_stereo`]); RDS is always decoded.
//!
//! Changing mode, bandwidth or input rate rebuilds the filter chain (state is cleared; the NCO
//! phase and squelch settings are kept). Changing the offset is glitch-free.

use num_complex::Complex32;

use crate::demod::{
    AgcTimes, AmDemod, DcBlock, Deemph, FmDemod, NFM_REF_DEVIATION_HZ, SsbDemod, WFM_DEVIATION_HZ,
};
use crate::fir::{DecimFir, design_lowpass};
use crate::nco::Nco;
use crate::rds::{RdsDecoder, RdsInfo};
use crate::resample::Resampler;
use crate::wfm::StereoMpx;

/// Audio output sample rate in Hz.
pub const OUTPUT_RATE: u32 = 48_000;

/// Input samples handled per internal pass (bounds scratch memory).
const CHUNK: usize = 8192;
/// Stopband attenuation of decimation / channel filters in dB.
const CHANNEL_ATTEN_DB: f64 = 70.0;
/// Stopband attenuation of the audio filter in dB.
const AUDIO_ATTEN_DB: f64 = 60.0;
/// Largest decimation factor of a single stage.
const MAX_STAGE_FACTOR: usize = 6;
/// Squelch closes only when the power falls this far below the open threshold.
const SQUELCH_HYSTERESIS_DB: f32 = 3.0;
/// Time the squelch stays open after the power drops below the close threshold.
const SQUELCH_HANG_S: f64 = 0.1;
/// Squelch decision interval.
const SQUELCH_WINDOW_S: f64 = 0.005;
/// Gate fade time (avoids clicks when the squelch toggles).
const GATE_RAMP_S: f64 = 0.005;
/// Lower audio edge of the SSB passband in Hz (the part of the sideband next to the carrier that is
/// cut off).
pub const SSB_LOW_CUT_HZ: f64 = 300.0;
/// Transition width of the SSB / CW channel filter.
const SSB_TRANSITION_HZ: f64 = 250.0;
/// Default CW pitch (BFO offset) in Hz.
pub const CW_DEFAULT_PITCH_HZ: u32 = 700;
/// Accepted CW pitch range in Hz.
pub const CW_PITCH_RANGE_HZ: std::ops::RangeInclusive<u32> = 200..=2_000;
/// Time the stereo matrix needs to fade between mono and stereo.
const STEREO_BLEND_S: f64 = 0.05;
/// A retune by more than this is a different station: forget RDS and the pilot lock.
const STATION_CHANGE_HZ: f64 = 5_000.0;
/// Lowest power reported (avoids `-inf`).
const POWER_FLOOR: f32 = 1e-20;

/// De-emphasis time constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Deemphasis {
    /// No de-emphasis.
    None,
    /// 50 us (Europe).
    #[default]
    Us50,
    /// 75 us (Americas, Korea).
    Us75,
}

impl Deemphasis {
    fn tau_us(self) -> Option<f64> {
        match self {
            Self::None => None,
            Self::Us50 => Some(50.0),
            Self::Us75 => Some(75.0),
        }
    }
}

/// Demodulation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Broadcast FM (stereo and RDS are decoded, see [`Channel::set_stereo`]).
    Wfm {
        /// De-emphasis (default 50 us).
        deemphasis: Deemphasis,
    },
    /// Narrow-band FM; full-scale audio at 5 kHz deviation.
    Nfm {
        /// De-emphasis (default off).
        deemphasis: Deemphasis,
    },
    /// Envelope AM with AGC.
    Am,
    /// Upper sideband: passband `300 Hz ..= 300 Hz + bandwidth` above the VFO frequency.
    Usb,
    /// Lower sideband: passband `300 Hz ..= 300 Hz + bandwidth` below the VFO frequency.
    Lsb,
    /// CW: filter centred on the VFO frequency, a carrier there is heard as a tone at
    /// `pitch_hz`.
    Cw {
        /// Tone frequency of a carrier exactly on the VFO frequency (default 700 Hz).
        pitch_hz: u32,
    },
}

impl Mode {
    /// WFM with 50 us de-emphasis.
    pub fn wfm() -> Self {
        Self::Wfm {
            deemphasis: Deemphasis::Us50,
        }
    }

    /// NFM without de-emphasis.
    pub fn nfm() -> Self {
        Self::Nfm {
            deemphasis: Deemphasis::None,
        }
    }

    /// CW with the default pitch.
    pub fn cw() -> Self {
        Self::Cw {
            pitch_hz: CW_DEFAULT_PITCH_HZ,
        }
    }

    /// Default channel bandwidth in Hz (WFM 180 kHz, NFM 12.5 kHz, AM 10 kHz, SSB 2.4 kHz,
    /// CW 500 Hz). For USB / LSB the bandwidth is the width of the passband, which starts
    /// 300 Hz away from the VFO frequency (default 300 Hz ..= 2.7 kHz).
    pub fn default_bandwidth_hz(self) -> u32 {
        match self {
            Self::Wfm { .. } => 180_000,
            Self::Nfm { .. } => 12_500,
            Self::Am => 10_000,
            Self::Usb | Self::Lsb => 2_400,
            Self::Cw { .. } => 500,
        }
    }

    /// Smallest supported bandwidth in Hz.
    fn min_bandwidth_hz(self) -> u32 {
        match self {
            Self::Wfm { .. } | Self::Nfm { .. } | Self::Am => 1_000,
            Self::Usb | Self::Lsb => 300,
            Self::Cw { .. } => 100,
        }
    }

    fn deemphasis(self) -> Deemphasis {
        match self {
            Self::Wfm { deemphasis } | Self::Nfm { deemphasis } => deemphasis,
            Self::Am | Self::Usb | Self::Lsb | Self::Cw { .. } => Deemphasis::None,
        }
    }
}

/// Channel parameters.
#[derive(Debug, Clone, Copy)]
pub struct ChannelConfig {
    /// Input (device) sample rate in Hz.
    pub input_rate: u32,
    /// VFO frequency minus centre frequency in Hz (`|offset| < input_rate / 2`).
    pub offset_hz: f64,
    /// Demodulation mode.
    pub mode: Mode,
    /// Channel bandwidth in Hz; `None` uses the mode default.
    pub bandwidth_hz: Option<u32>,
    /// Squelch threshold in dBFS of the channel power; `None` keeps the squelch open.
    pub squelch_dbfs: Option<f32>,
}

/// Invalid channel parameters.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum DspError {
    /// Input rate outside the supported range.
    #[error("input rate {0} Hz is outside 100 kHz..=20 MHz")]
    InputRate(u32),
    /// Offset is not strictly inside the input band.
    #[error("offset {0} Hz is outside the input band")]
    Offset(f64),
    /// Bandwidth is below 1 kHz or too wide for the channel rate of the mode.
    #[error("bandwidth {0} Hz is not supported for this mode and input rate")]
    Bandwidth(u32),
    /// CW pitch outside 200 Hz..=2 kHz.
    #[error("CW pitch {0} Hz is outside 200..=2000 Hz")]
    Pitch(u32),
}

/// Per-call measurements.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelStats {
    /// Mean channel power (after the channel filter) of this call in dBFS; a complex tone of
    /// amplitude 1.0 reads 0 dBFS. Holds the previous value when the call produced no samples.
    pub power_dbfs: f32,
    /// True if audio is passing (squelch open or disabled) at the end of the call.
    pub squelch_open: bool,
}

enum Demod {
    Fm(FmDemod),
    Am(AmDemod),
    Ssb(SsbDemod),
}

/// Broadcast-FM extras: stereo difference path and RDS.
struct WfmPath {
    mpx: StereoMpx,
    rds: RdsDecoder,
    /// L-R path: same filter as the L+R `audio_dec`.
    audio_dec_d: DecimFir<f32>,
    deemph_d: Option<Deemph>,
    resampler_r: Resampler,
    /// Current stereo matrix gain (0 = mono, 1 = full stereo).
    blend: f32,
    /// Blend change per audio sample.
    blend_step: f32,
}

/// Everything that depends on mode, bandwidth and input rate.
struct Chain {
    stages: Vec<DecimFir<Complex32>>,
    /// Moves the wanted sideband to DC (USB / LSB).
    pre_shift: Option<Nco>,
    channel_filter: DecimFir<Complex32>,
    channel_rate: f64,
    demod: Demod,
    audio_dec: DecimFir<f32>,
    dc_block: Option<DcBlock>,
    deemph: Option<Deemph>,
    resampler: Resampler,
    wfm: Option<WfmPath>,
}

impl Chain {
    fn build(rate: u32, mode: Mode, bandwidth_hz: u32) -> Result<Self, DspError> {
        if !(100_000..=20_000_000).contains(&rate) {
            return Err(DspError::InputRate(rate));
        }
        let fs = f64::from(rate);
        let bw = f64::from(bandwidth_hz);
        if bandwidth_hz < mode.min_bandwidth_hz() {
            return Err(DspError::Bandwidth(bandwidth_hz));
        }
        if let Mode::Cw { pitch_hz } = mode
            && !CW_PITCH_RANGE_HZ.contains(&pitch_hz)
        {
            return Err(DspError::Pitch(pitch_hz));
        }
        // SSB: the decimation stages must pass the whole sideband up to the outer edge (they are
        // symmetric around DC), the channel filter only the passband after it was shifted to DC.
        let ssb_shift = match mode {
            Mode::Usb => Some(SSB_LOW_CUT_HZ + bw / 2.0),
            Mode::Lsb => Some(-(SSB_LOW_CUT_HZ + bw / 2.0)),
            _ => None,
        };
        let stage_bw = ssb_shift.map_or(bw, |shift| 2.0 * (shift.abs() + bw / 2.0));
        let min_rate = match mode {
            Mode::Wfm { .. } => (1.25 * bw).max(240_000.0),
            Mode::Nfm { .. } | Mode::Am | Mode::Usb | Mode::Lsb | Mode::Cw { .. } => {
                (2.5 * stage_bw).max(24_000.0)
            }
        };
        let d = pick_decimation(((fs / min_rate).floor() as usize).max(1));
        let rch = fs / d as f64;
        if rch < 1.15 * stage_bw {
            return Err(DspError::Bandwidth(bandwidth_hz));
        }

        let mut stages = Vec::new();
        let mut rate_k = fs;
        for m in stage_factors(d) {
            let out_rate = rate_k / m as f64;
            let taps = design_lowpass(
                rate_k,
                stage_bw / 2.0,
                out_rate - stage_bw / 2.0,
                CHANNEL_ATTEN_DB,
            );
            stages.push(DecimFir::new(taps, m));
            rate_k = out_rate;
        }
        let fstop = match mode {
            Mode::Usb | Mode::Lsb => bw / 2.0 + SSB_TRANSITION_HZ,
            Mode::Cw { .. } => (bw / 2.0 * 1.25).max(bw / 2.0 + 60.0),
            _ => bw / 2.0 * 1.25,
        }
        .min(0.49 * rch);
        let channel_filter =
            DecimFir::new(design_lowpass(rch, bw / 2.0, fstop, CHANNEL_ATTEN_DB), 1);

        let (demod, fc_pass, fc_stop) = match mode {
            Mode::Wfm { .. } => (
                Demod::Fm(FmDemod::new(rch, WFM_DEVIATION_HZ)),
                15_000.0,
                18_500.0,
            ),
            Mode::Nfm { .. } => {
                let pass = 3_400.0_f64.min(0.4 * bw);
                (
                    Demod::Fm(FmDemod::new(rch, NFM_REF_DEVIATION_HZ)),
                    pass,
                    pass + 1_000.0,
                )
            }
            Mode::Am => (Demod::Am(AmDemod::new(rch)), 0.45 * bw, 0.55 * bw),
            Mode::Usb | Mode::Lsb => {
                let shift = ssb_shift.unwrap_or_default();
                let hi = SSB_LOW_CUT_HZ + bw;
                (
                    Demod::Ssb(SsbDemod::new(rch, shift, AgcTimes::SSB)),
                    hi,
                    hi + 800.0,
                )
            }
            Mode::Cw { pitch_hz } => {
                let pitch = f64::from(pitch_hz);
                let pass = pitch + bw / 2.0 + 100.0;
                (
                    Demod::Ssb(SsbDemod::new(rch, pitch, AgcTimes::CW)),
                    pass,
                    pass + 800.0,
                )
            }
        };
        let ma = ((rch / f64::from(OUTPUT_RATE)).floor() as usize).max(1);
        let audio_rate = rch / ma as f64;
        let fc_stop = fc_stop.min(0.45 * audio_rate);
        let audio_taps = design_lowpass(rch, fc_pass.min(fc_stop * 0.9), fc_stop, AUDIO_ATTEN_DB);
        let audio_dec = DecimFir::new(audio_taps.clone(), ma);
        let wfm = matches!(mode, Mode::Wfm { .. }).then(|| WfmPath {
            mpx: StereoMpx::new(rch),
            rds: RdsDecoder::new(rch),
            audio_dec_d: DecimFir::new(audio_taps, ma),
            deemph_d: mode
                .deemphasis()
                .tau_us()
                .map(|t| Deemph::new(audio_rate, t)),
            resampler_r: Resampler::new(audio_rate, f64::from(OUTPUT_RATE)),
            blend: 0.0,
            blend_step: (1.0 / (STEREO_BLEND_S * audio_rate)) as f32,
        });
        let is_fm = matches!(mode, Mode::Wfm { .. } | Mode::Nfm { .. });
        Ok(Self {
            stages,
            pre_shift: ssb_shift.map(|shift| Nco::new(shift, rch)),
            channel_filter,
            channel_rate: rch,
            demod,
            audio_dec,
            dc_block: is_fm.then(|| DcBlock::new(audio_rate, 10.0)),
            deemph: mode
                .deemphasis()
                .tau_us()
                .map(|t| Deemph::new(audio_rate, t)),
            resampler: Resampler::new(audio_rate, f64::from(OUTPUT_RATE)),
            wfm,
        })
    }

    fn reset(&mut self) {
        for s in &mut self.stages {
            s.reset();
        }
        self.channel_filter.reset();
        match &mut self.demod {
            Demod::Fm(d) => d.reset(),
            Demod::Am(d) => d.reset(),
            Demod::Ssb(d) => d.reset(),
        }
        if let Some(n) = &mut self.pre_shift {
            n.reset();
        }
        self.audio_dec.reset();
        if let Some(d) = &mut self.dc_block {
            d.reset();
        }
        if let Some(d) = &mut self.deemph {
            d.reset();
        }
        self.resampler.reset();
        if let Some(w) = &mut self.wfm {
            w.mpx.reset();
            w.rds.reset();
            w.audio_dec_d.reset();
            if let Some(d) = &mut w.deemph_d {
                d.reset();
            }
            w.resampler_r.reset();
            w.blend = 0.0;
        }
    }

    /// Clears the audio filters only (pilot lock and RDS data are kept).
    fn reset_audio(&mut self) {
        self.audio_dec.reset();
        if let Some(d) = &mut self.dc_block {
            d.reset();
        }
        if let Some(d) = &mut self.deemph {
            d.reset();
        }
        self.resampler.reset();
        if let Some(w) = &mut self.wfm {
            w.audio_dec_d.reset();
            if let Some(d) = &mut w.deemph_d {
                d.reset();
            }
            w.resampler_r.reset();
            w.blend = 0.0;
        }
    }
}

/// Largest decimation `<= d0` (within 25 %) whose prime factors are all `<= 5`; `d0` otherwise.
fn pick_decimation(d0: usize) -> usize {
    let lo = (d0 * 3 / 4).max(1);
    (lo..=d0)
        .rev()
        .find(|&d| prime_factors(d).iter().all(|&p| p <= 5))
        .unwrap_or(d0)
}

fn prime_factors(mut n: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut p = 2;
    while p * p <= n {
        while n.is_multiple_of(p) {
            out.push(p);
            n /= p;
        }
        p += 1;
    }
    if n > 1 {
        out.push(n);
    }
    out
}

/// Splits `d` into stage factors (largest first), each at most [`MAX_STAGE_FACTOR`] where the
/// prime factorisation allows.
fn stage_factors(d: usize) -> Vec<usize> {
    let mut primes = prime_factors(d);
    primes.sort_unstable_by(|a, b| b.cmp(a));
    let mut stages = Vec::new();
    let mut cur = 1;
    for p in primes {
        if cur * p <= MAX_STAGE_FACTOR {
            cur *= p;
        } else {
            if cur > 1 {
                stages.push(cur);
            }
            cur = p;
        }
    }
    if cur > 1 {
        stages.push(cur);
    }
    stages.sort_unstable_by(|a, b| b.cmp(a));
    stages
}

#[derive(Debug, Clone, Copy)]
struct Squelch {
    threshold: Option<f32>,
    open: bool,
    hang_left: i64,
    sum: f64,
    count: usize,
}

/// Streaming channelizer + demodulator for one VFO; see the module docs.
pub struct Channel {
    cfg: ChannelConfig,
    nco: Nco,
    chain: Chain,
    squelch: Squelch,
    gate: f32,
    last_power_dbfs: f32,
    stereo: bool,
    buf_a: Vec<Complex32>,
    buf_b: Vec<Complex32>,
    audio_a: Vec<f32>,
    audio_b: Vec<f32>,
    diff: Vec<f32>,
    rds_mix: Vec<Complex32>,
    audio_d: Vec<f32>,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
}

fn validate_offset(offset_hz: f64, rate: u32) -> Result<(), DspError> {
    if offset_hz.is_finite() && offset_hz.abs() < f64::from(rate) / 2.0 {
        Ok(())
    } else {
        Err(DspError::Offset(offset_hz))
    }
}

impl Channel {
    /// Creates a channel; fails on an unsupported rate, offset or bandwidth.
    pub fn new(cfg: ChannelConfig) -> Result<Self, DspError> {
        validate_offset(cfg.offset_hz, cfg.input_rate)?;
        let bw = cfg
            .bandwidth_hz
            .unwrap_or_else(|| cfg.mode.default_bandwidth_hz());
        let chain = Chain::build(cfg.input_rate, cfg.mode, bw)?;
        let open = cfg.squelch_dbfs.is_none();
        Ok(Self {
            nco: Nco::new(cfg.offset_hz, f64::from(cfg.input_rate)),
            squelch: Squelch {
                threshold: cfg.squelch_dbfs,
                open,
                hang_left: 0,
                sum: 0.0,
                count: 0,
            },
            gate: if open { 1.0 } else { 0.0 },
            last_power_dbfs: -200.0,
            chain,
            cfg,
            stereo: false,
            buf_a: Vec::new(),
            buf_b: Vec::new(),
            audio_a: Vec::new(),
            audio_b: Vec::new(),
            diff: Vec::new(),
            rds_mix: Vec::new(),
            audio_d: Vec::new(),
            out_l: Vec::new(),
            out_r: Vec::new(),
        })
    }

    /// Current configuration.
    pub fn config(&self) -> &ChannelConfig {
        &self.cfg
    }

    /// Intermediate channel rate in Hz (after decimation, before audio resampling).
    pub fn channel_rate(&self) -> f64 {
        self.chain.channel_rate
    }

    /// Retunes within the input band; only the NCO phase increment changes (no state reset).
    pub fn set_offset(&mut self, offset_hz: f64) -> Result<(), DspError> {
        validate_offset(offset_hz, self.cfg.input_rate)?;
        if (offset_hz - self.cfg.offset_hz).abs() > STATION_CHANGE_HZ
            && let Some(w) = &mut self.chain.wfm
        {
            // A different station: its pilot and RDS data are not those of the old one.
            w.mpx.reset();
            w.rds.reset();
            w.blend = 0.0;
        }
        self.cfg.offset_hz = offset_hz;
        self.nco
            .set_offset(offset_hz, f64::from(self.cfg.input_rate));
        Ok(())
    }

    /// Enables or disables stereo output (WFM only; ignored by the other modes). Takes effect
    /// through [`Self::output_channels`]; toggling clears the audio filters (a short gap) but
    /// keeps the pilot lock and the RDS data.
    pub fn set_stereo(&mut self, stereo: bool) {
        if self.stereo != stereo {
            self.stereo = stereo;
            self.chain.reset_audio();
            self.gate = if self.squelch.open { 1.0 } else { 0.0 };
        }
    }

    /// Channels per output frame of [`Self::process`]: 2 (interleaved left/right) for WFM with
    /// stereo enabled, otherwise 1.
    #[must_use]
    pub fn output_channels(&self) -> usize {
        if self.stereo && self.chain.wfm.is_some() {
            2
        } else {
            1
        }
    }

    /// True while a stereo pilot is locked (WFM); the matrix is then (fading towards) stereo.
    #[must_use]
    pub fn stereo_locked(&self) -> bool {
        self.chain.wfm.as_ref().is_some_and(|w| w.mpx.locked())
    }

    /// RDS data decoded so far (WFM only).
    #[must_use]
    pub fn rds_info(&self) -> Option<&RdsInfo> {
        self.chain.wfm.as_ref().map(|w| w.rds.info())
    }

    /// True while the RDS block synchronisation holds.
    #[must_use]
    pub fn rds_synced(&self) -> bool {
        self.chain.wfm.as_ref().is_some_and(|w| w.rds.synced())
    }

    /// Counter that increases whenever [`Self::rds_info`] changes.
    #[must_use]
    pub fn rds_revision(&self) -> u64 {
        self.chain.wfm.as_ref().map_or(0, |w| w.rds.revision())
    }

    fn rebuild(&mut self, cfg: ChannelConfig) -> Result<(), DspError> {
        validate_offset(cfg.offset_hz, cfg.input_rate)?;
        let bw = cfg
            .bandwidth_hz
            .unwrap_or_else(|| cfg.mode.default_bandwidth_hz());
        self.chain = Chain::build(cfg.input_rate, cfg.mode, bw)?;
        self.nco
            .set_offset(cfg.offset_hz, f64::from(cfg.input_rate));
        self.cfg = cfg;
        self.reset_squelch_timing();
        Ok(())
    }

    fn reset_squelch_timing(&mut self) {
        self.squelch.sum = 0.0;
        self.squelch.count = 0;
    }

    /// Switches the mode (rebuilds the filter chain; the bandwidth override is kept).
    pub fn set_mode(&mut self, mode: Mode) -> Result<(), DspError> {
        self.rebuild(ChannelConfig { mode, ..self.cfg })
    }

    /// Sets the channel bandwidth (`None` = mode default); rebuilds the filter chain.
    pub fn set_bandwidth(&mut self, bandwidth_hz: Option<u32>) -> Result<(), DspError> {
        self.rebuild(ChannelConfig {
            bandwidth_hz,
            ..self.cfg
        })
    }

    /// Changes the input rate (e.g. after a device reconfiguration); rebuilds the filter chain.
    pub fn set_input_rate(&mut self, input_rate: u32) -> Result<(), DspError> {
        self.rebuild(ChannelConfig {
            input_rate,
            ..self.cfg
        })
    }

    /// Sets the squelch threshold in dBFS; `None` disables the squelch (always open).
    pub fn set_squelch(&mut self, squelch_dbfs: Option<f32>) {
        self.cfg.squelch_dbfs = squelch_dbfs;
        self.squelch.threshold = squelch_dbfs;
        self.squelch.hang_left = 0;
        self.reset_squelch_timing();
        self.squelch.open = squelch_dbfs.is_none();
    }

    /// Clears all filter, demodulator and resampler state (call on retune / tuning epoch change).
    pub fn reset(&mut self) {
        self.chain.reset();
        self.nco.reset();
        self.squelch.open = self.squelch.threshold.is_none();
        self.squelch.hang_left = 0;
        self.reset_squelch_timing();
        self.gate = if self.squelch.open { 1.0 } else { 0.0 };
        self.last_power_dbfs = -200.0;
    }

    fn squelch_update(&mut self, sum: f64, count: usize) {
        let sq = &mut self.squelch;
        sq.sum += sum;
        sq.count += count;
        let window = ((self.chain.channel_rate * SQUELCH_WINDOW_S) as usize).max(1);
        if sq.count < window {
            return;
        }
        let db = power_to_db((sq.sum / sq.count as f64) as f32);
        let n = sq.count as i64;
        sq.sum = 0.0;
        sq.count = 0;
        let Some(thr) = sq.threshold else {
            sq.open = true;
            return;
        };
        let hang = (self.chain.channel_rate * SQUELCH_HANG_S) as i64;
        if !sq.open {
            if db >= thr {
                sq.open = true;
                sq.hang_left = hang;
            }
        } else if db >= thr - SQUELCH_HYSTERESIS_DB {
            sq.hang_left = hang;
        } else {
            sq.hang_left -= n;
            if sq.hang_left <= 0 {
                sq.open = false;
            }
        }
    }

    /// Processes a block of IQ, appending 48 kHz audio to `out`: mono samples, or interleaved
    /// left/right frames if [`Self::output_channels`] is 2.
    ///
    /// Blocks may have any length; the long-term output count is exactly
    /// `48000 / input_rate` frames per input sample (plus a fixed filter delay). While the
    /// squelch is closed the output is silence of the same length.
    pub fn process(&mut self, iq: &[Complex32], out: &mut Vec<f32>) -> ChannelStats {
        let mut power_sum = 0.0f64;
        let mut power_n = 0usize;
        for chunk in iq.chunks(CHUNK) {
            self.nco.mix(chunk, &mut self.buf_a);
            for st in &mut self.chain.stages {
                self.buf_b.clear();
                st.process(&self.buf_a, &mut self.buf_b);
                std::mem::swap(&mut self.buf_a, &mut self.buf_b);
            }
            if let Some(n) = &mut self.chain.pre_shift {
                n.mix(&self.buf_a, &mut self.buf_b);
                std::mem::swap(&mut self.buf_a, &mut self.buf_b);
            }
            self.buf_b.clear();
            self.chain
                .channel_filter
                .process(&self.buf_a, &mut self.buf_b);
            std::mem::swap(&mut self.buf_a, &mut self.buf_b);

            let e: f64 = self
                .buf_a
                .chunks(256)
                .map(|c| f64::from(c.iter().map(|x| x.norm_sqr()).sum::<f32>()))
                .sum();
            power_sum += e;
            power_n += self.buf_a.len();
            self.squelch_update(e, self.buf_a.len());

            self.audio_a.clear();
            match &mut self.chain.demod {
                Demod::Fm(d) => d.process(&self.buf_a, &mut self.audio_a),
                Demod::Am(d) => d.process(&self.buf_a, &mut self.audio_a),
                Demod::Ssb(d) => d.process(&self.buf_a, &mut self.audio_a),
            }
            let stereo = self.output_channels() == 2;
            if let Some(w) = &mut self.chain.wfm {
                self.diff.clear();
                self.rds_mix.clear();
                w.mpx
                    .process(&self.audio_a, stereo, &mut self.diff, &mut self.rds_mix);
                w.rds.process(&self.rds_mix);
            }
            self.audio_b.clear();
            self.chain
                .audio_dec
                .process(&self.audio_a, &mut self.audio_b);
            if let Some(d) = &mut self.chain.dc_block {
                d.process_block(&mut self.audio_b);
            }
            if let Some(d) = &mut self.chain.deemph {
                d.process_block(&mut self.audio_b);
            }
            let start = out.len();
            match &mut self.chain.wfm {
                Some(w) if stereo => {
                    self.audio_d.clear();
                    w.audio_dec_d.process(&self.diff, &mut self.audio_d);
                    if let Some(d) = &mut w.deemph_d {
                        d.process_block(&mut self.audio_d);
                    }
                    let n = self.audio_b.len().min(self.audio_d.len());
                    let target = if w.mpx.locked() { 1.0 } else { 0.0 };
                    for (s, d) in self.audio_b[..n].iter_mut().zip(&mut self.audio_d[..n]) {
                        w.blend += (target - w.blend).clamp(-w.blend_step, w.blend_step);
                        let side = w.blend * *d;
                        *d = *s - side;
                        *s += side;
                    }
                    self.out_l.clear();
                    self.out_r.clear();
                    self.chain
                        .resampler
                        .process(&self.audio_b[..n], &mut self.out_l);
                    w.resampler_r.process(&self.audio_d[..n], &mut self.out_r);
                    for (l, r) in self.out_l.iter().zip(&self.out_r) {
                        out.push(*l);
                        out.push(*r);
                    }
                }
                _ => self.chain.resampler.process(&self.audio_b, out),
            }

            let target = if self.squelch.open { 1.0 } else { 0.0 };
            if self.gate != target || target == 0.0 {
                let step = (1.0 / (GATE_RAMP_S * f64::from(OUTPUT_RATE))) as f32;
                for frame in out[start..].chunks_mut(if stereo { 2 } else { 1 }) {
                    self.gate += (target - self.gate).clamp(-step, step);
                    for y in frame {
                        *y *= self.gate;
                    }
                }
            }
        }
        if power_n > 0 {
            self.last_power_dbfs = power_to_db((power_sum / power_n as f64) as f32);
        }
        ChannelStats {
            power_dbfs: self.last_power_dbfs,
            squelch_open: self.squelch.open,
        }
    }
}

fn power_to_db(p: f32) -> f32 {
    10.0 * p.max(POWER_FLOOR).log10()
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;

    /// Complex tone / FM carrier at `offset` with amplitude `amp`; `dev`/`fm` add a sine-modulated
    /// deviation (`dev == 0` is an unmodulated carrier).
    fn fm_sample(rate: u32, n: usize, offset: f64, amp: f64, dev: f64, fm: f64) -> Complex32 {
        let t = n as f64 / f64::from(rate);
        let ph = TAU * offset * t
            - if dev != 0.0 {
                dev / fm * (TAU * fm * t).cos()
            } else {
                0.0
            };
        Complex32::new((amp * ph.cos()) as f32, (amp * ph.sin()) as f32)
    }

    /// Feeds samples `n0..n1` of `src` in awkward block sizes.
    fn run(
        ch: &mut Channel,
        src: &dyn Fn(usize) -> Complex32,
        n0: usize,
        n1: usize,
        out: &mut Vec<f32>,
    ) -> ChannelStats {
        const SIZES: [usize; 9] = [4093, 1, 777, 16384, 5000, 3, 12345, 2048, 31];
        let mut stats = ChannelStats {
            power_dbfs: -200.0,
            squelch_open: false,
        };
        let (mut n, mut k) = (n0, 0);
        while n < n1 {
            let len = SIZES[k % SIZES.len()].min(n1 - n);
            k += 1;
            let block: Vec<Complex32> = (n..n + len).map(src).collect();
            stats = ch.process(&block, out);
            n += len;
        }
        stats
    }

    /// Least-squares sine fit at `f_hz`: returns (amplitude, SNR in dB of everything else).
    fn fit(audio: &[f32], f_hz: f64) -> (f64, f64) {
        let rate = f64::from(OUTPUT_RATE);
        let cycle = (rate / f_hz).round() as usize; // f_hz divides 48 kHz in these tests
        let len = audio.len() / cycle * cycle;
        let seg = &audio[audio.len() - len..];
        let (mut a, mut b, mut p) = (0.0, 0.0, 0.0);
        for (i, &v) in seg.iter().enumerate() {
            let ph = TAU * f_hz * i as f64 / rate;
            a += f64::from(v) * ph.cos();
            b += f64::from(v) * ph.sin();
            p += f64::from(v) * f64::from(v);
        }
        let n = seg.len() as f64;
        let amp = 2.0 * a.hypot(b) / n;
        let tone = amp * amp / 2.0;
        let noise = (p / n - tone).max(1e-18);
        (amp, 10.0 * (tone / noise).log10())
    }

    fn cfg(rate: u32, offset: f64, mode: Mode) -> ChannelConfig {
        ChannelConfig {
            input_rate: rate,
            offset_hz: offset,
            mode,
            bandwidth_hz: None,
            squelch_dbfs: None,
        }
    }

    fn demod_tone(rate: u32, offset: f64, mode: Mode, dev: f64, secs: f64) -> (f64, f64) {
        let mut ch = Channel::new(cfg(rate, offset, mode)).expect("valid config");
        let n = (f64::from(rate) * secs) as usize;
        let src = move |i| fm_sample(rate, i, offset, 0.5, dev, 1000.0);
        let mut out = Vec::new();
        run(&mut ch, &src, 0, n, &mut out);
        let skip = out.len() / 3;
        fit(&out[skip..], 1000.0)
    }

    #[test]
    fn nfm_recovers_tone() {
        for (rate, offset) in [
            (2_400_000, 200_000.0),
            (2_048_000, -310_000.0),
            (250_000, 15_000.0),
        ] {
            let (amp, snr) = demod_tone(rate, offset, Mode::nfm(), 3000.0, 0.6);
            assert!(snr > 30.0, "rate {rate}: snr {snr}");
            assert!((amp - 0.6).abs() < 0.05, "rate {rate}: amp {amp}");
        }
    }

    /// Hann-windowed amplitude of the component at `f_hz` (any frequency, sine amplitude).
    fn amp_at(audio: &[f32], f_hz: f64) -> f64 {
        let rate = f64::from(OUTPUT_RATE);
        let n = audio.len();
        let (mut a, mut b, mut w_sum) = (0.0, 0.0, 0.0);
        for (i, &v) in audio.iter().enumerate() {
            let w = 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos();
            let ph = TAU * f_hz * i as f64 / rate;
            a += w * f64::from(v) * ph.cos();
            b += w * f64::from(v) * ph.sin();
            w_sum += w;
        }
        2.0 * a.hypot(b) / w_sum
    }

    /// Sum of complex tones `(offset_from_vfo_hz, amplitude)` around the VFO at `vfo_offset`.
    fn tones(
        rate: u32,
        vfo_offset: f64,
        parts: &'static [(f64, f64)],
    ) -> impl Fn(usize) -> Complex32 {
        move |i| {
            let t = i as f64 / f64::from(rate);
            let mut acc = Complex32::new(0.0, 0.0);
            for &(df, amp) in parts {
                let ph = TAU * (vfo_offset + df) * t;
                acc += Complex32::new((amp * ph.cos()) as f32, (amp * ph.sin()) as f32);
            }
            acc
        }
    }

    fn demod_tones(
        rate: u32,
        vfo_offset: f64,
        mode: Mode,
        bandwidth_hz: Option<u32>,
        parts: &'static [(f64, f64)],
    ) -> Vec<f32> {
        let mut c = cfg(rate, vfo_offset, mode);
        c.bandwidth_hz = bandwidth_hz;
        let mut ch = Channel::new(c).expect("valid config");
        let n = (f64::from(rate) * 0.8) as usize;
        let mut out = Vec::new();
        run(&mut ch, &tones(rate, vfo_offset, parts), 0, n, &mut out);
        out.split_off(out.len() / 2)
    }

    fn db(ratio: f64) -> f64 {
        20.0 * ratio.log10()
    }

    #[test]
    fn usb_passes_upper_and_rejects_lower_sideband() {
        for (rate, offset) in [(2_400_000, 150_000.0), (250_000, -20_000.0)] {
            // Wanted +1 kHz; the unwanted tone sits in the mirrored sideband.
            let audio = demod_tones(
                rate,
                offset,
                Mode::Usb,
                None,
                &[(1000.0, 0.3), (-1500.0, 0.3)],
            );
            let wanted = amp_at(&audio, 1000.0);
            let image = amp_at(&audio, 1500.0);
            assert!(
                (0.5..0.95).contains(&wanted),
                "rate {rate}: wanted {wanted}"
            );
            assert!(db(wanted / image) > 40.0, "rate {rate}: image {image}");
        }
    }

    #[test]
    fn lsb_passes_lower_and_rejects_upper_sideband() {
        for (rate, offset) in [(2_048_000, -100_000.0), (250_000, 0.0)] {
            let audio = demod_tones(
                rate,
                offset,
                Mode::Lsb,
                None,
                &[(-1000.0, 0.3), (1500.0, 0.3)],
            );
            let wanted = amp_at(&audio, 1000.0);
            let image = amp_at(&audio, 1500.0);
            assert!(
                (0.5..0.95).contains(&wanted),
                "rate {rate}: wanted {wanted}"
            );
            assert!(db(wanted / image) > 40.0, "rate {rate}: image {image}");
        }
        // The same signal is heard in USB at +1 kHz and suppressed in LSB.
        let both: &'static [(f64, f64)] = &[(1000.0, 0.3), (-1500.0, 0.3)];
        let usb = demod_tones(250_000, 0.0, Mode::Usb, None, both);
        let lsb = demod_tones(250_000, 0.0, Mode::Lsb, None, both);
        assert!(amp_at(&lsb, 1500.0) > 0.5);
        assert!(amp_at(&usb, 1000.0) > 0.5);
        assert!(db(amp_at(&usb, 1000.0) / amp_at(&lsb, 1000.0)) > 40.0);
    }

    #[test]
    fn ssb_passband_edges_follow_the_bandwidth() {
        let parts: &'static [(f64, f64)] = &[(1000.0, 0.3), (2000.0, 0.3), (3200.0, 0.3)];
        // 1700 Hz wide passband: 300 Hz ..= 2000 Hz.
        let audio = demod_tones(250_000, 0.0, Mode::Usb, Some(1700), parts);
        assert!(amp_at(&audio, 1000.0) > 0.3);
        assert!(db(amp_at(&audio, 1000.0) / amp_at(&audio, 3200.0)) > 40.0);
        // The default 2400 Hz (300 Hz ..= 2700 Hz) passes 2 kHz and rejects 3.2 kHz.
        let audio = demod_tones(250_000, 0.0, Mode::Usb, None, parts);
        assert!(amp_at(&audio, 2000.0) > 0.2);
        assert!(db(amp_at(&audio, 1000.0) / amp_at(&audio, 3200.0)) > 40.0);
    }

    #[test]
    fn ssb_agc_levels_weak_and_strong_signals() {
        let weak = demod_tones(250_000, 0.0, Mode::Usb, None, &[(1000.0, 0.005)]);
        let strong = demod_tones(250_000, 0.0, Mode::Usb, None, &[(1000.0, 0.4)]);
        let (w, s) = (amp_at(&weak, 1000.0), amp_at(&strong, 1000.0));
        assert!(
            (0.4..1.0).contains(&w) && (0.4..1.0).contains(&s),
            "{w} {s}"
        );
        assert!(strong.iter().all(|v| v.abs() <= 1.0));
    }

    #[test]
    fn cw_carrier_on_vfo_gives_pitch_tone() {
        let carrier: &'static [(f64, f64)] = &[(0.0, 0.3)];
        let audio = demod_tones(250_000, 20_000.0, Mode::cw(), None, carrier);
        let tone = amp_at(&audio, 700.0);
        assert!((0.5..0.95).contains(&tone), "tone {tone}");
        assert!(db(tone / amp_at(&audio, 1400.0).max(1e-9)) > 30.0);

        // Configurable pitch.
        let audio = demod_tones(
            2_400_000,
            -300_000.0,
            Mode::Cw { pitch_hz: 600 },
            None,
            carrier,
        );
        assert!(amp_at(&audio, 600.0) > 0.5);
        assert!(amp_at(&audio, 700.0) < 0.05);
    }

    #[test]
    fn cw_filter_is_narrow_and_follows_the_offset() {
        // A carrier 100 Hz above the VFO lands 100 Hz above the pitch.
        let audio = demod_tones(250_000, 0.0, Mode::cw(), None, &[(100.0, 0.3)]);
        assert!(amp_at(&audio, 800.0) > 0.5);
        // 1 kHz away is outside the 500 Hz filter; the carrier on the VFO dominates.
        let audio = demod_tones(250_000, 0.0, Mode::cw(), None, &[(0.0, 0.3), (1000.0, 0.3)]);
        assert!(db(amp_at(&audio, 700.0) / amp_at(&audio, 1700.0)) > 40.0);
        // A 100 Hz wide filter is realisable and still passes the carrier.
        let audio = demod_tones(250_000, 0.0, Mode::cw(), Some(100), &[(0.0, 0.3)]);
        assert!(amp_at(&audio, 700.0) > 0.5);
    }

    #[test]
    fn ssb_and_cw_parameters_are_validated() {
        assert!(matches!(
            Channel::new(ChannelConfig {
                bandwidth_hz: Some(200),
                ..cfg(250_000, 0.0, Mode::Usb)
            }),
            Err(DspError::Bandwidth(200))
        ));
        assert!(matches!(
            Channel::new(cfg(250_000, 0.0, Mode::Cw { pitch_hz: 5000 })),
            Err(DspError::Pitch(5000))
        ));
        assert!(matches!(
            Channel::new(ChannelConfig {
                bandwidth_hz: Some(50),
                ..cfg(250_000, 0.0, Mode::cw())
            }),
            Err(DspError::Bandwidth(50))
        ));
        assert_eq!(Mode::Usb.default_bandwidth_hz(), 2_400);
        assert_eq!(Mode::cw().default_bandwidth_hz(), 500);
    }

    #[test]
    fn wfm_recovers_tone() {
        for (rate, offset) in [
            (2_400_000, -250_000.0),
            (2_048_000, 100_000.0),
            (250_000, 0.0),
        ] {
            let (amp, snr) = demod_tone(rate, offset, Mode::wfm(), 75_000.0, 0.6);
            assert!(snr > 30.0, "rate {rate}: snr {snr}");
            // 50 us de-emphasis attenuates 1 kHz by 0.4 dB.
            assert!((0.9..1.05).contains(&amp), "rate {rate}: amp {amp}");
        }
    }

    #[test]
    fn am_recovers_tone() {
        for (rate, offset) in [(2_400_000, -100_000.0), (2_048_000, 50_000.0)] {
            let mut ch = Channel::new(cfg(rate, offset, Mode::Am)).expect("valid config");
            let n = (f64::from(rate) * 0.6) as usize;
            let src = move |i: usize| {
                let t = i as f64 / f64::from(rate);
                let env = 0.3 * (1.0 + 0.5 * (TAU * 1000.0 * t).sin());
                let ph = TAU * offset * t;
                Complex32::new((env * ph.cos()) as f32, (env * ph.sin()) as f32)
            };
            let mut out = Vec::new();
            run(&mut ch, &src, 0, n, &mut out);
            let (amp, snr) = fit(&out[out.len() / 3..], 1000.0);
            assert!(snr > 30.0, "rate {rate}: snr {snr}");
            assert!(amp > 0.1 && amp < 0.6, "rate {rate}: amp {amp}");
        }
    }

    #[test]
    fn am_agc_levels_weak_and_strong_carriers() {
        let rate = 250_000;
        let mut amps = Vec::new();
        for level in [0.01, 0.5] {
            let mut ch = Channel::new(cfg(rate, 0.0, Mode::Am)).expect("valid config");
            let src = move |i: usize| {
                let t = i as f64 / f64::from(rate);
                Complex32::new((level * (1.0 + 0.5 * (TAU * 1000.0 * t).sin())) as f32, 0.0)
            };
            let mut out = Vec::new();
            run(&mut ch, &src, 0, rate as usize, &mut out);
            amps.push(fit(&out[out.len() / 2..], 1000.0).0);
        }
        assert!((amps[0] / amps[1] - 1.0).abs() < 0.15, "{amps:?}");
    }

    fn channel_power(mut ch: Channel, rate: u32, offset: f64, amp: f64) -> f32 {
        let src = move |i| fm_sample(rate, i, offset, amp, 0.0, 1.0);
        let n = (f64::from(rate) * 0.2) as usize;
        run(&mut ch, &src, 0, n, &mut Vec::new()).power_dbfs
    }

    #[test]
    fn off_channel_interferers_are_rejected() {
        let cases: [(u32, Mode, &[f64]); 3] = [
            (
                2_400_000,
                Mode::nfm(),
                &[20_000.0, 30_000.0, -100_000.0, 500_000.0, 1_100_000.0],
            ),
            (2_400_000, Mode::Am, &[20_000.0, -60_000.0, 700_000.0]),
            (2_048_000, Mode::wfm(), &[-300_000.0, 500_000.0, 900_000.0]),
        ];
        for (rate, mode, freqs) in cases {
            let wanted = channel_power(
                Channel::new(cfg(rate, 0.0, mode)).expect("cfg"),
                rate,
                1_500.0,
                0.5,
            );
            assert!((wanted + 6.0).abs() < 0.5, "{mode:?}: in-channel {wanted}");
            for &f in freqs {
                let p = channel_power(
                    Channel::new(cfg(rate, 0.0, mode)).expect("cfg"),
                    rate,
                    f,
                    0.5,
                );
                assert!(wanted - p >= 50.0, "{mode:?} @ {f}: only {} dB", wanted - p);
            }
        }
    }

    #[test]
    fn output_count_has_no_drift() {
        let cases = [
            (2_048_000, Mode::nfm()),
            (2_048_000, Mode::wfm()),
            (2_400_000, Mode::Am),
            (250_000, Mode::wfm()),
            (250_000, Mode::nfm()),
            (3_200_000, Mode::nfm()),
        ];
        for (rate, mode) in cases {
            let mut ch = Channel::new(cfg(rate, 10_000.0, mode)).expect("valid config");
            let src = |_| Complex32::new(0.01, 0.0);
            let mut out = Vec::new();
            let r = rate as usize;
            run(&mut ch, &src, 0, r / 2, &mut out);
            let c1 = out.len();
            run(&mut ch, &src, r / 2, 2 * r, &mut out);
            let c2 = out.len();
            assert!(
                (c2 as i64 - c1 as i64 - 72_000).abs() <= 2,
                "{rate} {mode:?}: drift {} vs 72000",
                c2 - c1
            );
            assert!(
                (c2 as i64 - 2 * 48_000).abs() < 400,
                "{rate} {mode:?}: total {c2}"
            );
        }
    }

    #[test]
    fn squelch_opens_closes_with_hysteresis() {
        let rate = 250_000;
        let thr = -40.0;
        let mut c = cfg(rate, 0.0, Mode::nfm());
        c.squelch_dbfs = Some(thr);
        let mut ch = Channel::new(c).expect("valid config");
        let per = (f64::from(rate) * 0.5) as usize;
        let levels_db = [-60.0, -30.0, -41.5, -50.0, -41.5, -30.0];
        let expect_open = [false, true, true, false, false, true];
        let src = move |i: usize| {
            let a = 10f64.powf(levels_db[(i / per).min(levels_db.len() - 1)] / 20.0);
            fm_sample(rate, i, 0.0, a, 3000.0, 1000.0)
        };
        for (k, &want) in expect_open.iter().enumerate() {
            let mut out = Vec::new();
            let stats = run(&mut ch, &src, k * per, (k + 1) * per, &mut out);
            assert_eq!(stats.squelch_open, want, "phase {k} ({} dB)", levels_db[k]);
            let tail = &out[out.len() - 4800..];
            let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            if want {
                assert!(peak > 0.3, "phase {k}: peak {peak}");
            } else {
                assert_eq!(peak, 0.0, "phase {k}: not silent");
            }
        }
    }

    #[test]
    fn offset_change_retunes_midstream() {
        let rate = 2_400_000;
        let (fa, fb) = (100_000.0, -150_000.0);
        let src = move |i: usize| {
            fm_sample(rate, i, fa, 0.5, 3000.0, 1000.0) + fm_sample(rate, i, fb, 0.5, 3000.0, 750.0)
        };
        let seg = (f64::from(rate) * 0.3) as usize;
        let mut ch = Channel::new(cfg(rate, fa, Mode::nfm())).expect("valid config");
        let mut out = Vec::new();
        let s1 = run(&mut ch, &src, 0, seg, &mut out);
        let (amp, snr) = fit(&out[out.len() / 2..], 1000.0);
        assert!(
            snr > 30.0 && (amp - 0.6).abs() < 0.06,
            "A: amp {amp} snr {snr}"
        );
        assert!(
            (s1.power_dbfs + 6.0).abs() < 1.0,
            "A power {}",
            s1.power_dbfs
        );

        ch.set_offset(fb).expect("in band");
        out.clear();
        let s2 = run(&mut ch, &src, seg, 2 * seg, &mut out);
        let (amp, snr) = fit(&out[out.len() / 2..], 750.0);
        assert!(
            snr > 30.0 && (amp - 0.6).abs() < 0.06,
            "B: amp {amp} snr {snr}"
        );
        assert!(
            (s2.power_dbfs + 6.0).abs() < 1.0,
            "B power {}",
            s2.power_dbfs
        );

        ch.set_offset(0.0).expect("in band");
        let s3 = run(&mut ch, &src, 2 * seg, 3 * seg, &mut Vec::new());
        assert!(s3.power_dbfs < -50.0, "empty power {}", s3.power_dbfs);
        assert_eq!(
            ch.set_offset(1_200_000.0),
            Err(DspError::Offset(1_200_000.0))
        );
        assert_eq!(ch.config().offset_hz, 0.0);
    }

    #[test]
    fn reconfiguration_and_errors() {
        let mut ch = Channel::new(cfg(2_400_000, 0.0, Mode::nfm())).expect("valid config");
        assert!(ch.channel_rate() >= 31_250.0);
        ch.set_mode(Mode::wfm()).expect("wfm");
        assert!(ch.channel_rate() >= 240_000.0);
        ch.set_bandwidth(Some(100_000)).expect("bw");
        ch.set_input_rate(2_048_000).expect("rate");
        assert_eq!(
            ch.set_bandwidth(Some(2_000_000)),
            Err(DspError::Bandwidth(2_000_000))
        );
        assert_eq!(ch.config().bandwidth_hz, Some(100_000));
        assert!(matches!(
            Channel::new(cfg(50_000, 0.0, Mode::Am)),
            Err(DspError::InputRate(_))
        ));
        assert!(matches!(
            Channel::new(cfg(2_400_000, 1.3e6, Mode::Am)),
            Err(DspError::Offset(_))
        ));
        let mut out = Vec::new();
        ch.process(&[], &mut out);
        assert!(out.is_empty());
        ch.reset();
    }

    #[test]
    fn decimation_planning() {
        assert_eq!(pick_decimation(76), 75);
        assert_eq!(pick_decimation(10), 10);
        assert_eq!(pick_decimation(7), 6);
        assert_eq!(stage_factors(10), vec![5, 2]);
        assert_eq!(stage_factors(75), vec![5, 5, 3]);
        assert_eq!(stage_factors(8), vec![4, 2]);
        assert!(stage_factors(1).is_empty());
    }

    /// Release-mode throughput: `cargo test -p superhet-dsp --release -- --ignored --nocapture`.
    #[test]
    #[ignore = "benchmark; run with --release --ignored --nocapture"]
    fn throughput() {
        let rate = 2_400_000u32;
        let secs = 10usize;
        let mut seed = 0x1234_5678u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let block: Vec<Complex32> = (0..65_536)
            .map(|_| Complex32::new(noise(), noise()))
            .collect();
        for (name, mode) in [("WFM", Mode::wfm()), ("NFM", Mode::nfm()), ("AM", Mode::Am)] {
            let mut ch = Channel::new(cfg(rate, 200_000.0, mode)).expect("valid config");
            let mut out = Vec::new();
            let blocks = rate as usize * secs / block.len();
            let t0 = std::time::Instant::now();
            for _ in 0..blocks {
                out.clear();
                ch.process(&block, &mut out);
            }
            let wall = t0.elapsed().as_secs_f64();
            let sig = (blocks * block.len()) as f64 / f64::from(rate);
            println!(
                "{name}: {:.1} ms CPU per s of signal = {:.2} % of one core ({:.0}x realtime)",
                wall / sig * 1000.0,
                wall / sig * 100.0,
                sig / wall
            );
        }
    }
}
