//! The scanner thread: takes spectrum frames from the probe, runs the [`Machine`], and carries
//! out its actions through the [`Controller`] (retune, move the VFO) while it holds the tuning.

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use superhet_bus::spectrum::SpectrumFrame;

use super::machine::{Action, Hit, Machine};
use super::plan::{self, Channel, Frame};
use super::wire::{Command, Start, parse_command};
use crate::control::{CtlError, ErrorCode};
use crate::controller::Controller;

/// Hits kept in the state document.
pub const RECENT_HITS: usize = 25;
/// How long a command waits for the scanner thread.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// Wake-up interval while scanning (the frame rate is the real clock).
const BUSY_TICK: Duration = Duration::from_millis(10);
/// Wake-up interval while idle (to notice a preemption by a plugin claim).
const IDLE_TICK: Duration = Duration::from_millis(250);
/// While scanning, progress (window, pass) is published at most this often.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// Takes the published spectrum frames while a scan runs. The DSP thread offers every encoded
/// frame (the same bytes the browsers get); nothing is decoded while the scanner is off.
#[derive(Default)]
pub struct ScanProbe {
    active: AtomicBool,
    seq: AtomicU64,
    slot: Mutex<Option<Arc<Frame>>>,
}

impl ScanProbe {
    /// Switch frame capture on or off.
    pub fn set_active(&self, on: bool) {
        self.active.store(on, Ordering::Release);
        if !on {
            *self.slot() = None;
        }
    }

    fn slot(&self) -> MutexGuard<'_, Option<Arc<Frame>>> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Offer an encoded `SpectrumFrame`; a no-op unless a scan is running.
    pub fn offer(&self, bytes: &[u8]) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        let Ok(frame) = SpectrumFrame::decode(bytes) else {
            return;
        };
        let db = (0..frame.bins.len())
            .map(|i| frame.db(i).unwrap_or(-200.0))
            .collect();
        let seq = self.seq.fetch_add(1, Ordering::AcqRel) + 1;
        *self.slot() = Some(Arc::new(Frame {
            seq,
            center_hz: frame.center_hz,
            sample_rate: frame.sample_rate,
            db,
        }));
    }

    /// The newest frame if it is newer than `seen`.
    fn take_new(&self, seen: u64) -> Option<Arc<Frame>> {
        self.slot().clone().filter(|f| f.seq > seen)
    }
}

/// Receives the JSON of each hit (published on `sh.v1.dev.<dev>.scanner.hit`).
pub type HitSink = Box<dyn Fn(Vec<u8>) + Send + Sync>;

struct Message {
    command: Command,
    reply: Sender<Result<(), CtlError>>,
}

/// Handle to the scanner thread.
pub struct Scanner {
    tx: Sender<Message>,
    controller: Arc<Controller>,
}

impl Scanner {
    /// Start the scanner thread for the device `device`.
    #[must_use]
    pub fn spawn(
        controller: Arc<Controller>,
        probe: Arc<ScanProbe>,
        device: String,
        hits: HitSink,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Message>();
        let mut ctx = Ctx {
            controller: Arc::clone(&controller),
            probe,
            device,
            sink: hits,
            hits: VecDeque::new(),
            lockouts: BTreeSet::new(),
            stopped: None,
        };
        let spawned = std::thread::Builder::new()
            .name("sdrd-scanner".into())
            .spawn(move || run(&mut ctx, &rx));
        if let Err(err) = spawned {
            tracing::error!(%err, "cannot start the scanner thread");
        }
        Self { tx, controller }
    }

    /// Run `ctl.scanner.<name>` and return the new state document.
    ///
    /// Blocking; call it from a blocking thread.
    ///
    /// # Errors
    /// `invalid` for a bad request, `locked` when the tuning is held by a user or a plugin,
    /// `not_found` for an unknown VFO.
    pub fn execute(&self, name: &str, payload: &[u8]) -> Result<Arc<str>, CtlError> {
        let command = parse_command(name, payload)?;
        let (reply, answer) = mpsc::channel();
        self.tx
            .send(Message { command, reply })
            .map_err(|_| CtlError::new(ErrorCode::Device, "scanner is not running"))?;
        match answer.recv_timeout(COMMAND_TIMEOUT) {
            Ok(result) => result?,
            Err(_) => {
                return Err(CtlError::new(
                    ErrorCode::Device,
                    "the scanner did not answer in time",
                ));
            }
        }
        self.controller
            .latest()
            .ok_or_else(|| CtlError::new(ErrorCode::Device, "state not available yet"))
    }
}

