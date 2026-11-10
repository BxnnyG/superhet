//! The scan state machine: pure, driven by time and spectrum frames, no I/O.
//!
//! The scanner watches one window (one device centre) at a time. All channels of the window are
//! measured on the same spectrum frame, so a scan inside the current span needs no retune and
//! the dwell time is per window, not per channel. A window is left when no channel stands out;
//! otherwise the scanner holds on the first one (priority channel first, then ascending
//! frequency) until the resume rule lets go.
//!
//! Phases: `Settling` (retuned, waiting for the first frame of the new centre), `Dwell`
//! (measuring), `Holding` (stopped on a channel; `signal` while it is above the threshold,
//! `hang` while it is gone and the hang time runs), `Blocked` (nothing can be scanned right
//! now: every channel locked out, or the tuning lock keeps the scanner from leaving the
//! window and nothing in it qualifies).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::plan::{Channel, Frame, Level, Measure, Segment};

/// A held channel stays held while it is within this many dB of the threshold.
pub const HYSTERESIS_DB: f32 = 3.0;
/// A skipped channel is ignored for this long.
pub const SNOOZE: Duration = Duration::from_secs(30);
/// Consecutive frames a channel has to stand out before the scanner stops on it.
const CONFIRM_FRAMES: u32 = 2;
/// A retune that shows no frame of the new centre this long is repeated.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(3);
/// After a refused retune the scanner stays in the window for this long before it tries again.
const RESTRICT_RETRY: Duration = Duration::from_secs(2);
/// Centres this close count as the same (a tuner may snap the requested centre).
const SAME_CENTER_HZ: u64 = 1_000;

fn same_center(a: u64, b: u64) -> bool {
    a.abs_diff(b) <= SAME_CENTER_HZ
}

/// When a held channel is let go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Once the signal has been gone for the hang time.
    Hang,
    /// A fixed time after the stop, whatever the signal does.
    Timeout,
    /// Never on its own (resume or skip from the user).
    Hold,
}

/// Scan parameters.
#[derive(Debug, Clone)]
pub struct Params {
    /// dB over the noise floor a channel has to reach.
    pub threshold_db: f32,
    /// Absolute level (dBFS) that replaces the relative rule when set.
    pub threshold_dbfs: Option<f32>,
    /// Observation time per window before a stop is decided.
    pub dwell: Duration,
    /// Hang time for [`Resume::Hang`].
    pub hang: Duration,
    /// Resume rule.
    pub resume: Resume,
    /// Hold time for [`Resume::Timeout`].
    pub timeout: Duration,
    /// How often the priority channel is checked.
    pub priority_interval: Duration,
}

impl Params {
    fn is_hit(&self, level: Level) -> bool {
        match self.threshold_dbfs {
            Some(abs) => level.dbfs >= abs,
            None => level.margin_db >= self.threshold_db,
        }
    }

    fn still_holds(&self, level: Level) -> bool {
        match self.threshold_dbfs {
            Some(abs) => level.dbfs >= abs - HYSTERESIS_DB,
            None => level.margin_db >= self.threshold_db - HYSTERESIS_DB,
        }
    }
}

/// A stop on a channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// Index into the channel list.
    pub channel: usize,
    /// What was measured when the scanner stopped.
    pub level: Level,
}

/// What the machine asks the runtime to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// Move the device centre; answer with [`Machine::retune_done`] or
    /// [`Machine::retune_refused`].
    Retune {
        /// Wanted centre, Hz.
        center_hz: u64,
    },
    /// Put the VFO on the channel and report the hit; on failure call [`Machine::hit_failed`].
    Hit(Hit),
}

/// Why nothing is being scanned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// Every channel is locked out (or skipped for now).
    NothingToScan,
    /// The tuning is locked and the window the scanner is in has nothing to scan.
    Locked,
}

impl Blocked {
    fn as_str(self) -> &'static str {
        match self {
            Self::NothingToScan => "nothing_to_scan",
            Self::Locked => "locked",
        }
    }
}

#[derive(Debug, Clone)]
enum Phase {
    Settling {
        target: u64,
        since: Instant,
    },
    Dwell {
        since: Instant,
        /// The channel that stood out in the last frames and for how many in a row.
        streak: Option<(usize, u32)>,
    },
    Holding {
        channel: usize,
        since: Instant,
        last_signal: Instant,
        signal: bool,
        level: Level,
    },
    Blocked(Blocked),
}

