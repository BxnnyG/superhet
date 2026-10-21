//! Device-level VFOs: model, request parsing/validation, registry and state encoding.
//!
//! A VFO is a demodulated channel at an absolute frequency inside the device's passband. The
//! daemon assigns ids (monotonically, never reused while it runs). Requests arrive on
//! `sh.v1.dev.<dev>.ctl.vfo.{create,update,delete}`; the registry is shared with the audio engine,
//! which follows it through [`VfoShared::generation`]. Everything here is pure and testable
//! without a bus or a device.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Deserializer, Serialize};
use superhet_bus::profile;
use superhet_dsp::{Channel, ChannelConfig, Deemphasis, Mode};

use crate::control::{CtlError, ErrorCode, MAX_CENTER_HZ, VERSION};
use crate::window;

/// Most VFOs one device can run.
pub const MAX_VFOS: usize = 8;
/// Accepted channel bandwidth in Hz (the channelizer applies its own, tighter limits).
const BANDWIDTH_RANGE: std::ops::RangeInclusive<u32> = 1_000..=1_000_000;
/// Accepted squelch threshold in dBFS.
const SQUELCH_RANGE: std::ops::RangeInclusive<f32> = -200.0..=0.0;

/// Demodulation mode as spelled on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VfoMode {
    /// Broadcast FM.
    Wfm,
    /// Narrow-band FM.
    Nfm,
    /// Envelope AM.
    Am,
}

/// De-emphasis as spelled on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeemphasisWire {
    /// No de-emphasis.
    #[serde(rename = "none")]
    None,
    /// 50 us.
    #[serde(rename = "50us")]
    Us50,
    /// 75 us.
    #[serde(rename = "75us")]
    Us75,
}

impl From<DeemphasisWire> for Deemphasis {
    fn from(w: DeemphasisWire) -> Self {
        match w {
            DeemphasisWire::None => Self::None,
            DeemphasisWire::Us50 => Self::Us50,
            DeemphasisWire::Us75 => Self::Us75,
        }
    }
}

impl From<Deemphasis> for DeemphasisWire {
    fn from(d: Deemphasis) -> Self {
        match d {
            Deemphasis::None => Self::None,
            Deemphasis::Us50 => Self::Us50,
            Deemphasis::Us75 => Self::Us75,
        }
    }
}

/// A VFO.
#[derive(Debug, Clone, PartialEq)]
pub struct Vfo {
    /// Id assigned by the daemon.
    pub id: u32,
    /// Absolute frequency in Hz.
    pub freq_hz: u64,
    /// Demodulation mode.
    pub mode: VfoMode,
    /// Bandwidth override; `None` uses the mode default.
    pub bandwidth_hz: Option<u32>,
    /// Squelch threshold in dBFS; `None` = off.
    pub squelch_dbfs: Option<f32>,
    /// De-emphasis override (WFM/NFM only); `None` uses the mode default.
    pub deemphasis: Option<Deemphasis>,
}

/// Tuning of the device a VFO is evaluated against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tuning {
    /// Centre frequency in Hz.
    pub center_hz: u64,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

/// Whether a VFO is inside the current passband.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Demodulating.
    Active,
    /// Outside the passband (or not realisable); no audio, no DSP.
    Paused,
}

impl Vfo {
    /// The DSP mode including the de-emphasis choice.
    #[must_use]
    pub fn dsp_mode(&self) -> Mode {
        match self.mode {
            VfoMode::Wfm => Mode::Wfm {
                deemphasis: self.deemphasis.unwrap_or(Deemphasis::Us50),
            },
            VfoMode::Nfm => Mode::Nfm {
                deemphasis: self.deemphasis.unwrap_or(Deemphasis::None),
            },
            VfoMode::Am => Mode::Am,
        }
    }

    /// Effective channel bandwidth in Hz.
    #[must_use]
    pub fn bandwidth(&self) -> u32 {
        self.bandwidth_hz
            .unwrap_or_else(|| self.dsp_mode().default_bandwidth_hz())
    }

    /// Frequency offset from the device centre in Hz.
    #[must_use]
    pub fn offset_hz(&self, center_hz: u64) -> f64 {
        (i128::from(self.freq_hz) - i128::from(center_hz)) as f64
    }

