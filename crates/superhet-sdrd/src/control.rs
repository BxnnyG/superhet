//! Wire format of the device control protocol (`sh.v1.dev.<dev>.ctl.*`) and the pure logic
//! around it: request parsing and validation, request coalescing, reply and state encoding.
//!
//! The NATS plumbing lives in `service`; everything here is testable without a bus.

use std::time::Duration;

use async_nats::Subject;
use serde::{Deserialize, Serialize};
use superhet_device::{DeviceState, Gain, TuneRequest};

use crate::lock::Holder;
use crate::vfo::{Tuning, Vfo, VfoWire};
use crate::window;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Protocol version carried in every message.
pub const VERSION: u32 = 1;
/// Quiet period after the last tune request before the batch is applied.
pub const DEBOUNCE: Duration = Duration::from_millis(30);
/// A batch is applied at the latest this long after its first request, however busy the bus is.
pub const MAX_BATCH_WAIT: Duration = Duration::from_millis(150);

/// Upper bound for centre frequencies (sanity check; the device applies its own range).
pub(crate) const MAX_CENTER_HZ: u64 = 1_000_000_000_000;
/// Accepted manual gain range in dB (the device snaps to what the tuner supports).
const GAIN_RANGE_DB: std::ops::RangeInclusive<f64> = -20.0..=100.0;
/// Accepted crystal correction in ppm.
const PPM_RANGE: std::ops::RangeInclusive<i32> = -1000..=1000;

/// Why a request failed; reported as `code` in the reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Malformed request or out-of-range value.
    Invalid,
    /// The request names something that does not exist (unknown VFO id).
    NotFound,
    /// The device cannot do what was asked.
    Unsupported,
    /// The device failed while applying the request.
    Device,
    /// The tuning lock forbids a centre or sample-rate change.
    Locked,
    /// The request clashes with the current state (the device is claimed by another plugin).
    Conflict,
}

impl ErrorCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::NotFound => "not_found",
            Self::Unsupported => "unsupported",
            Self::Device => "device",
            Self::Locked => "locked",
            Self::Conflict => "conflict",
        }
    }
}

/// A request failure with a human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtlError {
    /// Machine-readable class.
    pub code: ErrorCode,
    /// Human-readable reason.
    pub message: String,
    /// For [`ErrorCode::Locked`]: who holds the lock.
    pub holders: Vec<Holder>,
}

impl CtlError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            holders: Vec::new(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Invalid, message)
    }
}

