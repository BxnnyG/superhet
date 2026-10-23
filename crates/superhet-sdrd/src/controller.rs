//! The single place that changes tuning, VFOs and locks.
//!
//! Every operation runs under one gate (the lock set's mutex), so the lock check, the free-tuning
//! decision, the device call and the state publication of one request cannot interleave with
//! another. All methods block (device control transfers, filter design) and belong on a blocking
//! thread. The newest state document is stored in the `watch` channel the NATS side delivers
//! from, so every change reaches the state subject and KV.

use std::sync::{Arc, Mutex, MutexGuard};

use serde::Deserialize;
use superhet_bus::profile::{self, ProfileData};
use superhet_device::{DeviceControl, DeviceState, Gain, TuneRequest};
use tokio::sync::watch;

use crate::control::{CtlError, Retune, VERSION, error_reply, ok_reply, state_json, vfo_reply};
use crate::lock::{Holder, HolderKind, LockSet};
use crate::vfo::{self, Tuning, Vfo, VfoCreate, VfoMode, VfoRegistry, VfoShared};
use crate::window;

/// Latest encoded state, `None` until the first one is known.
pub type StateTx = watch::Sender<Option<Arc<str>>>;

/// Owns the control path of one device.
pub struct Controller {
    device: String,
    control: Arc<dyn DeviceControl>,
    vfos: Arc<VfoShared>,
    locks: Mutex<LockSet>,
    state: StateTx,
}

fn tuning_of(state: &DeviceState) -> Tuning {
    Tuning {
        center_hz: state.center_hz,
        sample_rate: state.sample_rate,
    }
}

fn device_gain(gain: profile::Gain) -> Option<Gain> {
    match (gain.mode, gain.db) {
        (profile::GainMode::Auto, _) => Some(Gain::Auto),
        (profile::GainMode::Manual, Some(db)) => Some(Gain::Db(db)),
        (profile::GainMode::Manual, None) => None,
    }
}