/// The scan state machine, see the module docs.
pub struct Machine {
    channels: Vec<Channel>,
    segments: Vec<Segment>,
    params: Params,
    lockouts: BTreeSet<u64>,
    device_center_hz: u64,
    cur: usize,
    phase: Phase,
    paused: bool,
    pass: u64,
    /// Channels already held in the current window visit.
    served: HashSet<usize>,
    snoozed: HashMap<usize, Instant>,
    /// The segment to continue with after a priority visit.
    after_priority: Option<usize>,
    last_priority: Instant,
    priority_segment: Option<usize>,
    /// The tuning lock refused a retune: stay in the windows reachable without one until then.
    restricted: Option<Instant>,
    revision: u64,
}

impl Machine {
    /// Start scanning at `now`. `device_center_hz` is the current centre; `segments` come from
    /// [`super::plan::plan`] for that centre. Returns the machine and its first actions.
    #[must_use]
    pub fn new(
        channels: Vec<Channel>,
        segments: Vec<Segment>,
        params: Params,
        lockouts: BTreeSet<u64>,
        device_center_hz: u64,
        now: Instant,
    ) -> (Self, Vec<Action>) {
        let priority_segment = channels
            .iter()
            .position(|c| c.priority)
            .and_then(|p| segments.iter().position(|s| s.channels.contains(&p)));
        let mut machine = Self {
            channels,
            segments,
            params,
            lockouts,
            device_center_hz,
            cur: 0,
            phase: Phase::Blocked(Blocked::NothingToScan),
            paused: false,
            pass: 1,
            served: HashSet::new(),
            snoozed: HashMap::new(),
            after_priority: None,
            last_priority: now,
            priority_segment,
            restricted: None,
            revision: 1,
        };
        let actions = match machine.pick_next(now, true) {
            Some(seg) => machine.enter(seg, now),
            None => Vec::new(),
        };
        (machine, actions)
    }