impl From<superhet_device::Error> for CtlError {
    fn from(err: superhet_device::Error) -> Self {
        let code = match err {
            superhet_device::Error::Config(_) => ErrorCode::Invalid,
            superhet_device::Error::Unsupported(_) => ErrorCode::Unsupported,
            superhet_device::Error::Io(_) | superhet_device::Error::State(_) => ErrorCode::Device,
        };
        Self::new(code, err.to_string())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TuneWire {
    v: u32,
    center_hz: Option<u64>,
    sample_rate: Option<u32>,
    gain: Option<serde_json::Value>,
    ppm: Option<i32>,
    bias_tee: Option<bool>,
}

/// Parse and validate a `ctl.tune` request body.
///
/// # Errors
/// [`ErrorCode::Invalid`] for malformed JSON, unknown fields, a wrong version, out-of-range
/// values or a request that changes nothing.
pub fn parse_tune(payload: &[u8]) -> Result<TuneRequest, CtlError> {
    let wire: TuneWire = serde_json::from_slice(payload)
        .map_err(|e| CtlError::invalid(format!("malformed request: {e}")))?;
    if wire.v != VERSION {
        return Err(CtlError::invalid(format!(
            "unsupported protocol version {}",
            wire.v
        )));
    }
    let gain = wire.gain.as_ref().map(parse_gain).transpose()?;
    if let Some(hz) = wire.center_hz
        && !(1..=MAX_CENTER_HZ).contains(&hz)
    {
        return Err(CtlError::invalid(format!("center_hz {hz} out of range")));
    }
    if wire.sample_rate == Some(0) {
        return Err(CtlError::invalid("sample_rate must be non-zero"));
    }
    if let Some(ppm) = wire.ppm
        && !PPM_RANGE.contains(&ppm)
    {
        return Err(CtlError::invalid(format!(
            "ppm {ppm} outside {}..={}",
            PPM_RANGE.start(),
            PPM_RANGE.end()
        )));
    }
    let request = TuneRequest {
        center_hz: wire.center_hz,
        sample_rate: wire.sample_rate,
        gain,
        ppm: wire.ppm,
        bias_tee: wire.bias_tee,
    };
    if request.is_empty() {
        return Err(CtlError::invalid("request changes nothing"));
    }
    Ok(request)
}

fn parse_gain(value: &serde_json::Value) -> Result<Gain, CtlError> {
    match value {
        serde_json::Value::String(s) if s == "auto" => Ok(Gain::Auto),
        serde_json::Value::Number(n) => match n.as_f64() {
            Some(db) if GAIN_RANGE_DB.contains(&db) => Ok(Gain::Db(db as f32)),
            _ => Err(CtlError::invalid(format!(
                "gain {n} outside {}..={} dB",
                GAIN_RANGE_DB.start(),
                GAIN_RANGE_DB.end()
            ))),
        },
        _ => Err(CtlError::invalid("gain must be \"auto\" or a number in dB")),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockWire {
    v: u32,
    holder: Option<String>,
    name: Option<String>,
    force: Option<bool>,
}

/// A validated `ctl.lock.acquire` / `ctl.lock.release` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockRequest {
    /// Take the lock as `id` (shown as `name`).
    Acquire {
        /// Holder id (`user` when the sender names none, otherwise e.g. `u:3`).
        id: String,
        /// Display name.
        name: Option<String>,
    },
    /// Drop the lock of holder `id`.
    Release {
        /// Holder id.
        id: String,
    },
    /// Drop every user lock (admin override).
    ReleaseAll,
}

/// Longest accepted holder display name, in characters.
const HOLDER_NAME_MAX: usize = 64;

/// Parse a `ctl.lock.acquire` (`acquire == true`) or `ctl.lock.release` body. An empty body
/// (`{"v":1}`) addresses the anonymous holder `user`, as before accounts existed.
///
/// # Errors
/// [`ErrorCode::Invalid`] for malformed JSON, unknown fields, a wrong version, a bad holder or
/// `force` on an acquire.
pub fn parse_lock(payload: &[u8], acquire: bool) -> Result<LockRequest, CtlError> {
    let wire: LockWire = serde_json::from_slice(payload)
        .map_err(|e| CtlError::invalid(format!("malformed request: {e}")))?;
    if wire.v != VERSION {
        return Err(CtlError::invalid(format!(
            "unsupported protocol version {}",
            wire.v
        )));
    }
    let id = wire.holder.unwrap_or_else(|| "user".to_owned());
    crate::vfo::check_owner(&id)?;
    if let Some(name) = &wire.name
        && (name.is_empty()
            || name.chars().count() > HOLDER_NAME_MAX
            || name.chars().any(char::is_control))
    {
        return Err(CtlError::invalid(
            "holder name must be 1-64 printable characters",
        ));
    }
    if acquire {
        if wire.force.is_some() {
            return Err(CtlError::invalid("force only applies to release"));
        }
        Ok(LockRequest::Acquire {
            id,
            name: wire.name,
        })
    } else if wire.force == Some(true) {
        Ok(LockRequest::ReleaseAll)
    } else {
        Ok(LockRequest::Release { id })
    }
}

/// A validated tune request waiting to be applied.
#[derive(Debug)]
pub struct TuneJob {
    /// What to change.
    pub request: TuneRequest,
    /// Where to send the result; `None` for fire-and-forget publishes.
    pub reply: Option<Subject>,
}

/// Several tune requests folded into one device operation.
#[derive(Debug, Default, PartialEq)]
pub struct Batch {
    /// The merged request (per field the latest value wins).
    pub request: TuneRequest,
    /// Everybody waiting for the result.
    pub replies: Vec<Subject>,
    /// Number of requests folded in.
    pub merged: usize,
}

impl Batch {
    fn push(&mut self, job: TuneJob) {
        self.request.merge(&job.request);
        self.replies.extend(job.reply);
        self.merged += 1;
    }
}

/// Collect further jobs after `first` until the queue has been quiet for `quiet`, but no longer
/// than `max_wait` after `first`. Bursts (a drag across the spectrum) become one retune.
pub async fn collect_batch(
    first: TuneJob,
    rx: &mut mpsc::Receiver<TuneJob>,
    quiet: Duration,
    max_wait: Duration,
) -> Batch {
    let hard_deadline = Instant::now() + max_wait;
    let mut batch = Batch::default();
    batch.push(first);
    loop {
        let deadline = (Instant::now() + quiet).min(hard_deadline);
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(job)) => batch.push(job),
            Ok(None) | Err(_) => return batch,
        }
    }
}

#[derive(Serialize)]
struct GainWire {
    mode: &'static str,
    db: Option<f32>,
}

#[derive(Serialize)]
struct StateWire<'a> {
    v: u32,
    device: &'a str,
    driver: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tuner: Option<&'a str>,
    center_hz: u64,
    sample_rate: u32,
    gain: Option<GainWire>,
    ppm: i32,
    bias_tee: bool,
    tuning_epoch: u64,
    gains_db: &'a [f32],
    vfos: Vec<VfoWire<'a>>,
    locks: &'a [Holder],
    window: WindowWire,
    overload: OverloadWire,
    /// IQ blocks dropped since start because a stage lagged (device to DSP, DSP to VFO).
    dropped_blocks: u64,
}