    /// Paused if any part of the channel falls outside the usable window (85 % of the sample
    /// rate around the centre, see [`window`]).
    #[must_use]
    pub fn status(&self, tuning: Tuning) -> Status {
        if window::fits(self, tuning) {
            Status::Active
        } else {
            Status::Paused
        }
    }

    /// Channelizer configuration for an active VFO.
    #[must_use]
    pub fn channel_config(&self, tuning: Tuning) -> ChannelConfig {
        ChannelConfig {
            input_rate: tuning.sample_rate,
            offset_hz: self.offset_hz(tuning.center_hz),
            mode: self.dsp_mode(),
            bandwidth_hz: self.bandwidth_hz,
            squelch_dbfs: self.squelch_dbfs,
        }
    }

    /// Encode for the state document and replies.
    #[must_use]
    pub fn wire(&self, tuning: Tuning) -> VfoWire {
        let deemphasis = match self.dsp_mode() {
            Mode::Wfm { deemphasis } | Mode::Nfm { deemphasis } => Some(deemphasis.into()),
            Mode::Am => None,
        };
        VfoWire {
            id: self.id,
            freq_hz: self.freq_hz,
            mode: self.mode,
            bandwidth_hz: self.bandwidth(),
            squelch_dbfs: self.squelch_dbfs,
            deemphasis,
            offset_hz: self.offset_hz(tuning.center_hz),
            status: self.status(tuning),
        }
    }
}

/// A VFO as published in the state document.
#[derive(Debug, Serialize)]
pub struct VfoWire {
    id: u32,
    freq_hz: u64,
    mode: VfoMode,
    bandwidth_hz: u32,
    squelch_dbfs: Option<f32>,
    deemphasis: Option<DeemphasisWire>,
    offset_hz: f64,
    status: Status,
}

/// Validated `vfo.create` request.
#[derive(Debug, Clone, PartialEq)]
pub struct VfoCreate {
    /// Absolute frequency in Hz.
    pub freq_hz: u64,
    /// Mode.
    pub mode: VfoMode,
    /// Bandwidth override.
    pub bandwidth_hz: Option<u32>,
    /// Squelch threshold.
    pub squelch_dbfs: Option<f32>,
    /// De-emphasis override.
    pub deemphasis: Option<Deemphasis>,
}

/// Validated `vfo.update` request; `Some(None)` means "reset to default / off".
#[derive(Debug, Clone, PartialEq)]
pub struct VfoPatch {
    /// Target VFO.
    pub id: u32,
    /// New frequency.
    pub freq_hz: Option<u64>,
    /// New mode (resets the de-emphasis override unless given too).
    pub mode: Option<VfoMode>,
    /// New bandwidth override.
    pub bandwidth_hz: Option<Option<u32>>,
    /// New squelch threshold.
    pub squelch_dbfs: Option<Option<f32>>,
    /// New de-emphasis override.
    pub deemphasis: Option<Option<Deemphasis>>,
}

fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateWire {
    v: u32,
    freq_hz: u64,
    mode: VfoMode,
    bandwidth_hz: Option<u32>,
    squelch_dbfs: Option<f32>,
    deemphasis: Option<DeemphasisWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateWire {
    v: u32,
    id: u32,
    freq_hz: Option<u64>,
    mode: Option<VfoMode>,
    #[serde(default, deserialize_with = "present")]
    bandwidth_hz: Option<Option<u32>>,
    #[serde(default, deserialize_with = "present")]
    squelch_dbfs: Option<Option<f32>>,
    #[serde(default, deserialize_with = "present")]
    deemphasis: Option<Option<DeemphasisWire>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteWire {
    v: u32,
    id: u32,
}

fn check_version(v: u32) -> Result<(), CtlError> {
    if v == VERSION {
        Ok(())
    } else {
        Err(CtlError::invalid(format!(
            "unsupported protocol version {v}"
        )))
    }
}

fn malformed(e: serde_json::Error) -> CtlError {
    CtlError::invalid(format!("malformed request: {e}"))
}

fn check_freq(hz: u64) -> Result<(), CtlError> {
    if (1..=MAX_CENTER_HZ).contains(&hz) {
        Ok(())
    } else {
        Err(CtlError::invalid(format!("freq_hz {hz} out of range")))
    }
}

fn check_bandwidth(bw: Option<u32>) -> Result<(), CtlError> {
    match bw {
        Some(bw) if !BANDWIDTH_RANGE.contains(&bw) => Err(CtlError::invalid(format!(
            "bandwidth_hz {bw} outside {}..={}",
            BANDWIDTH_RANGE.start(),
            BANDWIDTH_RANGE.end()
        ))),
        _ => Ok(()),
    }
}

fn check_squelch(squelch: Option<f32>) -> Result<(), CtlError> {
    match squelch {
        Some(db) if !SQUELCH_RANGE.contains(&db) => Err(CtlError::invalid(format!(
            "squelch_dbfs {db} outside {}..={} dBFS",
            SQUELCH_RANGE.start(),
            SQUELCH_RANGE.end()
        ))),
        _ => Ok(()),
    }
}

fn check_deemphasis(mode: VfoMode, deemphasis: Option<Deemphasis>) -> Result<(), CtlError> {
    if mode == VfoMode::Am && deemphasis.is_some() {
        Err(CtlError::invalid("deemphasis does not apply to am"))
    } else {
        Ok(())
    }
}

/// Parse and validate a `ctl.vfo.create` body.
///
/// # Errors
/// [`ErrorCode::Invalid`] for malformed JSON, unknown fields, a wrong version or bad values.
pub fn parse_create(payload: &[u8]) -> Result<VfoCreate, CtlError> {
    let wire: CreateWire = serde_json::from_slice(payload).map_err(malformed)?;
    check_version(wire.v)?;
    let request = VfoCreate {
        freq_hz: wire.freq_hz,
        mode: wire.mode,
        bandwidth_hz: wire.bandwidth_hz,
        squelch_dbfs: wire.squelch_dbfs,
        deemphasis: wire.deemphasis.map(Into::into),
    };
    check_freq(request.freq_hz)?;
    check_bandwidth(request.bandwidth_hz)?;
    check_squelch(request.squelch_dbfs)?;
    check_deemphasis(request.mode, request.deemphasis)?;
    Ok(request)
}

/// Parse and validate a `ctl.vfo.update` body.
///
/// # Errors
/// [`ErrorCode::Invalid`] for malformed JSON, unknown fields, a wrong version, bad values or a
/// patch that changes nothing.
pub fn parse_update(payload: &[u8]) -> Result<VfoPatch, CtlError> {
    let wire: UpdateWire = serde_json::from_slice(payload).map_err(malformed)?;
    check_version(wire.v)?;
    let patch = VfoPatch {
        id: wire.id,
        freq_hz: wire.freq_hz,
        mode: wire.mode,
        bandwidth_hz: wire.bandwidth_hz,
        squelch_dbfs: wire.squelch_dbfs,
        deemphasis: wire.deemphasis.map(|d| d.map(Into::into)),
    };
    if let Some(hz) = patch.freq_hz {
        check_freq(hz)?;
    }
    check_bandwidth(patch.bandwidth_hz.flatten())?;
    check_squelch(patch.squelch_dbfs.flatten())?;
    if patch.freq_hz.is_none()
        && patch.mode.is_none()
        && patch.bandwidth_hz.is_none()
        && patch.squelch_dbfs.is_none()
        && patch.deemphasis.is_none()
    {
        return Err(CtlError::invalid("request changes nothing"));
    }
    Ok(patch)
}

/// Parse a `ctl.vfo.delete` body and return the id.
///
/// # Errors
/// [`ErrorCode::Invalid`] for malformed JSON, unknown fields or a wrong version.
pub fn parse_delete(payload: &[u8]) -> Result<u32, CtlError> {
    let wire: DeleteWire = serde_json::from_slice(payload).map_err(malformed)?;
    check_version(wire.v)?;
    Ok(wire.id)
}

/// The set of VFOs of one device.
#[derive(Debug, Default)]
pub struct VfoRegistry {
    vfos: Vec<Vfo>,
    last_id: u32,
    revision: u64,
}

/// Reject a VFO the channelizer cannot realise at this sample rate (bandwidth too wide or too
/// narrow for the mode, unsupported rate).
fn check_realisable(vfo: &Vfo, sample_rate: u32) -> Result<(), CtlError> {
    Channel::new(ChannelConfig {
        input_rate: sample_rate,
        offset_hz: 0.0,
        mode: vfo.dsp_mode(),
        bandwidth_hz: vfo.bandwidth_hz,
        squelch_dbfs: vfo.squelch_dbfs,
    })
    .map(|_| ())
    .map_err(|e| CtlError::invalid(e.to_string()))
}

fn not_found(id: u32) -> CtlError {
    CtlError::new(ErrorCode::NotFound, format!("no VFO with id {id}"))
}

fn too_many() -> CtlError {
    CtlError::new(
        ErrorCode::Unsupported,
        format!("at most {MAX_VFOS} VFOs per device"),
    )
}

impl Vfo {
    fn from_create(id: u32, request: VfoCreate) -> Self {
        Self {
            id,
            freq_hz: request.freq_hz,
            mode: request.mode,
            bandwidth_hz: request.bandwidth_hz,
            squelch_dbfs: request.squelch_dbfs,
            deemphasis: request.deemphasis,
        }
    }
}

impl From<profile::Mode> for VfoMode {
    fn from(mode: profile::Mode) -> Self {
        match mode {
            profile::Mode::Wfm => Self::Wfm,
            profile::Mode::Nfm => Self::Nfm,
            profile::Mode::Am => Self::Am,
        }
    }
}

impl From<VfoMode> for profile::Mode {
    fn from(mode: VfoMode) -> Self {
        match mode {
            VfoMode::Wfm => Self::Wfm,
            VfoMode::Nfm => Self::Nfm,
            VfoMode::Am => Self::Am,
        }
    }
}

fn deemphasis_of(d: profile::Deemphasis) -> Deemphasis {
    match d {
        profile::Deemphasis::None => Deemphasis::None,
        profile::Deemphasis::Us50 => Deemphasis::Us50,
        profile::Deemphasis::Us75 => Deemphasis::Us75,
    }
}

impl From<&profile::Vfo> for VfoCreate {
    fn from(vfo: &profile::Vfo) -> Self {
        Self {
            freq_hz: vfo.freq_hz,
            mode: vfo.mode.into(),
            bandwidth_hz: vfo.bandwidth_hz,
            squelch_dbfs: vfo.squelch_dbfs,
            deemphasis: vfo.deemphasis.map(deemphasis_of),
        }
    }
}

impl VfoRegistry {
    /// All VFOs in creation order.
    #[must_use]
    pub fn list(&self) -> &[Vfo] {
        &self.vfos
    }

    /// Counter that changes whenever the set or any VFO changes.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Add a VFO.
    ///
    /// # Errors
    /// [`ErrorCode::Unsupported`] when [`MAX_VFOS`] exist, [`ErrorCode::Invalid`] when the
    /// channelizer cannot realise it at the device's sample rate.
    pub fn create(&mut self, request: VfoCreate, sample_rate: u32) -> Result<Vfo, CtlError> {
        if self.vfos.len() >= MAX_VFOS {
            return Err(too_many());
        }
        let vfo = Vfo::from_create(self.last_id.saturating_add(1), request);
        check_realisable(&vfo, sample_rate)?;
        self.last_id = vfo.id;
        self.vfos.push(vfo.clone());
        self.revision += 1;
        Ok(vfo)
    }

    /// Apply a partial update.
    ///
    /// # Errors
    /// [`ErrorCode::NotFound`] for an unknown id, [`ErrorCode::Invalid`] for an inconsistent
    /// result (de-emphasis on AM) or a VFO the channelizer cannot realise.
    pub fn update(&mut self, patch: &VfoPatch, sample_rate: u32) -> Result<Vfo, CtlError> {
        let index = self
            .vfos
            .iter()
            .position(|v| v.id == patch.id)
            .ok_or_else(|| not_found(patch.id))?;
        let mut vfo = self.vfos[index].clone();
        if let Some(hz) = patch.freq_hz {
            vfo.freq_hz = hz;
        }
        if let Some(mode) = patch.mode {
            if mode != vfo.mode {
                vfo.deemphasis = None;
            }
            vfo.mode = mode;
        }
        if let Some(bw) = patch.bandwidth_hz {
            vfo.bandwidth_hz = bw;
        }
        if let Some(sq) = patch.squelch_dbfs {
            vfo.squelch_dbfs = sq;
        }
        if let Some(de) = patch.deemphasis {
            vfo.deemphasis = de;
        }
        check_deemphasis(vfo.mode, vfo.deemphasis)?;
        check_realisable(&vfo, sample_rate)?;
        self.vfos[index] = vfo.clone();
        self.revision += 1;
        Ok(vfo)
    }

    /// Replace the whole set (profile load). Everything is validated first; on an error the
    /// registry is untouched. Fresh ids are assigned.
    ///
    /// # Errors
    /// [`ErrorCode::Unsupported`] for more than [`MAX_VFOS`], [`ErrorCode::Invalid`] when one of
    /// them cannot be realised at `sample_rate`.
    pub fn replace(
        &mut self,
        requests: Vec<VfoCreate>,
        sample_rate: u32,
    ) -> Result<Vec<Vfo>, CtlError> {
        if requests.len() > MAX_VFOS {
            return Err(too_many());
        }
        let mut next = self.last_id;
        let mut vfos = Vec::with_capacity(requests.len());
        for request in requests {
            next = next.saturating_add(1);
            let vfo = Vfo::from_create(next, request);
            check_realisable(&vfo, sample_rate).map_err(|e| {
                CtlError::invalid(format!("VFO at {} Hz: {}", vfo.freq_hz, e.message))
            })?;
            vfos.push(vfo);
        }
        self.last_id = next;
        self.vfos.clone_from(&vfos);
        self.revision += 1;
        Ok(vfos)
    }

    /// Check that `requests` could replace the set at `sample_rate` without touching anything.
    ///
    /// # Errors
    /// Same as [`VfoRegistry::replace`].
    pub fn check_set(requests: &[VfoCreate], sample_rate: u32) -> Result<(), CtlError> {
        Self::default()
            .replace(requests.to_vec(), sample_rate)
            .map(|_| ())
    }

    /// Replace the set with VFOs of a previous run, keeping their ids. Entries that are no
    /// longer valid (duplicate id, unrealisable at `sample_rate`, over the limit) are dropped;
    /// returns how many.
    pub fn restore(&mut self, saved: Vec<(u32, VfoCreate)>, sample_rate: u32) -> usize {
        let total = saved.len();
        let mut vfos: Vec<Vfo> = Vec::new();
        for (id, request) in saved {
            let vfo = Vfo::from_create(id, request);
            if vfos.len() < MAX_VFOS
                && id > 0
                && !vfos.iter().any(|v| v.id == id)
                && check_realisable(&vfo, sample_rate).is_ok()
            {
                vfos.push(vfo);
            }
        }
        self.last_id = vfos.iter().map(|v| v.id).max().unwrap_or(0);
        let dropped = total - vfos.len();
        self.vfos = vfos;
        self.revision += 1;
        dropped
    }

    /// Remove a VFO.
    ///
    /// # Errors
    /// [`ErrorCode::NotFound`] for an unknown id.
    pub fn delete(&mut self, id: u32) -> Result<(), CtlError> {
        let index = self
            .vfos
            .iter()
            .position(|v| v.id == id)
            .ok_or_else(|| not_found(id))?;
        self.vfos.remove(index);
        self.revision += 1;
        Ok(())
    }
}

/// The registry shared between the control service and the audio engine.
#[derive(Debug, Default)]
pub struct VfoShared {
    registry: Mutex<VfoRegistry>,
    generation: AtomicU64,
}

impl VfoShared {
    /// Run `f` on the registry; the generation follows the registry's revision.
    pub fn with<R>(&self, f: impl FnOnce(&mut VfoRegistry) -> R) -> R {
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = f(&mut registry);
        self.generation
            .store(registry.revision(), Ordering::Release);
        result
    }

    /// Cheap change detector for pollers.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Copy of the current VFOs and the generation they belong to.
    #[must_use]
    pub fn snapshot(&self) -> (u64, Vec<Vfo>) {
        self.with(|r| (r.revision(), r.list().to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 1_024_000;
    const CENTER: u64 = 100_000_000;
    const T: Tuning = Tuning {
        center_hz: CENTER,
        sample_rate: RATE,
    };

    fn create(json: &str) -> Result<VfoCreate, CtlError> {
        parse_create(json.as_bytes())
    }

    fn simple(freq_hz: u64) -> VfoCreate {
        VfoCreate {
            freq_hz,
            mode: VfoMode::Nfm,
            bandwidth_hz: None,
            squelch_dbfs: None,
            deemphasis: None,
        }
    }

    #[test]
    fn parses_create() {
        let r = create(
            r#"{"v":1,"freq_hz":100200000,"mode":"wfm","bandwidth_hz":150000,"squelch_dbfs":-60.5,"deemphasis":"75us"}"#,
        )
        .expect("valid");
        assert_eq!(r.freq_hz, 100_200_000);
        assert_eq!(r.mode, VfoMode::Wfm);
        assert_eq!(r.bandwidth_hz, Some(150_000));
        assert_eq!(r.squelch_dbfs, Some(-60.5));
        assert_eq!(r.deemphasis, Some(Deemphasis::Us75));
        let r = create(r#"{"v":1,"freq_hz":1,"mode":"am","squelch_dbfs":null}"#).expect("valid");
        assert_eq!(
            (r.bandwidth_hz, r.squelch_dbfs, r.deemphasis),
            (None, None, None)
        );
    }

    #[test]
    fn rejects_invalid_create() {
        for bad in [
            "",
            "[]",
            r#"{"freq_hz":1,"mode":"am"}"#,
            r#"{"v":2,"freq_hz":1,"mode":"am"}"#,
            r#"{"v":1,"mode":"am"}"#,
            r#"{"v":1,"freq_hz":1}"#,
            r#"{"v":1,"freq_hz":0,"mode":"am"}"#,
            r#"{"v":1,"freq_hz":2000000000000,"mode":"am"}"#,
            r#"{"v":1,"freq_hz":1,"mode":"usb"}"#,
            r#"{"v":1,"freq_hz":1,"mode":"am","bandwidth_hz":10}"#,
            r#"{"v":1,"freq_hz":1,"mode":"am","bandwidth_hz":5000000}"#,
            r#"{"v":1,"freq_hz":1,"mode":"am","squelch_dbfs":5}"#,
            r#"{"v":1,"freq_hz":1,"mode":"am","squelch_dbfs":"x"}"#,
            r#"{"v":1,"freq_hz":1,"mode":"am","deemphasis":"50us"}"#,
            r#"{"v":1,"freq_hz":1,"mode":"nfm","deemphasis":"1us"}"#,
            r#"{"v":1,"freq_hz":1,"mode":"nfm","freq":1}"#,
            r#"{"v":1,"id":1,"freq_hz":1,"mode":"nfm"}"#,
        ] {
            let err = create(bad).expect_err(bad);
            assert_eq!(err.code, ErrorCode::Invalid, "{bad}");
        }
    }

    #[test]
    fn parses_update_with_null_resets() {
        let p = parse_update(
            br#"{"v":1,"id":3,"freq_hz":5,"bandwidth_hz":null,"squelch_dbfs":-70,"deemphasis":null}"#,
        )
        .expect("valid");
        assert_eq!(p.id, 3);
        assert_eq!(p.freq_hz, Some(5));
        assert_eq!(p.mode, None);
        assert_eq!(p.bandwidth_hz, Some(None));
        assert_eq!(p.squelch_dbfs, Some(Some(-70.0)));
        assert_eq!(p.deemphasis, Some(None));
        for bad in [
            r#"{"v":1,"id":3}"#,
            r#"{"v":1,"freq_hz":5}"#,
            r#"{"v":1,"id":3,"bandwidth_hz":5}"#,
            r#"{"v":1,"id":3,"squelch_dbfs":1}"#,
            r#"{"v":1,"id":3,"mode":"x"}"#,
            r#"{"v":1,"id":-3,"freq_hz":5}"#,
            r#"{"v":3,"id":3,"freq_hz":5}"#,
            r#"{"v":1,"id":3,"foo":5}"#,
        ] {
            assert_eq!(
                parse_update(bad.as_bytes()).expect_err(bad).code,
                ErrorCode::Invalid,
                "{bad}"
            );
        }
        assert_eq!(parse_delete(br#"{"v":1,"id":9}"#), Ok(9));
        assert!(parse_delete(br#"{"v":1}"#).is_err());
        assert!(parse_delete(br#"{"v":1,"id":1,"x":1}"#).is_err());
    }

    #[test]
    fn registry_assigns_monotonic_ids_and_enforces_limit() {
        let mut reg = VfoRegistry::default();
        let a = reg.create(simple(100_100_000), RATE).expect("a");
        let b = reg.create(simple(100_200_000), RATE).expect("b");
        assert_eq!((a.id, b.id), (1, 2));
        reg.delete(2).expect("delete");
        assert_eq!(reg.create(simple(100_300_000), RATE).expect("c").id, 3);
        for _ in 0..MAX_VFOS - 2 {
            reg.create(simple(100_100_000), RATE).expect("fill");
        }
        assert_eq!(reg.list().len(), MAX_VFOS);
        let err = reg.create(simple(100_100_000), RATE).expect_err("limit");
        assert_eq!(err.code, ErrorCode::Unsupported);
        assert_eq!(
            reg.delete(99).expect_err("unknown").code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn update_patches_only_given_fields() {
        let mut reg = VfoRegistry::default();
        let id = reg
            .create(
                VfoCreate {
                    freq_hz: 100_000_000,
                    mode: VfoMode::Wfm,
                    bandwidth_hz: Some(150_000),
                    squelch_dbfs: Some(-60.0),
                    deemphasis: Some(Deemphasis::Us75),
                },
                RATE,
            )
            .expect("create")
            .id;
        let patch = |json: &str| parse_update(json.as_bytes()).expect("patch");

        let v = reg
            .update(
                &patch(&format!(r#"{{"v":1,"id":{id},"freq_hz":100400000}}"#)),
                RATE,
            )
            .expect("update");
        assert_eq!(v.freq_hz, 100_400_000);
        assert_eq!(v.bandwidth_hz, Some(150_000));
        assert_eq!(v.squelch_dbfs, Some(-60.0));
        assert_eq!(v.deemphasis, Some(Deemphasis::Us75));

        let v = reg
            .update(
                &patch(&format!(
                    r#"{{"v":1,"id":{id},"squelch_dbfs":null,"bandwidth_hz":null}}"#
                )),
                RATE,
            )
            .expect("update");
        assert_eq!((v.squelch_dbfs, v.bandwidth_hz), (None, None));
        assert_eq!(v.bandwidth(), 180_000);

        // A mode change drops the de-emphasis override ...
        let v = reg
            .update(
                &patch(&format!(r#"{{"v":1,"id":{id},"mode":"nfm"}}"#)),
                RATE,
            )
            .expect("update");
        assert_eq!(
            (v.mode, v.deemphasis, v.bandwidth()),
            (VfoMode::Nfm, None, 12_500)
        );
        // ... but an explicit one given together with it wins.
        let v = reg
            .update(
                &patch(&format!(
                    r#"{{"v":1,"id":{id},"mode":"wfm","deemphasis":"none"}}"#
                )),
                RATE,
            )
            .expect("update");
        assert_eq!(v.deemphasis, Some(Deemphasis::None));
        // AM takes no de-emphasis; a failed patch leaves the VFO untouched.
        let before = reg.list().to_vec();
        let revision = reg.revision();
        let err = reg
            .update(
                &patch(&format!(
                    r#"{{"v":1,"id":{id},"mode":"am","deemphasis":"50us"}}"#
                )),
                RATE,
            )
            .expect_err("am deemphasis");
        assert_eq!(err.code, ErrorCode::Invalid);
        assert_eq!(reg.list(), before);
        assert_eq!(reg.revision(), revision);
        assert_eq!(
            reg.update(&patch(r#"{"v":1,"id":42,"freq_hz":5}"#), RATE)
                .expect_err("unknown")
                .code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn unrealisable_bandwidth_is_rejected_at_the_device_rate() {
        let mut reg = VfoRegistry::default();
        let mut request = simple(CENTER);
        request.mode = VfoMode::Wfm;
        request.bandwidth_hz = Some(900_000);
        // Too wide for the channel rate the mode needs at this device rate.
        assert!(reg.create(request, 250_000).is_err());
        assert!(reg.list().is_empty());
        assert_eq!(reg.revision(), 0);
    }

    #[test]
    fn paused_when_any_part_of_the_channel_leaves_the_passband() {
        let at = |offset: i64, mode: VfoMode, bw: Option<u32>| Vfo {
            id: 1,
            freq_hz: (CENTER as i64 + offset) as u64,
            mode,
            bandwidth_hz: bw,
            squelch_dbfs: None,
            deemphasis: None,
        };
        // The usable half span is 435.2 kHz (85 % of 1.024 MS/s); NFM default 12.5 kHz leaves
        // 428.95 kHz of offset.
        assert_eq!(at(428_000, VfoMode::Nfm, None).status(T), Status::Active);
        assert_eq!(at(-428_000, VfoMode::Nfm, None).status(T), Status::Active);
        assert_eq!(at(430_000, VfoMode::Nfm, None).status(T), Status::Paused);
        assert_eq!(at(-430_000, VfoMode::Nfm, None).status(T), Status::Paused);
        assert_eq!(at(0, VfoMode::Wfm, None).status(T), Status::Active);
        // WFM default 180 kHz: 345.2 kHz of offset at most.
        assert_eq!(at(350_000, VfoMode::Wfm, None).status(T), Status::Paused);
        assert_eq!(at(340_000, VfoMode::Wfm, None).status(T), Status::Active);
        // An explicit bandwidth overrides the default.
        assert_eq!(
            at(350_000, VfoMode::Wfm, Some(50_000)).status(T),
            Status::Active
        );
        // Far away (e.g. after a retune) and a different rate.
        assert_eq!(at(60_000_000, VfoMode::Am, None).status(T), Status::Paused);
        let narrow = Tuning {
            sample_rate: 250_000,
            ..T
        };
        assert_eq!(
            at(95_000, VfoMode::Nfm, None).status(narrow),
            Status::Active
        );
        assert_eq!(
            at(120_000, VfoMode::Nfm, None).status(narrow),
            Status::Paused
        );
        // Offsets are signed and exact even for large absolute frequencies.
        let v = at(-1000, VfoMode::Am, None);
        assert_eq!(v.offset_hz(CENTER), -1000.0);
    }

    #[test]
    fn wire_reports_effective_values_and_status() {
        let vfo = Vfo {
            id: 4,
            freq_hz: CENTER + 200_000,
            mode: VfoMode::Wfm,
            bandwidth_hz: None,
            squelch_dbfs: None,
            deemphasis: None,
        };
        let value = serde_json::to_value(vfo.wire(T)).expect("json");
        assert_eq!(value["id"], 4);
        assert_eq!(value["mode"], "wfm");
        assert_eq!(value["bandwidth_hz"], 180_000);
        assert_eq!(value["deemphasis"], "50us");
        assert!(value["squelch_dbfs"].is_null());
        assert_eq!(value["offset_hz"], 200_000.0);
        assert_eq!(value["status"], "active");
        let away = Tuning { center_hz: 0, ..T };
        assert_eq!(
            serde_json::to_value(vfo.wire(away)).expect("json")["status"],
            "paused"
        );
        let am = Vfo {
            mode: VfoMode::Am,
            ..vfo
        };
        assert!(serde_json::to_value(am.wire(T)).expect("json")["deemphasis"].is_null());
    }

    #[test]
    fn shared_generation_follows_revision() {
        let shared = VfoShared::default();
        assert_eq!(shared.generation(), 0);
        shared
            .with(|r| r.create(simple(CENTER), RATE))
            .expect("create");
        assert_eq!(shared.generation(), 1);
        let _ = shared.with(|r| r.delete(77));
        assert_eq!(shared.generation(), 1);
        let (rev, list) = shared.snapshot();
        assert_eq!((rev, list.len()), (1, 1));
    }
}