    /// Changes whenever the visible state changed (phase, hold, lockouts, pause); progress
    /// through the windows does not count.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn touch(&mut self) {
        self.revision += 1;
    }

    /// The channel list (for the runtime: VFO settings, labels).
    #[must_use]
    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }

    /// Number of windows in the plan.
    #[must_use]
    pub fn windows(&self) -> usize {
        self.segments.len()
    }

    /// Locked-out frequencies.
    #[must_use]
    pub fn lockouts(&self) -> &BTreeSet<u64> {
        &self.lockouts
    }

    /// The held channel, if any.
    #[must_use]
    pub fn held(&self) -> Option<usize> {
        match self.phase {
            Phase::Holding { channel, .. } => Some(channel),
            _ => None,
        }
    }

    /// Short name of the phase: `settling`, `dwell`, `signal`, `hang` or `blocked`.
    #[cfg(test)]
    #[must_use]
    pub fn phase_name(&self) -> &'static str {
        match self.phase {
            Phase::Settling { .. } => "settling",
            Phase::Dwell { .. } => "dwell",
            Phase::Holding { signal: true, .. } => "signal",
            Phase::Holding { signal: false, .. } => "hang",
            Phase::Blocked(_) => "blocked",
        }
    }

    fn locked_out(&self, channel: usize) -> bool {
        self.lockouts.contains(&self.channels[channel].freq_hz)
    }

    fn snoozed_now(&self, channel: usize, now: Instant) -> bool {
        self.snoozed.get(&channel).is_some_and(|until| *until > now)
    }

    fn usable(&self, channel: usize, now: Instant) -> bool {
        !self.locked_out(channel) && !self.snoozed_now(channel, now)
    }

    fn segment_active(&self, seg: usize, now: Instant) -> bool {
        self.segments[seg]
            .channels
            .iter()
            .any(|&c| self.usable(c, now))
    }

    fn reachable(&self, seg: usize) -> bool {
        self.restricted.is_none()
            || same_center(self.segments[seg].center_hz, self.device_center_hz)
    }

    /// The next window to visit, `None` if there is none right now. `first` is the very first
    /// choice (starts at window 0).
    fn pick_next(&mut self, now: Instant, first: bool) -> Option<usize> {
        let n = self.segments.len();
        if n == 0 {
            return None;
        }
        if !first
            && let Some(p) = self.priority_segment
            && p != self.cur
            && now.duration_since(self.last_priority) >= self.params.priority_interval
            && self.segment_active(p, now)
            && self.reachable(p)
        {
            self.after_priority = Some((self.cur + 1) % n);
            self.last_priority = now;
            return Some(p);
        }
        let start = if first {
            0
        } else {
            self.after_priority.take().unwrap_or((self.cur + 1) % n)
        };
        for k in 0..n {
            let seg = (start + k) % n;
            if self.segment_active(seg, now) && self.reachable(seg) {
                if !first && seg <= self.cur {
                    self.pass += 1;
                }
                return Some(seg);
            }
        }
        None
    }

    /// Go to window `seg`: retune first if its centre differs.
    fn enter(&mut self, seg: usize, now: Instant) -> Vec<Action> {
        self.cur = seg;
        self.served.clear();
        self.touch();
        let center_hz = self.segments[seg].center_hz;
        if same_center(center_hz, self.device_center_hz) {
            self.phase = Phase::Dwell {
                since: now,
                streak: None,
            };
            Vec::new()
        } else {
            self.phase = Phase::Settling {
                target: center_hz,
                since: now,
            };
            vec![Action::Retune { center_hz }]
        }
    }

    /// Leave the current window for the next one.
    fn advance(&mut self, now: Instant) -> Vec<Action> {
        match self.pick_next(now, false) {
            Some(seg) => self.enter(seg, now),
            None => {
                let reason = if self.restricted.is_some() {
                    Blocked::Locked
                } else {
                    Blocked::NothingToScan
                };
                if !matches!(self.phase, Phase::Blocked(b) if b == reason) {
                    self.touch();
                }
                self.phase = Phase::Blocked(reason);
                Vec::new()
            }
        }
    }

    /// The device now sits at `center_hz` (a retune requested by [`Action::Retune`] worked).
    pub fn retune_done(&mut self, center_hz: u64) {
        self.device_center_hz = center_hz;
        if let Phase::Settling { target, .. } = &mut self.phase {
            *target = center_hz;
        }
    }

    /// The retune was refused (tuning lock, claim, device error). The scanner stays within the
    /// windows reachable without a retune and tries again after a while.
    pub fn retune_refused(&mut self, now: Instant) -> Vec<Action> {
        self.restricted = Some(now + RESTRICT_RETRY);
        self.touch();
        self.advance(now)
    }

    /// The VFO could not be put on the held channel: carry on scanning.
    pub fn hit_failed(&mut self, now: Instant) -> Vec<Action> {
        self.leave_hold(now, false)
    }

    fn leave_hold(&mut self, now: Instant, snooze: bool) -> Vec<Action> {
        if let Phase::Holding { channel, .. } = self.phase {
            if snooze {
                self.snoozed.insert(channel, now + SNOOZE);
            }
            // Measure the rest of the window right away.
            self.phase = Phase::Dwell {
                since: now.checked_sub(self.params.dwell).unwrap_or(now),
                streak: None,
            };
            self.touch();
        }
        Vec::new()
    }

    /// Freeze the scan (the VFO stays where it is).
    pub fn pause(&mut self) {
        if !self.paused {
            self.paused = true;
            self.touch();
        }
    }

    /// Continue after a pause; without a pause, let go of a held channel.
    pub fn resume(&mut self, now: Instant) -> Vec<Action> {
        if self.paused {
            self.paused = false;
            self.touch();
            match &mut self.phase {
                Phase::Settling { since, .. } | Phase::Dwell { since, .. } => *since = now,
                Phase::Holding {
                    since, last_signal, ..
                } => {
                    *since = now;
                    *last_signal = now;
                }
                Phase::Blocked(_) => {}
            }
            return Vec::new();
        }
        self.leave_hold(now, false)
    }

    /// Leave the held channel (and ignore it for a while), or the current window.
    pub fn skip(&mut self, now: Instant) -> Vec<Action> {
        self.paused = false;
        match self.phase {
            Phase::Holding { .. } => self.leave_hold(now, true),
            _ => self.advance(now),
        }
    }

    /// Exclude the frequency from scanning. Releases the channel if it is the held one.
    pub fn lockout(&mut self, freq_hz: u64, now: Instant) -> Vec<Action> {
        if self.lockouts.insert(freq_hz) {
            self.touch();
        }
        match self.phase {
            Phase::Holding { channel, .. } if self.channels[channel].freq_hz == freq_hz => {
                self.leave_hold(now, false)
            }
            Phase::Dwell { .. } if !self.segment_active(self.cur, now) => self.advance(now),
            _ => Vec::new(),
        }
    }

    /// Take a frequency (`None`: all) off the lockout list.
    pub fn unlock(&mut self, freq_hz: Option<u64>, now: Instant) -> Vec<Action> {
        let changed = match freq_hz {
            Some(f) => self.lockouts.remove(&f),
            None => {
                let had = !self.lockouts.is_empty();
                self.lockouts.clear();
                had
            }
        };
        if changed {
            self.touch();
        }
        if matches!(self.phase, Phase::Blocked(_)) {
            return self.advance(now);
        }
        Vec::new()
    }

    /// Advance the machine to `now`, with the newest spectrum frame if there is a new one.
    pub fn step(&mut self, now: Instant, frame: Option<&Frame>) -> Vec<Action> {
        if self.paused {
            return Vec::new();
        }
        if let Some(retry) = self.restricted
            && now >= retry
        {
            self.restricted = None;
            self.touch();
        }
        match self.phase.clone() {
            Phase::Settling { target, since } => {
                if frame.is_some_and(|f| f.center_hz == target) {
                    self.device_center_hz = target;
                    self.phase = Phase::Dwell {
                        since: now,
                        streak: None,
                    };
                    self.touch();
                    return self.step(now, frame);
                }
                if now.duration_since(since) >= SETTLE_TIMEOUT {
                    self.phase = Phase::Settling { target, since: now };
                    return vec![Action::Retune { center_hz: target }];
                }
                Vec::new()
            }
            Phase::Dwell { since, streak } => self.step_dwell(now, frame, since, streak),
            Phase::Holding { .. } => self.step_holding(now, frame),
            Phase::Blocked(_) => self.advance(now),
        }
    }

    fn first_candidate(&self, measure: &Measure<'_>, now: Instant) -> Option<Hit> {
        let seg = &self.segments[self.cur];
        let check = |c: usize| -> Option<Hit> {
            if !self.usable(c, now) || self.served.contains(&c) {
                return None;
            }
            let level = measure.level(&self.channels[c])?;
            self.params
                .is_hit(level)
                .then_some(Hit { channel: c, level })
        };
        seg.channels
            .iter()
            .copied()
            .find(|&c| self.channels[c].priority)
            .and_then(check)
            .or_else(|| seg.channels.iter().copied().find_map(check))
    }

    fn step_dwell(
        &mut self,
        now: Instant,
        frame: Option<&Frame>,
        since: Instant,
        streak: Option<(usize, u32)>,
    ) -> Vec<Action> {
        let Some(frame) = frame.filter(|f| f.center_hz == self.device_center_hz) else {
            return Vec::new();
        };
        let measure = Measure::new(frame);
        let candidate = self.first_candidate(&measure, now);
        if self.priority_segment.is_some_and(|p| p == self.cur) {
            self.last_priority = now;
        }
        let streak = match (candidate, streak) {
            (Some(hit), Some((c, n))) if hit.channel == c => Some((c, n + 1)),
            (Some(hit), _) => Some((hit.channel, 1)),
            (None, _) => None,
        };
        self.phase = Phase::Dwell { since, streak };
        if now.duration_since(since) < self.params.dwell {
            return Vec::new();
        }
        match (candidate, streak) {
            (Some(hit), Some((_, n))) if n >= CONFIRM_FRAMES => self.hold(hit, now),
            (Some(_), _) => Vec::new(),
            (None, _) => self.advance(now),
        }
    }

    fn hold(&mut self, hit: Hit, now: Instant) -> Vec<Action> {
        self.served.insert(hit.channel);
        self.phase = Phase::Holding {
            channel: hit.channel,
            since: now,
            last_signal: now,
            signal: true,
            level: hit.level,
        };
        self.touch();
        vec![Action::Hit(hit)]
    }

    fn step_holding(&mut self, now: Instant, frame: Option<&Frame>) -> Vec<Action> {
        let Phase::Holding {
            channel,
            since,
            mut last_signal,
            mut signal,
            mut level,
        } = self.phase.clone()
        else {
            return Vec::new();
        };
        if let Some(frame) = frame.filter(|f| f.center_hz == self.device_center_hz) {
            let measure = Measure::new(frame);
            if let Some(now_level) = measure.level(&self.channels[channel]) {
                if self.params.still_holds(now_level) {
                    last_signal = now;
                    level = now_level;
                    signal = true;
                } else {
                    signal = false;
                }
            }
            // The priority channel interrupts a hold on another channel of the same window.
            if let Some(p) = self.channels.iter().position(|c| c.priority)
                && p != channel
                && self.segments[self.cur].channels.contains(&p)
                && self.usable(p, now)
                && now.duration_since(self.last_priority) >= self.params.priority_interval
            {
                self.last_priority = now;
                if let Some(level) = measure.level(&self.channels[p])
                    && self.params.is_hit(level)
                {
                    return self.hold(Hit { channel: p, level }, now);
                }
            }
        }
        let release = match self.params.resume {
            Resume::Hang => now.duration_since(last_signal) >= self.params.hang,
            Resume::Timeout => now.duration_since(since) >= self.params.timeout,
            Resume::Hold => false,
        };
        if release {
            return self.leave_hold(now, false);
        }
        let changed = match self.phase {
            Phase::Holding { signal: before, .. } => before != signal,
            _ => false,
        };
        self.phase = Phase::Holding {
            channel,
            since,
            last_signal,
            signal,
            level,
        };
        if changed {
            self.touch();
        }
        Vec::new()
    }

    /// The machine's part of the state document.
    #[must_use]
    pub fn view(&self) -> Value {
        let state = if self.paused {
            "paused"
        } else {
            match self.phase {
                Phase::Holding { .. } => "holding",
                Phase::Blocked(_) => "blocked",
                _ => "scanning",
            }
        };
        let mut doc = json!({"state": state, "pass": self.pass});
        if let Some(seg) = self.segments.get(self.cur) {
            doc["window"] = json!({
                "index": self.cur + 1,
                "count": self.segments.len(),
                "center_hz": seg.center_hz,
                "low_hz": seg.low_hz,
                "high_hz": seg.high_hz,
            });
        }
        match &self.phase {
            Phase::Holding {
                channel,
                signal,
                level,
                since: _,
                last_signal: _,
            } => {
                doc["phase"] = json!(if *signal { "signal" } else { "hang" });
                let c = &self.channels[*channel];
                doc["current"] = json!({
                    "freq_hz": c.freq_hz,
                    "mode": c.mode,
                    "bandwidth_hz": c.vfo_bandwidth_hz(),
                    "label": c.label,
                    "level_dbfs": round1(level.dbfs),
                    "margin_db": round1(level.margin_db),
                    "priority": c.priority,
                });
            }
            Phase::Blocked(reason) => doc["blocked"] = json!(reason.as_str()),
            _ => {}
        }
        if self.restricted.is_some() {
            doc["restricted"] = json!("locked");
        }
        doc
    }
}