/// ADC overload as seen over the last few seconds.
#[derive(Serialize)]
struct OverloadWire {
    active: bool,
    /// Share of the samples at the ADC rails in percent.
    clipped_percent: f64,
}

#[derive(Serialize)]
struct WindowWire {
    start_hz: i64,
    end_hz: i64,
}

/// Encode a device state, with its VFOs evaluated against the current tuning, as the JSON
/// published on the state subject and stored in KV.
#[must_use]
pub fn state_json(
    device: &str,
    state: &DeviceState,
    vfos: &[Vfo],
    locks: &[Holder],
    overload: (f64, bool),
    dropped_blocks: u64,
) -> String {
    let tuning = Tuning {
        center_hz: state.center_hz,
        sample_rate: state.sample_rate,
    };
    let wire = StateWire {
        v: VERSION,
        device,
        driver: state.driver,
        tuner: state.tuner.as_deref(),
        center_hz: state.center_hz,
        sample_rate: state.sample_rate,
        gain: state.gain.map(|g| match g {
            Gain::Auto => GainWire {
                mode: "auto",
                db: None,
            },
            Gain::Db(db) => GainWire {
                mode: "manual",
                db: Some(db),
            },
        }),
        ppm: state.ppm,
        bias_tee: state.bias_tee,
        tuning_epoch: state.tuning_epoch,
        gains_db: &state.gains_db,
        vfos: vfos.iter().map(|v| v.wire(tuning)).collect(),
        locks,
        overload: OverloadWire {
            active: overload.1,
            clipped_percent: overload.0,
        },
        dropped_blocks,
        window: {
            let (start_hz, end_hz) = window::bounds(tuning);
            WindowWire { start_hz, end_hz }
        },
    };
    // Plain data with string keys: serialisation cannot fail.
    serde_json::to_string(&wire).unwrap_or_else(|_| "{}".to_owned())
}

/// Success reply wrapping an encoded state.
#[must_use]
pub fn ok_reply(state_json: &str) -> Vec<u8> {
    format!(r#"{{"v":{VERSION},"ok":true,"state":{state_json}}}"#).into_bytes()
}

/// What free tuning did about a created or moved VFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retune {
    /// The VFO is inside the window (or was not moved); the centre stays.
    None,
    /// The centre followed the VFO.
    Applied,
    /// The VFO is outside the window but the tuning lock holds the centre; the VFO is paused.
    Locked,
    /// The centre had to follow but the device refused; the VFO is outside the window.
    Failed,
}

impl Retune {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Applied => "applied",
            Self::Locked => "locked",
            Self::Failed => "failed",
        }
    }
}