/// What lives across scans.
struct Ctx {
    controller: Arc<Controller>,
    probe: Arc<ScanProbe>,
    device: String,
    sink: HitSink,
    /// Newest first.
    hits: VecDeque<Value>,
    lockouts: BTreeSet<u64>,
    /// Why the last scan ended (shown while idle).
    stopped: Option<&'static str>,
}

struct Session {
    machine: Machine,
    vfo: u32,
    config: Value,
    started_ms: u64,
    seen_seq: u64,
    last_revision: u64,
    last_progress: Instant,
    last_view: Value,
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn run(ctx: &mut Ctx, rx: &mpsc::Receiver<Message>) {
    let mut session: Option<Session> = None;
    loop {
        let wait = if session.is_some() {
            BUSY_TICK
        } else {
            IDLE_TICK
        };
        match rx.recv_timeout(wait) {
            Ok(Message { command, reply }) => {
                let result = handle(ctx, &mut session, command);
                let _ = reply.send(result);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if ctx.controller.scan_preempted() && session.is_some() {
            tracing::info!("scanner stopped: an exclusive plugin claimed the device");
            end(ctx, &mut session, "claimed");
        }
        let Some(s) = session.as_mut() else {
            continue;
        };
        let frame = ctx.probe.take_new(s.seen_seq);
        if let Some(f) = &frame {
            s.seen_seq = f.seq;
        }
        let actions = s.machine.step(Instant::now(), frame.as_deref());
        if let Some(reason) = carry_out(ctx, s, actions) {
            end(ctx, &mut session, reason);
            continue;
        }
        if let Some(s) = session.as_mut() {
            publish_if_changed(ctx, s, false);
        }
    }
    if session.is_some() {
        end(ctx, &mut session, "shutdown");
    }
}

fn handle(ctx: &mut Ctx, session: &mut Option<Session>, command: Command) -> Result<(), CtlError> {
    let now = Instant::now();
    match command {
        Command::Start(start) => start_scan(ctx, session, *start),
        Command::Stop => {
            if session.is_some() {
                end(ctx, session, "stopped");
            }
            Ok(())
        }
        Command::Pause | Command::Resume | Command::Skip => {
            let Some(s) = session.as_mut() else {
                return Err(CtlError::invalid("the scanner is not running"));
            };
            let actions = match command {
                Command::Pause => {
                    s.machine.pause();
                    Vec::new()
                }
                Command::Resume => s.machine.resume(now),
                _ => s.machine.skip(now),
            };
            finish_command(ctx, session, actions)
        }
        Command::Lockout(freq) => {
            match session.as_mut() {
                Some(s) => {
                    let Some(freq) =
                        freq.or_else(|| s.machine.held().map(|c| s.machine.channels()[c].freq_hz))
                    else {
                        return Err(CtlError::invalid(
                            "lockout needs freq_hz while no channel is held",
                        ));
                    };
                    let actions = s.machine.lockout(freq, now);
                    ctx.lockouts = s.machine.lockouts().clone();
                    finish_command(ctx, session, actions)?;
                }
                None => {
                    let Some(freq) = freq else {
                        return Err(CtlError::invalid("lockout needs freq_hz"));
                    };
                    if ctx.lockouts.len() >= super::wire::MAX_LOCKOUTS {
                        return Err(CtlError::invalid("too many locked-out frequencies"));
                    }
                    ctx.lockouts.insert(freq);
                    publish_idle(ctx);
                }
            }
            Ok(())
        }
        Command::Unlock(freq) => {
            match session.as_mut() {
                Some(s) => {
                    let actions = s.machine.unlock(freq, now);
                    ctx.lockouts = s.machine.lockouts().clone();
                    finish_command(ctx, session, actions)?;
                }
                None => {
                    match freq {
                        Some(f) => {
                            ctx.lockouts.remove(&f);
                        }
                        None => ctx.lockouts.clear(),
                    }
                    publish_idle(ctx);
                }
            }
            Ok(())
        }
    }
}

/// Carry out what a command made the machine ask for, then publish.
fn finish_command(
    ctx: &mut Ctx,
    session: &mut Option<Session>,
    actions: Vec<Action>,
) -> Result<(), CtlError> {
    let Some(s) = session.as_mut() else {
        return Ok(());
    };
    if let Some(reason) = carry_out(ctx, s, actions) {
        end(ctx, session, reason);
        return Ok(());
    }
    if let Some(s) = session.as_mut() {
        publish_if_changed(ctx, s, true);
    }
    Ok(())
}

fn start_scan(ctx: &mut Ctx, session: &mut Option<Session>, start: Start) -> Result<(), CtlError> {
    if session.is_some() {
        end(ctx, session, "restarted");
    }
    let ids = ctx.controller.scan_vfo_ids();
    let vfo = match start.vfo {
        Some(id) if ids.contains(&id) => id,
        Some(id) => {
            return Err(CtlError::new(ErrorCode::NotFound, format!("no VFO {id}")));
        }
        None => *ids.iter().min().ok_or_else(|| {
            CtlError::invalid("the scanner needs a VFO to listen with; create one first")
        })?,
    };
    let begin = ctx.controller.scan_begin()?;
    let segments = match plan::plan(&start.channels, begin.center_hz, begin.sample_rate) {
        Ok(segments) => segments,
        Err(err) => {
            ctx.controller.scan_end();
            return Err(CtlError::invalid(err.to_string()));
        }
    };
    if let Some(locked) = begin.retune_blocked
        && !segments
            .iter()
            .any(|s| s.center_hz.abs_diff(begin.center_hz) <= 1_000)
    {
        ctx.controller.scan_end();
        return Err(locked);
    }
    ctx.lockouts.extend(start.lockouts.iter().copied());
    ctx.probe.set_active(true);
    let now = Instant::now();
    let (machine, actions) = Machine::new(
        start.channels,
        segments,
        start.params,
        ctx.lockouts.clone(),
        begin.center_hz,
        now,
    );
    tracing::info!(
        vfo,
        channels = machine.channels().len(),
        windows = machine.windows(),
        "scanner started"
    );
    ctx.stopped = None;
    let mut s = Session {
        machine,
        vfo,
        config: start.echo,
        started_ms: unix_ms(),
        seen_seq: 0,
        last_revision: 0,
        last_progress: now,
        last_view: Value::Null,
    };
    if let Some(reason) = carry_out(ctx, &mut s, actions) {
        *session = Some(s);
        end(ctx, session, reason);
        return Ok(());
    }
    publish_if_changed(ctx, &mut s, true);
    *session = Some(s);
    Ok(())
}

/// Stop the scan, give the tuning back and show the idle state.
fn end(ctx: &mut Ctx, session: &mut Option<Session>, reason: &'static str) {
    if let Some(s) = session.take() {
        ctx.lockouts = s.machine.lockouts().clone();
        tracing::info!(reason, "scanner stopped");
    }
    ctx.probe.set_active(false);
    ctx.controller.scan_end();
    ctx.stopped = Some(reason);
    publish_idle(ctx);
}

fn publish_idle(ctx: &Ctx) {
    let mut view = json!({"state": "idle"});
    if let Some(reason) = ctx.stopped {
        view["stopped"] = json!(reason);
    }
    view["lockouts"] = json!(ctx.lockouts);
    view["hits"] = json!(ctx.hits);
    ctx.controller.scan_publish(Some(view));
}

fn compose(ctx: &Ctx, s: &Session) -> Value {
    let mut view = s.machine.view();
    view["vfo"] = json!(s.vfo);
    view["config"] = s.config.clone();
    view["started_ms"] = json!(s.started_ms);
    view["lockouts"] = json!(s.machine.lockouts());
    view["hits"] = json!(ctx.hits);
    view
}

fn publish_if_changed(ctx: &Ctx, s: &mut Session, force: bool) {
    let revision = s.machine.revision();
    let progress_due = s.last_progress.elapsed() >= PROGRESS_EVERY;
    if !force && revision == s.last_revision && !progress_due {
        return;
    }
    let view = compose(ctx, s);
    if view == s.last_view {
        return;
    }
    s.last_revision = revision;
    s.last_progress = Instant::now();
    s.last_view = view.clone();
    ctx.controller.scan_publish(Some(view));
}

/// Execute the machine's requests. Returns why the scan has to end, if it has.
fn carry_out(ctx: &mut Ctx, s: &mut Session, actions: Vec<Action>) -> Option<&'static str> {
    let mut queue: VecDeque<Action> = actions.into();
    let mut budget = 16;
    while let Some(action) = queue.pop_front() {
        budget -= 1;
        if budget == 0 {
            tracing::warn!("scanner: too many chained actions; dropping the rest");
            break;
        }
        let now = Instant::now();
        match action {
            Action::Retune { center_hz } => match ctx.controller.scan_retune(center_hz) {
                Ok(applied) => s.machine.retune_done(applied),
                Err(err) => {
                    tracing::info!(error = %err.message, "scanner cannot retune; staying in this window");
                    queue.extend(s.machine.retune_refused(now));
                }
            },
            Action::Hit(hit) => match move_vfo(ctx, s, hit) {
                Ok(()) => {}
                Err(err) if err.code == ErrorCode::NotFound => return Some("vfo_removed"),
                Err(err) => {
                    tracing::warn!(error = %err.message, "scanner cannot move the VFO");
                    queue.extend(s.machine.hit_failed(now));
                }
            },
        }
    }
    None
}

fn move_vfo(ctx: &mut Ctx, s: &Session, hit: Hit) -> Result<(), CtlError> {
    let channel: &Channel = &s.machine.channels()[hit.channel];
    ctx.controller.scan_move_vfo(s.vfo, channel)?;
    let record = json!({
        "ts_ms": unix_ms(),
        "vfo": s.vfo,
        "freq_hz": channel.freq_hz,
        "mode": channel.mode,
        "bandwidth_hz": channel.vfo_bandwidth_hz(),
        "label": channel.label,
        "level_dbfs": round1(hit.level.dbfs),
        "margin_db": round1(hit.level.margin_db),
        "priority": channel.priority,
    });
    let mut event = record.clone();
    event["v"] = json!(1);
    event["device"] = json!(ctx.device);
    event["source"] = s.config["kind"].clone();
    event["list"] = s.config["name"].clone();
    (ctx.sink)(event.to_string().into_bytes());
    ctx.hits.push_front(record);
    ctx.hits.truncate(RECENT_HITS);
    Ok(())
}

fn round1(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SpectrumSettings;
    use crate::pipeline::Pipeline;
    use crate::vfo::VfoShared;
    use std::io::Write;
    use superhet_bus::tap::ClaimAcquire;

    const RATE: u32 = 1_024_000;
    const CENTER: u64 = 145_000_000;
    const A_HZ: u64 = CENTER + 100_000;
    const B_HZ: u64 = CENTER - 200_000;
    const C_HZ: u64 = CENTER + 300_000;

    /// Two seconds of IQ that loop seamlessly: carrier B (-200 kHz) always on, carrier A
    /// (+100 kHz) on from 0.05 s to 0.95 s of each period (raised-cosine keying), a little
    /// noise. Nothing at +300 kHz.
    fn write_band(tag: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("superhet-scanner-{tag}-{}.cu8", std::process::id()));
        let samples = 2 * RATE as usize;
        let mut bytes = Vec::with_capacity(samples * 2);
        let mut noise = 12_345_u32;
        let mut rand = move || {
            noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (f64::from(noise >> 8) / f64::from(1_u32 << 24) - 0.5) * 0.006
        };
        for i in 0..samples {
            let t = i as f64 / f64::from(RATE);
            let key = if !(0.05..=0.95).contains(&t) {
                0.0
            } else {
                let edge = ((t - 0.05).min(0.95 - t) / 0.02).min(1.0);
                0.5 + 0.5 * (std::f64::consts::PI * (1.0 - edge)).cos()
            };
            let (mut re, mut im) = (rand(), rand());
            for (offset, amp) in [(100_000.0, 0.4 * key), (-200_000.0, 0.4)] {
                let ph = std::f64::consts::TAU * offset * t;
                re += amp * ph.cos();
                im += amp * ph.sin();
            }
            bytes.push((127.5 + 127.0 * re).round().clamp(0.0, 255.0) as u8);
            bytes.push((127.5 + 127.0 * im).round().clamp(0.0, 255.0) as u8);
        }
        std::fs::File::create(&path)
            .and_then(|mut f| f.write_all(&bytes))
            .expect("write fixture");
        path
    }

    struct Rig {
        pipeline: Option<Pipeline>,
        controller: Arc<Controller>,
        scanner: Scanner,
        hits: Arc<Mutex<Vec<Value>>>,
        path: std::path::PathBuf,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            drop(self.pipeline.take());
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn rig(tag: &str) -> Rig {
        let path = write_band(tag);
        let spec = format!(
            "file:{}?format=cu8&rate={RATE}&freq={CENTER}&block=4096",
            path.display()
        );
        let device = superhet_device::open_from_spec(&spec).expect("device");
        let probe = Arc::new(ScanProbe::default());
        let settings = SpectrumSettings {
            fft_size: 4096,
            bins: 1024,
            fps: 25.0,
            min_db: -120.0,
            max_db: 0.0,
        };
        let feed = Arc::clone(&probe);
        let pipeline = Pipeline::start(device, &settings, move |f| feed.offer(&f), None, None)
            .expect("pipeline");
        let (tx, _rx) = tokio::sync::watch::channel(None);
        let controller = Arc::new(Controller::new(
            "dev0".into(),
            pipeline.control(),
            Arc::new(VfoShared::with_max(8)),
            tx,
        ));
        controller
            .vfo("vfo.create", br#"{"v":1,"freq_hz":145000000,"mode":"nfm"}"#)
            .expect("vfo");
        controller.refresh();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let sink = {
            let hits = Arc::clone(&hits);
            Box::new(move |bytes: Vec<u8>| {
                if let Ok(v) = serde_json::from_slice(&bytes) {
                    hits.lock().expect("hits").push(v);
                }
            })
        };
        let scanner = Scanner::spawn(Arc::clone(&controller), probe, "dev0".into(), sink);
        Rig {
            pipeline: Some(pipeline),
            controller,
            scanner,
            hits,
            path,
        }
    }

    impl Rig {
        fn state(&self) -> Value {
            serde_json::from_str(&self.controller.latest().expect("state")).expect("json")
        }

        fn scanner_state(&self) -> String {
            self.state()["scanner"]["state"]
                .as_str()
                .unwrap_or("none")
                .to_owned()
        }

        fn wait(&self, what: &str, secs: u64, ok: impl Fn(&Value) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < deadline {
                if ok(&self.state()) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!(
                "timed out waiting for {what}; scanner: {}",
                self.state()["scanner"]
            );
        }

        fn start(&self, body: &str) {
            self.scanner
                .execute("start", body.as_bytes())
                .expect("start");
        }

        fn command(&self, name: &str, body: &str) {
            self.scanner
                .execute(name, body.as_bytes())
                .expect("command");
        }
    }

    fn list(freqs: &[u64], extra: &str) -> String {
        let channels: Vec<String> = freqs
            .iter()
            .map(|f| format!(r#"{{"freq_hz":{f},"mode":"nfm","label":"ch {f}"}}"#))
            .collect();
        format!(
            r#"{{"v":1,"source":{{"kind":"list","name":"test","channels":[{}]}},"dwell_ms":100{extra}}}"#,
            channels.join(",")
        )
    }

    fn holding(freq: u64) -> impl Fn(&Value) -> bool {
        move |s: &Value| {
            s["scanner"]["state"] == "holding" && s["scanner"]["current"]["freq_hz"] == freq
        }
    }

    #[test]
    fn stops_on_a_carrier_then_lockout_unlock_and_stop() {
        let rig = rig("lockout");
        rig.start(&list(&[B_HZ, C_HZ], r#","hang_ms":300"#));
        rig.wait("hold on B", 10, holding(B_HZ));
        let state = rig.state();
        // The scanner holds the tuning lock and moved the VFO onto the carrier.
        assert_eq!(state["locks"][0]["kind"], "scanner");
        assert_eq!(state["vfos"][0]["freq_hz"], B_HZ);
        assert_eq!(state["scanner"]["hits"][0]["freq_hz"], B_HZ);
        assert!(!rig.hits.lock().expect("hits").is_empty());
        // Nobody else may retune meanwhile.
        let err = rig
            .controller
            .tune(&superhet_device::TuneRequest {
                center_hz: Some(100_000_000),
                ..superhet_device::TuneRequest::default()
            })
            .expect_err("locked");
        assert_eq!(err.code, ErrorCode::Locked);

        // Locking out the held channel releases it; nothing else qualifies.
        rig.command("lockout", r#"{"v":1}"#);
        rig.wait("scanning again", 5, |s| s["scanner"]["state"] == "scanning");
        assert_eq!(rig.state()["scanner"]["lockouts"][0], B_HZ);
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(rig.scanner_state(), "scanning");
        // Unlocking brings it back.
        rig.command("unlock", r#"{"v":1,"all":true}"#);
        rig.wait("hold on B again", 10, holding(B_HZ));

        rig.command("stop", r#"{"v":1}"#);
        let state = rig.state();
        assert_eq!(state["scanner"]["state"], "idle");
        assert_eq!(state["scanner"]["stopped"], "stopped");
        assert!(state["locks"].as_array().is_some_and(Vec::is_empty));
        // The hit list survives the stop.
        assert!(
            state["scanner"]["hits"]
                .as_array()
                .is_some_and(|h| h.len() >= 2)
        );
    }

    #[test]
    fn holds_while_the_carrier_is_on_and_resumes_after_the_hang_time() {
        let rig = rig("hang");
        rig.start(&list(&[A_HZ, C_HZ], r#","hang_ms":300"#));
        // Carrier A is on for 0.9 s of every 2 s: found, released, found again.
        rig.wait("first hold on A", 10, holding(A_HZ));
        rig.wait("resume after hang", 10, |s| {
            s["scanner"]["state"] == "scanning"
        });
        rig.wait("second hold on A", 10, |s| {
            s["scanner"]["state"] == "holding"
                && s["scanner"]["hits"]
                    .as_array()
                    .is_some_and(|h| h.len() >= 2)
        });
        assert!(rig.hits.lock().expect("hits").len() >= 2);
        rig.command("stop", r#"{"v":1}"#);
    }

    #[test]
    fn pause_freezes_and_the_hold_mode_waits_for_the_user() {
        let rig = rig("hold");
        rig.start(&list(&[B_HZ, C_HZ], r#","resume":"hold""#));
        rig.wait("hold on B", 10, holding(B_HZ));
        rig.command("pause", r#"{"v":1}"#);
        assert_eq!(rig.scanner_state(), "paused");
        rig.command("resume", r#"{"v":1}"#);
        assert_eq!(rig.scanner_state(), "holding");
        // Carrier B never goes away and the mode is "hold": only the user lets go.
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(rig.scanner_state(), "holding");
        rig.command("skip", r#"{"v":1}"#);
        rig.wait("scanning after skip", 5, |s| {
            s["scanner"]["state"] == "scanning"
        });
        rig.command("stop", r#"{"v":1}"#);
    }

    #[test]
    fn an_exclusive_claim_ends_the_scan_and_a_claim_blocks_a_new_one() {
        let rig = rig("claim");
        rig.start(&list(&[B_HZ], ""));
        rig.wait("hold on B", 10, holding(B_HZ));
        let claim = ClaimAcquire {
            v: 1,
            id: "i-1".into(),
            center_hz: 1_090_000_000,
            sample_rate: None,
        };
        rig.controller
            .claim_acquire(&claim, Duration::from_secs(30))
            .expect("the claim wins over the scanner");
        rig.wait("scan ended", 5, |s| {
            s["scanner"]["state"] == "idle" && s["scanner"]["stopped"] == "claimed"
        });
        let err = rig
            .scanner
            .execute("start", list(&[B_HZ], "").as_bytes())
            .expect_err("claimed");
        assert_eq!(err.code, ErrorCode::Locked);
    }

    #[test]
    fn a_user_lock_keeps_the_scan_inside_the_current_window() {
        let rig = rig("userlock");
        rig.controller
            .change_lock(&crate::control::LockRequest::Acquire {
                id: "u:1".into(),
                name: None,
            });
        // Nothing in the current window: refused up front.
        let err = rig
            .scanner
            .execute("start", list(&[433_500_000], "").as_bytes())
            .expect_err("locked");
        assert_eq!(err.code, ErrorCode::Locked);
        // Channels in the window are fine.
        rig.start(&list(&[B_HZ, 433_500_000], ""));
        rig.wait("hold on B", 10, holding(B_HZ));
        rig.command("stop", r#"{"v":1}"#);
        // The user's lock is untouched.
        assert_eq!(rig.state()["locks"][0]["id"], "u:1");
    }
}