/// Mode of the default VFO: WFM in the FM broadcast band (87.5-108 MHz), NFM elsewhere.
fn default_mode(center_hz: u64) -> VfoMode {
    if (87_500_000..=108_000_000).contains(&center_hz) {
        VfoMode::Wfm
    } else {
        VfoMode::Nfm
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadWire {
    v: u32,
    profile: ProfileData,
}

impl Controller {
    /// Create the controller; `state` receives every new state document.
    #[must_use]
    pub fn new(
        device: String,
        control: Arc<dyn DeviceControl>,
        vfos: Arc<VfoShared>,
        state: StateTx,
    ) -> Self {
        Self {
            device,
            control,
            vfos,
            locks: Mutex::new(LockSet::default()),
            state,
        }
    }

    fn gate(&self) -> MutexGuard<'_, LockSet> {
        self.locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Encode the state for `device_state` plus the current VFOs and `locks`, and publish it if
    /// it changed. Returns the JSON for replies.
    fn publish(&self, locks: &LockSet, device_state: &DeviceState) -> Arc<str> {
        let vfos = self.vfos.with(|r| r.list().to_vec());
        let json: Arc<str> = state_json(&self.device, device_state, &vfos, locks.holders()).into();
        self.state.send_if_modified(|current| {
            if current.as_deref() == Some(&*json) {
                false
            } else {
                *current = Some(Arc::clone(&json));
                true
            }
        });
        json
    }

    /// The newest published state document.
    #[must_use]
    pub fn latest(&self) -> Option<Arc<str>> {
        self.state.borrow().clone()
    }

    /// Publish the state as it is now (start-up).
    pub fn refresh(&self) -> Arc<str> {
        let locks = self.gate();
        self.publish(&locks, &self.control.state())
    }

    /// Apply a (merged) tune request. Rejected with `locked` if it changes centre or sample rate
    /// while a lock is held.
    ///
    /// # Errors
    /// [`crate::control::ErrorCode::Locked`] or the device's error. The state is re-published
    /// either way, because after a device error the hardware may be half-way applied.
    pub fn tune(&self, request: &TuneRequest) -> Result<Arc<str>, CtlError> {
        let locks = self.gate();
        let current = self.control.state();
        if locks.is_locked()
            && (request.center_hz.is_some_and(|c| c != current.center_hz)
                || request
                    .sample_rate
                    .is_some_and(|r| r != current.sample_rate))
        {
            tracing::info!(device = %self.device, "tune rejected: locked");
            return Err(locks.error("tuning"));
        }
        let result = self.control.apply(request);
        let state = result
            .as_ref()
            .map_or_else(|_| self.control.state(), Clone::clone);
        let json = self.publish(&locks, &state);
        match result {
            Ok(state) => {
                tracing::info!(
                    center_hz = state.center_hz,
                    sample_rate = state.sample_rate,
                    epoch = state.tuning_epoch,
                    "tuned"
                );
                Ok(json)
            }
            Err(err) => {
                tracing::warn!(error = %err, "tune failed");
                Err(err.into())
            }
        }
    }

    /// Take or drop the user lock (idempotent) and return the new state.
    pub fn set_user_lock(&self, held: bool) -> Arc<str> {
        let mut locks = self.gate();
        let changed = if held {
            locks.acquire(Holder::user())
        } else {
            locks.release(HolderKind::User, "user")
        };
        if changed {
            tracing::info!(held, "user tuning lock changed");
        }
        self.publish(&locks, &self.control.state())
    }

    /// Execute `vfo.create` / `vfo.update` / `vfo.delete` and build the reply.
    ///
    /// A created or moved VFO that ends up outside the window makes the centre follow (section
    /// 3.1) unless a lock is held, in which case the VFO just stays paused.
    ///
    /// # Errors
    /// Validation errors of the request; see `docs/api-devices.md`.
    pub fn vfo(&self, command: &str, payload: &[u8]) -> Result<Vec<u8>, CtlError> {
        let locks = self.gate();
        let before = self.control.state();
        let rate = before.sample_rate;
        let (affected, moved) = match command {
            "vfo.create" => {
                let request = vfo::parse_create(payload)?;
                let vfo = self.vfos.with(|r| r.create(request, rate))?;
                tracing::info!(id = vfo.id, freq_hz = vfo.freq_hz, mode = ?vfo.mode, "vfo created");
                (Some(vfo), true)
            }
            "vfo.update" => {
                let patch = vfo::parse_update(payload)?;
                let moved =
                    patch.freq_hz.is_some() || patch.mode.is_some() || patch.bandwidth_hz.is_some();
                let vfo = self.vfos.with(|r| r.update(&patch, rate))?;
                tracing::info!(id = vfo.id, freq_hz = vfo.freq_hz, mode = ?vfo.mode, "vfo updated");
                (Some(vfo), moved)
            }
            _ => {
                let id = vfo::parse_delete(payload)?;
                self.vfos.with(|r| r.delete(id))?;
                tracing::info!(id, "vfo deleted");
                (None, false)
            }
        };
        let retune = match &affected {
            Some(vfo) if moved => self.follow(&locks, vfo, &before),
            _ => Retune::None,
        };
        // Read the tuning again: a retune above changed it.
        let device_state = self.control.state();
        let tuning = tuning_of(&device_state);
        let state = self.publish(&locks, &device_state);
        Ok(match affected {
            Some(vfo) => vfo_reply(&vfo.wire(tuning), retune, &state),
            None => ok_reply(&state),
        })
    }

    /// Free tuning: bring the centre to `vfo` if it left the window.
    fn follow(&self, locks: &LockSet, vfo: &Vfo, before: &DeviceState) -> Retune {
        let others: Vec<Vfo> = self.vfos.with(|r| {
            r.list()
                .iter()
                .filter(|o| o.id != vfo.id)
                .cloned()
                .collect()
        });
        let Some(center_hz) = window::retune_center(vfo, &others, tuning_of(before)) else {
            return Retune::None;
        };
        if locks.is_locked() {
            tracing::info!(
                id = vfo.id,
                "vfo outside the window; tuning is locked, it stays paused"
            );
            return Retune::Locked;
        }
        let request = TuneRequest {
            center_hz: Some(center_hz),
            ..TuneRequest::default()
        };
        match self.control.apply(&request) {
            Ok(_) => {
                tracing::info!(id = vfo.id, center_hz, "centre follows the vfo");
                Retune::Applied
            }
            Err(e) => {
                tracing::warn!(id = vfo.id, center_hz, error = %e, "retune for the vfo failed");
                Retune::Failed
            }
        }
    }

    /// Apply a profile atomically: tuning, gain, ppm, bias tee, the VFO set and the lock flag.
    ///
    /// Everything that can be checked without touching the device is checked first (lock, VFOs
    /// realisable at the profile's rate), so a refused profile changes nothing.
    ///
    /// # Errors
    /// `invalid` for a malformed profile or an unrealisable VFO, `locked` if the profile moves
    /// the tuning while a lock is held, `unsupported`/`device` from the device.
    pub fn load_profile(&self, payload: &[u8]) -> Result<Arc<str>, CtlError> {
        let wire: LoadWire = serde_json::from_slice(payload)
            .map_err(|e| CtlError::invalid(format!("malformed request: {e}")))?;
        if wire.v != VERSION {
            return Err(CtlError::invalid(format!(
                "unsupported protocol version {}",
                wire.v
            )));
        }
        let data = wire.profile;
        data.validate().map_err(CtlError::invalid)?;

        let mut locks = self.gate();
        let before = self.control.state();
        if locks.is_locked()
            && (data.center_hz != before.center_hz || data.sample_rate != before.sample_rate)
        {
            return Err(locks.error("tuning"));
        }
        let requests: Vec<VfoCreate> = data.vfos.iter().map(Into::into).collect();
        VfoRegistry::check_set(&requests, data.sample_rate)?;

        let tune = TuneRequest {
            center_hz: Some(data.center_hz),
            sample_rate: Some(data.sample_rate),
            gain: data.gain.and_then(device_gain),
            ppm: Some(data.ppm),
            bias_tee: Some(data.bias_tee),
        };
        if let Err(err) = self.control.apply(&tune) {
            tracing::warn!(error = %err, "profile load: device refused the settings");
            self.publish(&locks, &self.control.state());
            return Err(err.into());
        }
        let applied = self.control.state();
        let replaced = self.vfos.with(|r| r.replace(requests, applied.sample_rate));
        if let Err(err) = replaced {
            // Only possible if the device snapped the rate to something the VFOs cannot use.
            self.publish(&locks, &applied);
            return Err(err);
        }
        if data.lock {
            locks.acquire(Holder::user());
        }
        tracing::info!(
            center_hz = applied.center_hz,
            sample_rate = applied.sample_rate,
            vfos = data.vfos.len(),
            lock = data.lock,
            "profile loaded"
        );
        Ok(self.publish(&locks, &applied))
    }

    /// Restore tuning, VFOs and the user lock from a state document saved by an earlier run.
    ///
    /// Best effort per field: what the device refuses or the current hardware cannot do is
    /// skipped with a warning.
    ///
    /// # Errors
    /// A reason if the document does not belong to this device and driver (nothing is changed).
    pub fn restore(&self, doc: &serde_json::Value) -> Result<(), String> {
        if doc["v"] != VERSION {
            return Err("unsupported state version".into());
        }
        if doc["device"] != self.device.as_str() {
            return Err("saved state belongs to another device".into());
        }
        let mut locks = self.gate();
        let current = self.control.state();
        if doc["driver"] != current.driver {
            return Err(format!(
                "saved state is from driver {}, this is {}",
                doc["driver"], current.driver
            ));
        }
        let data = ProfileData::from_state(doc)?;

        let mut steps: Vec<(&str, TuneRequest)> = Vec::new();
        if data.center_hz != current.center_hz {
            steps.push((
                "center",
                TuneRequest {
                    center_hz: Some(data.center_hz),
                    ..TuneRequest::default()
                },
            ));
        }
        if data.sample_rate != current.sample_rate {
            steps.push((
                "sample rate",
                TuneRequest {
                    sample_rate: Some(data.sample_rate),
                    ..TuneRequest::default()
                },
            ));
        }
        if let Some(gain) = data.gain.and_then(device_gain)
            && Some(gain) != current.gain
        {
            steps.push((
                "gain",
                TuneRequest {
                    gain: Some(gain),
                    ..TuneRequest::default()
                },
            ));
        }
        if data.ppm != current.ppm {
            steps.push((
                "ppm",
                TuneRequest {
                    ppm: Some(data.ppm),
                    ..TuneRequest::default()
                },
            ));
        }
        if data.bias_tee != current.bias_tee {
            steps.push((
                "bias tee",
                TuneRequest {
                    bias_tee: Some(data.bias_tee),
                    ..TuneRequest::default()
                },
            ));
        }
        for (name, request) in steps {
            if let Err(e) = self.control.apply(&request) {
                tracing::warn!(setting = name, error = %e, "could not restore setting");
            }
        }

        let applied = self.control.state();
        let ids = doc["vfos"].as_array().map_or_else(Vec::new, |vfos| {
            vfos.iter()
                .map(|v| v["id"].as_u64().and_then(|id| u32::try_from(id).ok()))
                .collect::<Vec<_>>()
        });
        let saved: Vec<(u32, VfoCreate)> = ids
            .into_iter()
            .zip(&data.vfos)
            .filter_map(|(id, vfo)| Some((id?, vfo.into())))
            .collect();
        let wanted = saved.len();
        let dropped = self.vfos.with(|r| r.restore(saved, applied.sample_rate));
        if dropped > 0 {
            tracing::warn!(dropped, "some saved VFOs could not be restored");
        }
        if let Some(lock) = doc["locks"]
            .as_array()
            .and_then(|l| l.iter().find(|h| h["kind"] == "user"))
        {
            locks.acquire(Holder::user_since(lock["since_ms"].as_u64().unwrap_or(0)));
        }
        tracing::info!(
            center_hz = applied.center_hz,
            vfos = wanted - dropped,
            locked = locks.is_locked(),
            "state restored"
        );
        self.publish(&locks, &applied);
        Ok(())
    }

    /// Give a device that starts without saved state one VFO at the centre frequency (WFM inside
    /// the FM broadcast band, NFM elsewhere), so the first listener has something to tune.
    ///
    /// Only meant for the very first start: once the user has deleted every VFO the snapshot
    /// restores an empty set and this is not called.
    pub fn seed_default_vfo(&self) {
        let locks = self.gate();
        let state = self.control.state();
        let request = VfoCreate {
            freq_hz: state.center_hz,
            mode: default_mode(state.center_hz),
            bandwidth_hz: None,
            squelch_dbfs: None,
            deemphasis: None,
            cw_pitch_hz: None,
            stereo: None,
            tones: None,
            tone_squelch_hz: None,
        };
        match self.vfos.with(|r| {
            if r.list().is_empty() {
                r.create(request, state.sample_rate).map(Some)
            } else {
                Ok(None)
            }
        }) {
            Ok(Some(vfo)) => {
                tracing::info!(id = vfo.id, freq_hz = vfo.freq_hz, mode = ?vfo.mode, "default vfo created");
                self.publish(&locks, &state);
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e.message, "could not create the default vfo"),
        }
    }

    /// Error reply used when the blocking task of a command panicked.
    #[must_use]
    pub fn internal_error() -> Vec<u8> {
        error_reply(&CtlError::new(
            crate::control::ErrorCode::Device,
            "internal error",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ErrorCode;
    use serde_json::{Value, json};
    use superhet_device::Error;

    /// A device that accepts everything, like an RTL-SDR would.
    struct Mock {
        state: Mutex<DeviceState>,
    }

    impl Mock {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(DeviceState {
                    driver: "mock",
                    tuner: None,
                    center_hz: 100_000_000,
                    sample_rate: 1_024_000,
                    gain: Some(Gain::Auto),
                    ppm: 0,
                    bias_tee: false,
                    tuning_epoch: 0,
                    gains_db: vec![0.0, 20.7, 28.0, 40.2],
                }),
            })
        }
    }

    impl DeviceControl for Mock {
        fn state(&self) -> DeviceState {
            self.state.lock().expect("state").clone()
        }

        fn apply(&self, request: &TuneRequest) -> Result<DeviceState, Error> {
            let mut s = self.state.lock().expect("state");
            let before = s.clone();
            s.center_hz = request.center_hz.unwrap_or(s.center_hz);
            s.sample_rate = request.sample_rate.unwrap_or(s.sample_rate);
            s.gain = request.gain.or(s.gain);
            s.ppm = request.ppm.unwrap_or(s.ppm);
            s.bias_tee = request.bias_tee.unwrap_or(s.bias_tee);
            if (s.center_hz, s.sample_rate, s.gain, s.ppm)
                != (
                    before.center_hz,
                    before.sample_rate,
                    before.gain,
                    before.ppm,
                )
            {
                s.tuning_epoch += 1;
            }
            Ok(s.clone())
        }
    }

    fn controller(control: Arc<dyn DeviceControl>) -> Controller {
        let (tx, _rx) = watch::channel(None);
        let controller = Controller::new("dev0".into(), control, Arc::default(), tx);
        controller.refresh();
        controller
    }

    fn file_device(
        tag: &str,
    ) -> (
        Arc<dyn DeviceControl>,
        Box<dyn superhet_device::Device>,
        std::path::PathBuf,
    ) {
        let path =
            std::env::temp_dir().join(format!("superhet-ctl-{tag}-{}.cu8", std::process::id()));
        std::fs::write(&path, vec![128u8; 8192]).expect("fixture");
        let device = superhet_device::open_from_spec(&format!(
            "file:{}?format=cu8&rate=1000000&freq=100000000",
            path.display()
        ))
        .expect("open file device");
        (device.control(), device, path)
    }

    fn state(c: &Controller) -> Value {
        serde_json::from_str(&c.latest().expect("state")).expect("json")
    }

    fn vfo_call(c: &Controller, command: &str, body: Value) -> Result<Value, CtlError> {
        c.vfo(command, body.to_string().as_bytes())
            .map(|reply| serde_json::from_slice(&reply).expect("json"))
    }

    fn create(c: &Controller, freq_hz: u64) -> Value {
        vfo_call(
            c,
            "vfo.create",
            json!({"v": 1, "freq_hz": freq_hz, "mode": "nfm"}),
        )
        .expect("created")
    }

    fn tune(c: &Controller, request: &TuneRequest) -> Result<Arc<str>, CtlError> {
        c.tune(request)
    }

    fn center(hz: u64) -> TuneRequest {
        TuneRequest {
            center_hz: Some(hz),
            ..TuneRequest::default()
        }
    }

    #[test]
    fn free_tuning_follows_a_vfo_outside_the_window_on_a_file_device() {
        let (control, _device, path) = file_device("follow");
        let c = controller(control);
        // Inside the window (+-425 kHz): the centre stays.
        let reply = create(&c, 100_200_000);
        assert_eq!(reply["retune"], "none");
        assert_eq!(state(&c)["center_hz"], 100_000_000);
        // Far outside: the centre follows, the VFO sits a quarter window above it.
        let reply = create(&c, 103_000_000);
        assert_eq!(reply["retune"], "applied");
        assert_eq!(reply["vfo"]["status"], "active");
        let s = state(&c);
        assert_eq!(s["center_hz"], 103_000_000u64 - 212_500);
        assert_eq!(s["tuning_epoch"], 1);
        // The first VFO is outside the new window now and paused.
        assert_eq!(s["vfos"][0]["status"], "paused");
        // Moving the second VFO inside the window leaves the centre alone ...
        let id = reply["vfo"]["id"].clone();
        let reply = vfo_call(
            &c,
            "vfo.update",
            json!({"v": 1, "id": id, "freq_hz": 103_100_000u64}),
        )
        .expect("updated");
        assert_eq!(reply["retune"], "none");
        // ... dragging it below the window moves the centre down.
        let reply = vfo_call(
            &c,
            "vfo.update",
            json!({"v": 1, "id": id, "freq_hz": 102_000_000u64}),
        )
        .expect("updated");
        assert_eq!(reply["retune"], "applied");
        assert_eq!(state(&c)["center_hz"], 102_000_000u64 + 212_500);
        // Changing only the squelch of a VFO that is outside the window does not retune.
        let first = state(&c)["vfos"][0]["id"].clone();
        let reply = vfo_call(
            &c,
            "vfo.update",
            json!({"v": 1, "id": first, "squelch_dbfs": -50}),
        )
        .expect("updated");
        assert_eq!(reply["retune"], "none");
        assert_eq!(state(&c)["center_hz"], 102_000_000u64 + 212_500);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_lock_freezes_centre_and_rate_but_lets_vfos_move_in_the_window() {
        let (control, _device, path) = file_device("lock");
        let c = controller(control);
        create(&c, 100_100_000);
        c.set_user_lock(true);
        let s = state(&c);
        assert_eq!(s["locks"][0]["kind"], "user");

        // Centre and rate changes are refused, naming the holders; the state is untouched.
        let err = tune(&c, &center(101_000_000)).expect_err("locked");
        assert_eq!(err.code, ErrorCode::Locked);
        assert_eq!(err.holders.len(), 1);
        assert!(err.message.contains("user"), "{}", err.message);
        let err = tune(
            &c,
            &TuneRequest {
                sample_rate: Some(2_000_000),
                ..TuneRequest::default()
            },
        )
        .expect_err("locked");
        assert_eq!(err.code, ErrorCode::Locked);
        assert_eq!(state(&c)["center_hz"], 100_000_000);
        // Repeating the current centre is no change.
        tune(&c, &center(100_000_000)).expect("no-op");

        // A VFO may move inside the window ...
        let reply = create(&c, 100_300_000);
        assert_eq!(reply["retune"], "none");
        // ... and one dragged outside goes paused without a retune.
        let id = reply["vfo"]["id"].clone();
        let reply = vfo_call(
            &c,
            "vfo.update",
            json!({"v": 1, "id": id, "freq_hz": 105_000_000u64}),
        )
        .expect("updated");
        assert_eq!(reply["retune"], "locked");
        assert_eq!(reply["vfo"]["status"], "paused");
        let s = state(&c);
        assert_eq!(
            (s["center_hz"].clone(), s["tuning_epoch"].clone()),
            (100_000_000.into(), 0.into())
        );

        // Locking twice is harmless; releasing frees the tuning again.
        c.set_user_lock(true);
        assert_eq!(state(&c)["locks"].as_array().map(Vec::len), Some(1));
        c.set_user_lock(false);
        assert_eq!(state(&c)["locks"].as_array().map(Vec::len), Some(0));
        tune(&c, &center(101_000_000)).expect("unlocked");
        assert_eq!(state(&c)["center_hz"], 101_000_000);
        let _ = std::fs::remove_file(path);
    }

    /// A state with everything set to non-default values.
    fn dressed(c: &Controller) {
        tune(
            c,
            &TuneRequest {
                center_hz: Some(433_920_000),
                sample_rate: Some(2_048_000),
                gain: Some(Gain::Db(28.0)),
                ppm: Some(-3),
                bias_tee: Some(true),
            },
        )
        .expect("tune");
        let body = |extra: Value| {
            let mut v = json!({"v": 1});
            v.as_object_mut()
                .expect("obj")
                .extend(extra.as_object().expect("obj").clone());
            v
        };
        vfo_call(c, "vfo.create", body(json!({"freq_hz": 433_970_000u64, "mode": "nfm", "bandwidth_hz": 8000, "squelch_dbfs": -42.5, "deemphasis": "75us"}))).expect("vfo 1");
        vfo_call(
            c,
            "vfo.create",
            body(json!({"freq_hz": 433_500_000u64, "mode": "am"})),
        )
        .expect("vfo 2");
        vfo_call(
            c,
            "vfo.create",
            body(json!({"freq_hz": 434_400_000u64, "mode": "wfm", "bandwidth_hz": 150_000})),
        )
        .expect("vfo 3");
    }

    /// The parts of a state document a profile covers, ids and epoch left out.
    fn essence(c: &Controller) -> Value {
        let mut s = state(c);
        s["tuning_epoch"] = Value::Null;
        s["locks"] = json!(s["locks"].as_array().map_or(0, Vec::len));
        for vfo in s["vfos"].as_array_mut().expect("vfos") {
            vfo["id"] = Value::Null;
        }
        s
    }

    fn load(c: &Controller, data: &ProfileData) -> Result<Arc<str>, CtlError> {
        c.load_profile(json!({"v": 1, "profile": data}).to_string().as_bytes())
    }

    #[test]
    fn profile_round_trip_restores_everything() {
        let c = controller(Mock::new());
        dressed(&c);
        c.set_user_lock(true);
        let saved = essence(&c);
        let data = ProfileData::from_state(&state(&c)).expect("snapshot");
        assert!(data.lock && data.vfos.len() == 3);

        // Change everything.
        c.set_user_lock(false);
        tune(
            &c,
            &TuneRequest {
                center_hz: Some(145_000_000),
                sample_rate: Some(1_024_000),
                gain: Some(Gain::Auto),
                ppm: Some(5),
                bias_tee: Some(false),
            },
        )
        .expect("tune");
        for id in [1, 2, 3] {
            vfo_call(&c, "vfo.delete", json!({"v": 1, "id": id})).expect("delete");
        }
        create(&c, 145_100_000);
        assert_ne!(essence(&c), saved);

        // Load: the snapshot comes back exactly (VFOs get fresh ids), and the lock with it.
        load(&c, &data).expect("load");
        assert_eq!(essence(&c), saved);
        let ids: Vec<u64> = state(&c)["vfos"]
            .as_array()
            .expect("vfos")
            .iter()
            .map(|v| v["id"].as_u64().expect("id"))
            .collect();
        assert_eq!(ids, [5, 6, 7]);

        // Loading again is idempotent.
        load(&c, &data).expect("load again");
        assert_eq!(essence(&c), saved);
    }

    #[test]
    fn loading_replaces_the_vfos_instead_of_adding_to_them() {
        let c = controller(Mock::new());
        dressed(&c);
        let data = ProfileData::from_state(&state(&c)).expect("snapshot");
        // Extra VFOs the profile does not know about.
        for hz in [434_100_000, 434_200_000, 434_300_000] {
            create(&c, hz);
        }
        assert_eq!(state(&c)["vfos"].as_array().map(Vec::len), Some(6));
        let mut counts = Vec::new();
        for _ in 0..4 {
            load(&c, &data).expect("load");
            counts.push(state(&c)["vfos"].as_array().map(Vec::len));
        }
        assert_eq!(counts, [Some(3); 4]);

        // A profile without VFOs empties the set, one VFO is not smuggled back in.
        let mut empty = data;
        empty.vfos.clear();
        load(&c, &empty).expect("load empty");
        assert_eq!(state(&c)["vfos"], json!([]));
    }

    #[test]
    fn default_vfo_follows_the_band_and_only_fills_an_empty_set() {
        for (center_hz, mode) in [
            (100_000_000u64, "wfm"),
            (87_500_000, "wfm"),
            (108_000_000, "wfm"),
            (433_920_000, "nfm"),
            (87_000_000, "nfm"),
        ] {
            let c = controller(Mock::new());
            tune(&c, &center(center_hz)).expect("tune");
            c.seed_default_vfo();
            let s = state(&c);
            let vfos = s["vfos"].as_array().expect("vfos");
            assert_eq!(vfos.len(), 1, "{center_hz}");
            assert_eq!(vfos[0]["freq_hz"], center_hz);
            assert_eq!(vfos[0]["mode"], mode, "{center_hz}");
            assert_eq!(vfos[0]["status"], "active");
            // Not a second one.
            c.seed_default_vfo();
            assert_eq!(state(&c)["vfos"].as_array().map(Vec::len), Some(1));
        }
    }

    #[test]
    fn a_restored_state_without_vfos_stays_empty() {
        let c = controller(Mock::new());
        c.seed_default_vfo();
        vfo_call(&c, "vfo.delete", json!({"v": 1, "id": 1})).expect("delete");
        let saved = state(&c);
        assert_eq!(saved["vfos"], json!([]));
        // The daemon only seeds when no snapshot was applied; applying this one keeps it empty.
        let fresh = controller(Mock::new());
        fresh.restore(&saved).expect("restore");
        assert_eq!(state(&fresh)["vfos"], json!([]));
    }

    #[test]
    fn a_locked_device_refuses_a_profile_that_moves_the_tuning() {
        let c = controller(Mock::new());
        create(&c, 100_100_000);
        c.set_user_lock(true);
        let before = state(&c);
        let other = ProfileData {
            center_hz: 145_000_000,
            ..ProfileData::from_state(&before).expect("snapshot")
        };
        let err = load(&c, &other).expect_err("locked");
        assert_eq!(err.code, ErrorCode::Locked);
        assert_eq!(state(&c), before);
        // A profile that keeps centre and rate may still replace the VFOs.
        let same = ProfileData {
            vfos: Vec::new(),
            gain: Some(profile::Gain {
                mode: profile::GainMode::Manual,
                db: Some(28.0),
            }),
            ..ProfileData::from_state(&before).expect("snapshot")
        };
        load(&c, &same).expect("allowed");
        let s = state(&c);
        assert_eq!(s["vfos"].as_array().map(Vec::len), Some(0));
        assert_eq!(s["gain"]["db"], 28.0);
        assert_eq!(s["locks"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn a_profile_with_an_unrealisable_vfo_changes_nothing() {
        let c = controller(Mock::new());
        create(&c, 100_100_000);
        let before = state(&c);
        let mut data = ProfileData::from_state(&before).expect("snapshot");
        data.center_hz = 145_000_000;
        data.sample_rate = 250_000;
        data.vfos = vec![profile::Vfo {
            freq_hz: 145_000_000,
            mode: profile::Mode::Wfm,
            bandwidth_hz: Some(900_000),
            squelch_dbfs: None,
            deemphasis: None,
            cw_pitch_hz: None,
            stereo: None,
            tones: None,
            tone_squelch_hz: None,
        }];
        let err = load(&c, &data).expect_err("invalid");
        assert_eq!(err.code, ErrorCode::Invalid);
        assert_eq!(state(&c), before);
        // Malformed requests are invalid, too.
        for body in [
            "{}",
            r#"{"v":1,"profile":{"v":1}}"#,
            r#"{"v":2,"profile":{}}"#,
        ] {
            let err = c.load_profile(body.as_bytes()).expect_err(body);
            assert_eq!(err.code, ErrorCode::Invalid, "{body}");
        }
    }

    #[test]
    fn a_profile_on_a_file_device_applies_what_the_file_can_do() {
        let (control, _device, path) = file_device("profile");
        let c = controller(control);
        let data = ProfileData {
            center_hz: 98_000_000,
            vfos: vec![profile::Vfo {
                freq_hz: 98_100_000,
                mode: profile::Mode::Nfm,
                bandwidth_hz: None,
                squelch_dbfs: None,
                deemphasis: None,
                cw_pitch_hz: None,
                stereo: None,
                tones: None,
                tone_squelch_hz: None,
            }],
            lock: true,
            ..ProfileData::from_state(&state(&c)).expect("snapshot")
        };
        load(&c, &data).expect("load");
        let s = state(&c);
        assert_eq!(s["center_hz"], 98_000_000);
        assert_eq!(s["vfos"][0]["status"], "active");
        assert_eq!(s["locks"][0]["kind"], "user");
        // A rate the file cannot provide is refused as unsupported (after the lock is gone;
        // loading a profile never releases it).
        c.set_user_lock(false);
        let before = state(&c);
        let err = load(
            &c,
            &ProfileData {
                sample_rate: 2_000_000,
                lock: false,
                ..data
            },
        )
        .expect_err("unsupported");
        assert_eq!(err.code, ErrorCode::Unsupported);
        assert_eq!(state(&c), before);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn saved_state_is_restored_with_ids_and_lock() {
        let old = controller(Mock::new());
        dressed(&old);
        vfo_call(&old, "vfo.delete", json!({"v": 1, "id": 2})).expect("delete");
        old.set_user_lock(true);
        let saved = state(&old);

        // A new run on a device with different settings.
        let fresh = controller(Mock::new());
        fresh.restore(&saved).expect("restore");
        let mut restored = state(&fresh);
        assert_eq!(restored["center_hz"], 433_920_000);
        assert_eq!(restored["locks"], saved["locks"]);
        // The epoch differs (the new device was retuned), everything else is identical.
        restored["tuning_epoch"] = saved["tuning_epoch"].clone();
        assert_eq!(restored, saved);
        // Ids survive and new VFOs continue after the highest one.
        let reply = create(&fresh, 433_600_000);
        assert_eq!(reply["vfo"]["id"], 4);
    }

    #[test]
    fn saved_state_of_another_device_or_driver_is_ignored() {
        let old = controller(Mock::new());
        dressed(&old);
        let saved = state(&old);
        let fresh = controller(Mock::new());
        let before = state(&fresh);

        let mut other_device = saved.clone();
        other_device["device"] = "elsewhere".into();
        assert!(fresh.restore(&other_device).is_err());
        let mut other_driver = saved.clone();
        other_driver["driver"] = "rtlsdr".into();
        assert!(fresh.restore(&other_driver).is_err());
        assert!(fresh.restore(&json!({"v": 1})).is_err());
        assert!(fresh.restore(&json!({"v": 9})).is_err());
        assert_eq!(state(&fresh), before);
    }

    #[test]
    fn restore_skips_what_the_device_cannot_do_and_drops_unrealisable_vfos() {
        // Saved on a "mock" device, restored on a file device that has the same driver name
        // only by design of this test: emulate by editing the driver.
        let (control, _device, path) = file_device("restore");
        let c = controller(control);
        let mut doc = state(&c);
        doc["center_hz"] = 99_000_000.into();
        doc["ppm"] = 4.into(); // the file device has no ppm: skipped
        doc["vfos"] = json!([
            {"id": 3, "freq_hz": 99_100_000u64, "mode": "nfm", "bandwidth_hz": 12_500, "squelch_dbfs": null, "deemphasis": "none"},
            {"id": 3, "freq_hz": 99_200_000u64, "mode": "nfm", "bandwidth_hz": 12_500, "squelch_dbfs": null, "deemphasis": "none"},
            {"id": 4, "freq_hz": 99_300_000u64, "mode": "wfm", "bandwidth_hz": 900_000, "squelch_dbfs": null, "deemphasis": "50us"},
        ]);
        c.restore(&doc).expect("restore");
        let s = state(&c);
        assert_eq!(s["center_hz"], 99_000_000);
        assert_eq!(s["ppm"], 0);
        assert_eq!(s["vfos"].as_array().map(Vec::len), Some(1));
        assert_eq!(s["vfos"][0]["freq_hz"], 99_100_000);
        let _ = std::fs::remove_file(path);
    }
}