/// Success reply for `vfo.create` / `vfo.update`: the affected VFO, what free tuning did, and
/// the new state.
#[must_use]
pub fn vfo_reply(vfo: &VfoWire, retune: Retune, state_json: &str) -> Vec<u8> {
    let vfo = serde_json::to_string(vfo).unwrap_or_else(|_| "{}".to_owned());
    let retune = retune.as_str();
    format!(r#"{{"v":{VERSION},"ok":true,"vfo":{vfo},"retune":"{retune}","state":{state_json}}}"#)
        .into_bytes()
}

/// Failure reply.
#[must_use]
pub fn error_reply(error: &CtlError) -> Vec<u8> {
    let mut reply = serde_json::json!({
        "v": VERSION,
        "ok": false,
        "code": error.code.as_str(),
        "error": error.message,
    });
    if error.code == ErrorCode::Locked {
        reply["holders"] = serde_json::to_value(&error.holders).unwrap_or_default();
    }
    reply.to_string().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<TuneRequest, CtlError> {
        parse_tune(json.as_bytes())
    }

    #[test]
    fn parses_full_request() {
        let r = parse(
            r#"{"v":1,"center_hz":145500000,"sample_rate":2048000,"gain":28.5,"ppm":-2,"bias_tee":true}"#,
        )
        .expect("valid");
        assert_eq!(r.center_hz, Some(145_500_000));
        assert_eq!(r.sample_rate, Some(2_048_000));
        assert_eq!(r.gain, Some(Gain::Db(28.5)));
        assert_eq!(r.ppm, Some(-2));
        assert_eq!(r.bias_tee, Some(true));
        assert_eq!(
            parse(r#"{"v":1,"gain":"auto"}"#).expect("valid").gain,
            Some(Gain::Auto)
        );
    }

    #[test]
    fn rejects_invalid_requests() {
        for bad in [
            "",
            "[]",
            r#"{"center_hz":1}"#,
            r#"{"v":2,"center_hz":1}"#,
            r#"{"v":1}"#,
            r#"{"v":1,"center_hz":0}"#,
            r#"{"v":1,"center_hz":-5}"#,
            r#"{"v":1,"center_hz":1.5}"#,
            r#"{"v":1,"center_hz":"100"}"#,
            r#"{"v":1,"center_hz":2000000000000}"#,
            r#"{"v":1,"sample_rate":0}"#,
            r#"{"v":1,"gain":"loud"}"#,
            r#"{"v":1,"gain":500}"#,
            r#"{"v":1,"gain":true}"#,
            r#"{"v":1,"ppm":100000}"#,
            r#"{"v":1,"bias_tee":1}"#,
            r#"{"v":1,"centre_hz":1}"#,
        ] {
            let err = parse(bad).expect_err(bad);
            assert_eq!(err.code, ErrorCode::Invalid, "{bad}");
        }
    }

    fn job(request: TuneRequest, reply: &str) -> TuneJob {
        TuneJob {
            request,
            reply: Some(Subject::from(reply)),
        }
    }

    fn center(hz: u64) -> TuneRequest {
        TuneRequest {
            center_hz: Some(hz),
            ..TuneRequest::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn burst_collapses_into_one_batch_latest_wins() {
        let (tx, mut rx) = mpsc::channel(16);
        let first = job(center(1), "r1");
        for (hz, name) in [(2, "r2"), (3, "r3")] {
            tx.send(job(center(hz), name)).await.expect("send");
        }
        tx.send(job(
            TuneRequest {
                gain: Some(Gain::Auto),
                ..TuneRequest::default()
            },
            "r4",
        ))
        .await
        .expect("send");
        let batch = collect_batch(first, &mut rx, DEBOUNCE, MAX_BATCH_WAIT).await;
        assert_eq!(batch.merged, 4);
        assert_eq!(batch.request.center_hz, Some(3));
        assert_eq!(batch.request.gain, Some(Gain::Auto));
        assert_eq!(batch.replies.len(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn batch_ends_after_quiet_period_and_keeps_later_jobs() {
        let (tx, mut rx) = mpsc::channel(16);
        let started = Instant::now();
        let batch = collect_batch(job(center(1), "r1"), &mut rx, DEBOUNCE, MAX_BATCH_WAIT).await;
        assert_eq!(batch.merged, 1);
        assert_eq!(started.elapsed(), DEBOUNCE);
        // A job arriving afterwards starts the next batch.
        tx.send(job(center(2), "r2")).await.expect("send");
        let next = rx.recv().await.expect("queued");
        assert_eq!(next.request.center_hz, Some(2));
    }

    #[tokio::test(start_paused = true)]
    async fn steady_stream_is_cut_off_at_max_wait() {
        let (tx, mut rx) = mpsc::channel(64);
        let feeder = tokio::spawn(async move {
            // One request every 10 ms, forever (until the receiver goes away).
            let mut hz = 10;
            while tx.send(job(center(hz), "r")).await.is_ok() {
                hz += 1;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let started = Instant::now();
        let batch = collect_batch(job(center(1), "r"), &mut rx, DEBOUNCE, MAX_BATCH_WAIT).await;
        assert_eq!(started.elapsed(), MAX_BATCH_WAIT);
        assert!(batch.merged >= 10, "{}", batch.merged);
        drop(rx);
        let _ = feeder.await;
    }

    #[test]
    fn device_errors_map_to_codes() {
        use superhet_device::Error;
        let code = |e: Error| CtlError::from(e).code;
        assert_eq!(code(Error::Config("x".into())), ErrorCode::Invalid);
        assert_eq!(code(Error::Unsupported("x")), ErrorCode::Unsupported);
        assert_eq!(code(Error::State("x")), ErrorCode::Device);
    }

    #[test]
    fn encodes_state_and_replies() {
        let state = DeviceState {
            driver: "rtlsdr",
            tuner: Some("R820T/R820T2".into()),
            center_hz: 100_000_000,
            sample_rate: 2_048_000,
            gain: Some(Gain::Db(28.0)),
            ppm: 1,
            bias_tee: false,
            tuning_epoch: 3,
            gains_db: vec![0.0, 0.9, 28.0],
        };
        let json = state_json(
            "rtl0",
            &state,
            &[],
            &[Holder::user_since(7)],
            (7.3, true),
            5,
        );
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["v"], 1);
        assert_eq!(value["device"], "rtl0");
        assert_eq!(value["tuner"], "R820T/R820T2");
        assert_eq!(value["gain"]["mode"], "manual");
        assert_eq!(value["gain"]["db"], 28.0);
        assert_eq!(value["tuning_epoch"], 3);
        assert_eq!(value["gains_db"].as_array().map(Vec::len), Some(3));
        assert_eq!(value["vfos"].as_array().map(Vec::len), Some(0));
        assert_eq!(value["locks"][0]["kind"], "user");
        assert_eq!(value["locks"][0]["since_ms"], 7);
        // 85 % of 2.048 MS/s around 100 MHz.
        assert_eq!(value["window"]["start_hz"], 99_129_600);
        assert_eq!(value["window"]["end_hz"], 100_870_400);
        assert_eq!(value["overload"]["active"], true);
        assert_eq!(value["overload"]["clipped_percent"], 7.3);
        assert_eq!(value["dropped_blocks"], 5);

        let reply: serde_json::Value = serde_json::from_slice(&ok_reply(&json)).expect("json");
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["state"]["center_hz"], 100_000_000);

        let file = DeviceState {
            driver: "file",
            tuner: None,
            gain: None,
            ..state
        };
        let value: serde_json::Value =
            serde_json::from_str(&state_json("f", &file, &[], &[], (0.0, false), 0)).expect("json");
        assert!(value.get("tuner").is_none());
        assert!(value["gain"].is_null());

        let err: serde_json::Value =
            serde_json::from_slice(&error_reply(&CtlError::invalid("nope"))).expect("json");
        assert_eq!(
            (err["ok"].clone(), err["code"].clone()),
            (false.into(), "invalid".into())
        );
        assert_eq!(err["error"], "nope");
        assert!(err.get("holders").is_none());

        let mut locks = crate::lock::LockSet::default();
        locks.acquire(Holder::user_since(1));
        let err: serde_json::Value =
            serde_json::from_slice(&error_reply(&locks.error("tuning"))).expect("json");
        assert_eq!(err["code"], "locked");
        assert_eq!(err["holders"][0]["kind"], "user");
    }
}