fn round1(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::super::plan::plan;
    use super::*;
    use crate::vfo::VfoMode;

    const RATE: u32 = 2_400_000;
    const BINS: usize = 2048;
    const FRAME: Duration = Duration::from_millis(40);

    fn nfm(freq_hz: u64) -> Channel {
        Channel {
            freq_hz,
            mode: VfoMode::Nfm,
            bandwidth_hz: None,
            detect_hz: None,
            label: None,
            priority: false,
        }
    }

    /// A band with carriers: noise at -90 dBFS plus a -40 dBFS bump for each active carrier.
    struct Band {
        carriers: Vec<u64>,
        seq: u64,
    }

    impl Band {
        fn frame(&mut self, center_hz: u64) -> Frame {
            self.seq += 1;
            let bin_hz = f64::from(RATE) / BINS as f64;
            let start = center_hz as f64 - f64::from(RATE) / 2.0;
            let mut db = vec![-90.0_f32; BINS];
            for f in &self.carriers {
                let bin = ((*f as f64 - start) / bin_hz).round();
                if (0.0..BINS as f64).contains(&bin) {
                    for k in -1..=1_i64 {
                        let i = (bin as i64 + k) as usize;
                        db[i] = db[i].max(if k == 0 { -40.0 } else { -50.0 });
                    }
                }
            }
            Frame {
                seq: self.seq,
                center_hz,
                sample_rate: RATE,
                db,
            }
        }
    }

    struct Rig {
        machine: Machine,
        band: Band,
        now: Instant,
        center_hz: u64,
        hits: Vec<u64>,
        retunes: Vec<u64>,
        /// Refuse every retune like a tuning lock does.
        refuse: bool,
        refused: usize,
    }

    fn params() -> Params {
        Params {
            threshold_db: 10.0,
            threshold_dbfs: None,
            dwell: Duration::from_millis(120),
            hang: Duration::from_millis(2000),
            resume: Resume::Hang,
            timeout: Duration::from_millis(5000),
            priority_interval: Duration::from_millis(1000),
        }
    }

    fn rig(channels: Vec<Channel>, params: Params, center_hz: u64) -> Rig {
        let segments = plan(&channels, center_hz, RATE).expect("plan");
        let now = Instant::now();
        let (machine, actions) =
            Machine::new(channels, segments, params, BTreeSet::new(), center_hz, now);
        let mut rig = Rig {
            machine,
            band: Band {
                carriers: Vec::new(),
                seq: 0,
            },
            now,
            center_hz,
            hits: Vec::new(),
            retunes: Vec::new(),
            refuse: false,
            refused: 0,
        };
        rig.apply(actions);
        rig
    }

    impl Rig {
        fn apply(&mut self, actions: Vec<Action>) {
            for action in actions {
                match action {
                    Action::Retune { .. } if self.refuse => {
                        self.refused += 1;
                        let more = self.machine.retune_refused(self.now);
                        self.apply(more);
                    }
                    Action::Retune { center_hz } => {
                        self.center_hz = center_hz;
                        self.retunes.push(center_hz);
                        self.machine.retune_done(center_hz);
                    }
                    Action::Hit(hit) => {
                        self.hits.push(self.machine.channels()[hit.channel].freq_hz);
                    }
                }
            }
        }

        /// One frame period of virtual time.
        fn tick(&mut self) {
            self.now += FRAME;
            let frame = self.band.frame(self.center_hz);
            let actions = self.machine.step(self.now, Some(&frame));
            self.apply(actions);
        }

        fn run(&mut self, d: Duration) {
            let end = self.now + d;
            while self.now < end {
                self.tick();
            }
        }
    }

    #[test]
    fn stops_on_a_carrier_and_reports_the_hit() {
        let mut rig = rig(
            vec![nfm(145_100_000), nfm(145_500_000)],
            params(),
            145_000_000,
        );
        rig.band.carriers = vec![145_500_000];
        rig.run(Duration::from_millis(400));
        assert_eq!(rig.hits, vec![145_500_000]);
        assert_eq!(rig.machine.phase_name(), "signal");
        assert_eq!(rig.machine.view()["current"]["freq_hz"], 145_500_000);
        assert!(
            rig.retunes.is_empty(),
            "scan inside the span must not retune"
        );
    }

    #[test]
    fn quiet_band_never_stops() {
        let mut rig = rig(
            vec![nfm(145_100_000), nfm(145_500_000)],
            params(),
            145_000_000,
        );
        rig.run(Duration::from_secs(3));
        assert!(rig.hits.is_empty());
        assert_eq!(rig.machine.view()["state"], "scanning");
        assert!(rig.machine.view()["pass"].as_u64().unwrap_or(0) > 3);
    }

    #[test]
    fn hang_time_resumes_after_the_signal_is_gone() {
        let mut rig = rig(
            vec![nfm(145_100_000), nfm(145_500_000)],
            params(),
            145_000_000,
        );
        rig.band.carriers = vec![145_500_000];
        rig.run(Duration::from_millis(400));
        assert_eq!(rig.machine.phase_name(), "signal");
        // Signal gone: hang phase, still held after 1.5 s, released after 2 s.
        rig.band.carriers.clear();
        rig.run(Duration::from_millis(200));
        assert_eq!(rig.machine.phase_name(), "hang");
        rig.run(Duration::from_millis(1400));
        assert_eq!(rig.machine.phase_name(), "hang");
        rig.run(Duration::from_millis(600));
        assert_eq!(rig.machine.view()["state"], "scanning");
        // A signal that comes back inside the hang time keeps the hold.
        rig.band.carriers = vec![145_100_000];
        rig.run(Duration::from_millis(400));
        assert_eq!(
            rig.machine
                .held()
                .map(|c| rig.machine.channels()[c].freq_hz),
            Some(145_100_000)
        );
        rig.band.carriers.clear();
        rig.run(Duration::from_millis(1500));
        rig.band.carriers = vec![145_100_000];
        rig.run(Duration::from_millis(1000));
        assert_eq!(rig.hits.len(), 2, "no second stop: the hold continued");
        assert_eq!(rig.machine.phase_name(), "signal");
    }

    #[test]
    fn resumes_with_the_next_channel_after_a_hold() {
        let mut rig = rig(
            vec![nfm(145_100_000), nfm(145_500_000)],
            params(),
            145_000_000,
        );
        rig.band.carriers = vec![145_100_000, 145_500_000];
        rig.run(Duration::from_millis(400));
        assert_eq!(rig.hits, vec![145_100_000]);
        rig.band.carriers = vec![145_500_000];
        rig.run(Duration::from_millis(3000));
        // The first carrier ended, hang ran out, the second one was found right after.
        assert_eq!(rig.hits, vec![145_100_000, 145_500_000]);
    }

    #[test]
    fn timeout_resume_lets_go_while_the_signal_is_still_there() {
        let mut p = params();
        p.resume = Resume::Timeout;
        p.timeout = Duration::from_millis(1000);
        let mut rig = rig(vec![nfm(145_100_000), nfm(145_500_000)], p, 145_000_000);
        rig.band.carriers = vec![145_100_000];
        rig.run(Duration::from_millis(400));
        assert_eq!(rig.machine.phase_name(), "signal");
        rig.run(Duration::from_millis(1200));
        // Released after 1 s, then it sits on the same carrier again (window is rescanned).
        assert!(rig.hits.len() >= 2, "{:?}", rig.hits);
    }

    #[test]
    fn hold_mode_stays_until_the_user_resumes() {
        let mut p = params();
        p.resume = Resume::Hold;
        let mut rig = rig(vec![nfm(145_100_000), nfm(145_500_000)], p, 145_000_000);
        rig.band.carriers = vec![145_100_000];
        rig.run(Duration::from_millis(400));
        rig.band.carriers.clear();
        rig.run(Duration::from_secs(20));
        assert_eq!(rig.machine.phase_name(), "hang");
        assert!(rig.machine.held().is_some());
        let actions = rig.machine.resume(rig.now);
        rig.apply(actions);
        assert_eq!(rig.machine.view()["state"], "scanning");
    }

    #[test]
    fn lockout_skips_the_channel_and_releases_a_hold_on_it() {
        let mut rig = rig(
            vec![nfm(145_100_000), nfm(145_500_000)],
            params(),
            145_000_000,
        );
        rig.band.carriers = vec![145_100_000, 145_500_000];
        rig.run(Duration::from_millis(400));
        assert_eq!(rig.hits, vec![145_100_000]);
        let actions = rig.machine.lockout(145_100_000, rig.now);
        rig.apply(actions);
        assert_eq!(rig.machine.view()["state"], "scanning");
        rig.run(Duration::from_millis(400));
        assert_eq!(rig.hits, vec![145_100_000, 145_500_000]);
        // Locked out for good: after the second carrier ends the first one is not found again.
        rig.band.carriers = vec![145_100_000];
        let actions = rig.machine.lockout(145_500_000, rig.now);
        rig.apply(actions);
        rig.run(Duration::from_secs(5));
        assert_eq!(rig.hits.len(), 2);
        assert_eq!(rig.machine.view()["blocked"], "nothing_to_scan");
        // Unlocking brings the scan back.
        let actions = rig.machine.unlock(Some(145_100_000), rig.now);
        rig.apply(actions);
        rig.run(Duration::from_millis(500));
        assert_eq!(rig.hits.last(), Some(&145_100_000));
    }

    #[test]
    fn skip_ignores_a_channel_that_is_still_transmitting() {
        let mut rig = rig(
            vec![nfm(145_100_000), nfm(145_500_000)],
            params(),
            145_000_000,
        );
        rig.band.carriers = vec![145_100_000];
        rig.run(Duration::from_millis(400));
        let actions = rig.machine.skip(rig.now);
        rig.apply(actions);
        rig.run(Duration::from_secs(5));
        assert_eq!(rig.hits.len(), 1);
        assert_eq!(rig.machine.view()["state"], "scanning");
    }

    #[test]
    fn pause_freezes_and_resume_continues() {
        let mut rig = rig(
            vec![nfm(145_100_000), nfm(145_500_000)],
            params(),
            145_000_000,
        );
        rig.machine.pause();
        rig.band.carriers = vec![145_500_000];
        rig.run(Duration::from_secs(2));
        assert!(rig.hits.is_empty());
        assert_eq!(rig.machine.view()["state"], "paused");
        let actions = rig.machine.resume(rig.now);
        rig.apply(actions);
        rig.run(Duration::from_millis(400));
        assert_eq!(rig.hits, vec![145_500_000]);
    }

    #[test]
    fn walks_across_windows_and_retunes() {
        let channels = vec![nfm(145_500_000), nfm(433_500_000), nfm(446_006_250)];
        let mut rig = rig(channels, params(), 145_000_000);
        rig.band.carriers = vec![446_006_250];
        rig.run(Duration::from_secs(3));
        assert_eq!(rig.hits, vec![446_006_250]);
        assert!(
            rig.retunes.contains(&433_500_000) || rig.retunes.len() >= 2,
            "{:?}",
            rig.retunes
        );
        assert_eq!(rig.machine.view()["window"]["count"], 3);
    }

    #[test]
    fn a_refused_retune_keeps_the_scan_inside_the_current_window() {
        let channels = vec![nfm(145_100_000), nfm(433_500_000)];
        let mut rig = rig(channels, params(), 145_000_000);
        rig.refuse = true;
        rig.band.carriers = vec![145_100_000];
        rig.run(Duration::from_millis(400));
        assert_eq!(rig.hits, vec![145_100_000]);
        rig.band.carriers.clear();
        rig.run(Duration::from_secs(8));
        // The far window was tried again after each pause, never reached.
        assert!(rig.refused >= 2, "retried after the pause: {}", rig.refused);
        assert_eq!(rig.center_hz, 145_000_000);
        assert_eq!(rig.machine.view()["state"], "scanning");
        // A carrier in the home window is still found.
        rig.band.carriers = vec![145_100_000];
        rig.run(Duration::from_millis(600));
        assert_eq!(rig.hits.len(), 2);
    }

    #[test]
    fn priority_channel_is_checked_at_its_interval() {
        // The priority channel sits in another window; the scan visits it every second.
        let mut priority = nfm(433_500_000);
        priority.priority = true;
        let channels = vec![nfm(145_100_000), nfm(145_500_000), priority];
        let mut p = params();
        p.priority_interval = Duration::from_millis(1000);
        let mut rig = rig(channels, p, 145_000_000);
        rig.run(Duration::from_secs(6));
        let visits = rig.retunes.iter().filter(|c| **c == 433_500_000).count();
        assert!(
            visits >= 4,
            "priority visited {visits} times: {:?}",
            rig.retunes
        );
        // And it wins as soon as it is active.
        rig.band.carriers = vec![433_500_000];
        rig.run(Duration::from_secs(3));
        assert_eq!(rig.hits, vec![433_500_000]);
    }

    #[test]
    fn priority_channel_interrupts_a_hold_in_the_same_window() {
        let mut priority = nfm(145_900_000);
        priority.priority = true;
        let channels = vec![nfm(145_100_000), priority];
        let mut rig = rig(channels, params(), 145_000_000);
        rig.band.carriers = vec![145_100_000];
        rig.run(Duration::from_millis(1500));
        assert_eq!(rig.hits, vec![145_100_000]);
        rig.band.carriers = vec![145_100_000, 145_900_000];
        rig.run(Duration::from_millis(1500));
        assert_eq!(rig.hits, vec![145_100_000, 145_900_000]);
    }

    #[test]
    fn absolute_threshold_replaces_the_relative_one() {
        let mut p = params();
        p.threshold_dbfs = Some(-30.0);
        let mut rig = rig(vec![nfm(145_100_000)], p, 145_000_000);
        rig.band.carriers = vec![145_100_000]; // -40 dBFS: below the absolute threshold
        rig.run(Duration::from_secs(1));
        assert!(rig.hits.is_empty());
    }

    #[test]
    fn nothing_to_scan_is_reported_not_spun() {
        let mut p = params();
        p.dwell = Duration::from_millis(40);
        let channels = vec![nfm(145_100_000)];
        let segments = plan(&channels, 145_000_000, RATE).expect("plan");
        let now = Instant::now();
        let lockouts = BTreeSet::from([145_100_000]);
        let (machine, actions) = Machine::new(channels, segments, p, lockouts, 145_000_000, now);
        assert!(actions.is_empty());
        assert_eq!(machine.view()["blocked"], "nothing_to_scan");
    }
}
