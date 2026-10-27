//! Turns the periodic tone / RDS state messages of the daemons into discrete history events.
//!
//! Pure logic with an injected clock (`now_ms`), so every rule is unit-tested:
//!
//! - DTMF: digits that arrive close together are one burst. A burst ends when no digit arrived
//!   for [`DTMF_GAP_MS`] (or the VFO is cleared / retuned / the burst reaches [`MAX_BURST`]);
//!   it is stored as one event whose value is the digit sequence.
//! - CTCSS: a tone must persist for [`CTCSS_CONFIRM_MS`] before it is reported (`edge:"start"`);
//!   it ends after being absent for [`CTCSS_HOLD_MS`] (`edge:"stop"` with `duration_ms`). A
//!   dropout shorter than the hold time is bridged, so a flickering detector does not spam.
//! - RDS: PS, RadioText and PI are stored when their value changes. An empty state (retune,
//!   mode change) forgets the last values, so tuning back to a station logs it again.

use std::collections::HashMap;

use serde_json::json;
use superhet_bus::{rds::RdsMessage, tones::TonesMessage};
use superhet_db::decoder_events::NewEvent;

/// A DTMF burst ends this long after its last digit.
pub const DTMF_GAP_MS: i64 = 1_500;
/// A burst is split after this many digits.
pub const MAX_BURST: usize = 64;
/// A CTCSS tone has to be present this long before it is reported.
pub const CTCSS_CONFIRM_MS: i64 = 800;
/// A CTCSS tone has to be gone this long before it is reported as stopped.
pub const CTCSS_HOLD_MS: i64 = 1_500;
/// Digits assumed to belong to a fresh VFO when its first message already carries some. Larger
/// counts are the history of a channel that ran before the worker started and are skipped.
const FRESH_DIGITS: u64 = 4;

/// Tuning of a VFO at the time of an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ctx {
    /// Frequency in Hz.
    pub freq_hz: i64,
    /// Demodulation mode (`nfm`, `wfm`, ...).
    pub mode: String,
}

#[derive(Debug, Default)]
struct Burst {
    digits: String,
    start_ms: i64,
    last_ms: i64,
    ctx: Option<Ctx>,
}

#[derive(Debug)]
struct Logged {
    key: i32,
    start_ms: i64,
    ctx: Option<Ctx>,
}

#[derive(Debug, Default)]
struct Ctcss {
    /// Tone seen right now and since when (not necessarily reported yet).
    candidate: Option<(i32, i64)>,
    /// Tone that was reported as started.
    logged: Option<Logged>,
    /// When the reported tone was last seen going away.
    lost_since: Option<i64>,
}

#[derive(Debug, Default)]
struct Vfo {
    ctx: Option<Ctx>,
    /// DTMF counter of the last message; `None` before the first one.
    dtmf_count: Option<u64>,
    burst: Option<Burst>,
    ctcss: Ctcss,
    ps: Option<String>,
    rt: Option<String>,
    pi: Option<String>,
}

/// Per-VFO recorder state of all devices.
#[derive(Debug, Default)]
pub struct Recorder {
    vfos: HashMap<(String, u32), Vfo>,
}

fn tone_key(hz: f32) -> i32 {
    // Tones are 0.1 Hz apart; the rounded value is exact for equality.
    (hz * 10.0).round() as i32
}

fn tone_hz(key: i32) -> f64 {
    f64::from(key) / 10.0
}

fn event(
    device: &str,
    vfo: u32,
    ts_ms: i64,
    ctx: Option<&Ctx>,
    kind: &str,
    value: String,
    meta: serde_json::Value,
) -> NewEvent {
    NewEvent {
        ts_ms,
        device: device.to_owned(),
        vfo: Some(i32::try_from(vfo).unwrap_or(i32::MAX)),
        freq_hz: ctx.map(|c| c.freq_hz),
        mode: ctx.map(|c| c.mode.clone()),
        kind: kind.to_owned(),
        source: superhet_db::decoder_events::CORE.to_owned(),
        value,
        meta,
    }
}

fn flush_burst(device: &str, vfo: u32, v: &mut Vfo, out: &mut Vec<NewEvent>) {
    if let Some(b) = v.burst.take()
        && !b.digits.is_empty()
    {
        let meta = json!({"digits": b.digits.len(), "duration_ms": b.last_ms - b.start_ms});
        out.push(event(
            device,
            vfo,
            b.start_ms,
            b.ctx.as_ref(),
            "dtmf",
            b.digits,
            meta,
        ));
    }
}

fn stop_ctcss(device: &str, vfo: u32, c: &mut Ctcss, end_ms: i64, out: &mut Vec<NewEvent>) {
    if let Some(l) = c.logged.take() {
        let hz = tone_hz(l.key);
        out.push(event(
            device,
            vfo,
            end_ms,
            l.ctx.as_ref(),
            "ctcss",
            format!("{hz:.1} Hz"),
            json!({"hz": hz, "edge": "stop", "duration_ms": (end_ms - l.start_ms).max(0)}),
        ));
    }
    c.lost_since = None;
}

/// Report a confirmed tone start / a finished tone stop that is due at `now_ms`.
fn advance_ctcss(device: &str, vfo: u32, v: &mut Vfo, now_ms: i64, out: &mut Vec<NewEvent>) {
    if let Some(lost) = v.ctcss.lost_since
        && now_ms - lost >= CTCSS_HOLD_MS
    {
        stop_ctcss(device, vfo, &mut v.ctcss, lost, out);
    }
    if v.ctcss.logged.is_none()
        && let Some((key, since)) = v.ctcss.candidate
        && now_ms - since >= CTCSS_CONFIRM_MS
    {
        let hz = tone_hz(key);
        out.push(event(
            device,
            vfo,
            since,
            v.ctx.as_ref(),
            "ctcss",
            format!("{hz:.1} Hz"),
            json!({"hz": hz, "edge": "start"}),
        ));
        v.ctcss.logged = Some(Logged {
            key,
            start_ms: since,
            ctx: v.ctx.clone(),
        });
    }
}

/// Close everything that is open: used when a VFO is cleared, retuned or removed.
fn close_all(device: &str, vfo: u32, v: &mut Vfo, now_ms: i64, out: &mut Vec<NewEvent>) {
    flush_burst(device, vfo, v, out);
    let end = v.ctcss.lost_since.unwrap_or(now_ms);
    stop_ctcss(device, vfo, &mut v.ctcss, end, out);
    v.ctcss = Ctcss::default();
    v.dtmf_count = None;
}

impl Recorder {
    /// Create an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The VFOs of `device` as the daemon reports them now. A VFO that disappeared or was
    /// retuned closes its open bursts / tones first.
    pub fn set_state(&mut self, device: &str, now_ms: i64, vfos: &[(u32, Ctx)]) -> Vec<NewEvent> {
        let mut out = Vec::new();
        for ((dev, id), v) in &mut self.vfos {
            if dev != device {
                continue;
            }
            let new = vfos.iter().find(|(i, _)| i == id).map(|(_, c)| c);
            let changed = match (&v.ctx, new) {
                (Some(old), Some(new)) => old.freq_hz != new.freq_hz || old.mode != new.mode,
                (_, None) => true,
                _ => false,
            };
            if changed {
                close_all(dev, *id, v, now_ms, &mut out);
                v.ps = None;
                v.rt = None;
                v.pi = None;
            }
            v.ctx = new.cloned();
        }
        // VFOs without recorder state yet only need their context for later messages.
        for (id, ctx) in vfos {
            self.vfos
                .entry((device.to_owned(), *id))
                .or_default()
                .ctx
                .get_or_insert_with(|| ctx.clone());
        }
        // Drop state of VFOs that are gone (their open items were closed above).
        self.vfos
            .retain(|(dev, id), _| dev != device || vfos.iter().any(|(i, _)| i == id));
        out
    }

    /// A tone state message.
    pub fn on_tones(&mut self, device: &str, now_ms: i64, msg: &TonesMessage) -> Vec<NewEvent> {
        let mut out = Vec::new();
        let v = self.vfos.entry((device.to_owned(), msg.vfo)).or_default();

        // DTMF digits.
        let count = msg.dtmf_count;
        match v.dtmf_count {
            None if count > FRESH_DIGITS => {
                // History of a channel that ran before us.
            }
            prev => {
                let prev = prev.unwrap_or(0);
                if count < prev {
                    // The daemon restarted the channel: close what was running.
                    flush_burst(device, msg.vfo, v, &mut out);
                }
                let base = if count < prev { 0 } else { prev };
                let new = count - base;
                if new > 0 {
                    let tail = msg.dtmf_tail.as_str();
                    let take = usize::try_from(new).unwrap_or(usize::MAX).min(tail.len());
                    let digits = &tail[tail.len() - take..];
                    let burst = v.burst.get_or_insert_with(|| Burst {
                        start_ms: now_ms,
                        ctx: v.ctx.clone(),
                        ..Burst::default()
                    });
                    for d in digits.chars() {
                        if burst.digits.len() >= MAX_BURST {
                            break;
                        }
                        burst.digits.push(d);
                    }
                    burst.last_ms = now_ms;
                    if burst.digits.len() >= MAX_BURST {
                        flush_burst(device, msg.vfo, v, &mut out);
                    }
                }
            }
        }
        v.dtmf_count = Some(count);
        if count == 0 {
            flush_burst(device, msg.vfo, v, &mut out);
        }

        // CTCSS.
        match msg.ctcss_hz.map(tone_key) {
            Some(key) => {
                if v.ctcss.logged.as_ref().is_some_and(|l| l.key == key) {
                    v.ctcss.lost_since = None;
                } else if v.ctcss.logged.is_some() {
                    // A different tone replaces the reported one.
                    let end = v.ctcss.lost_since.unwrap_or(now_ms);
                    stop_ctcss(device, msg.vfo, &mut v.ctcss, end, &mut out);
                }
                if v.ctcss.candidate.is_none_or(|(k, _)| k != key) {
                    v.ctcss.candidate = Some((key, now_ms));
                }
            }
            None => {
                v.ctcss.candidate = None;
                if v.ctcss.logged.is_some() && v.ctcss.lost_since.is_none() {
                    v.ctcss.lost_since = Some(now_ms);
                }
            }
        }
        advance_ctcss(device, msg.vfo, v, now_ms, &mut out);
        out
    }

    /// An RDS state message.
    pub fn on_rds(&mut self, device: &str, now_ms: i64, msg: &RdsMessage) -> Vec<NewEvent> {
        let mut out = Vec::new();
        let v = self.vfos.entry((device.to_owned(), msg.vfo)).or_default();
        if msg.is_empty() {
            v.ps = None;
            v.rt = None;
            v.pi = None;
            return out;
        }
        let meta = json!({"pi": msg.pi, "pty": msg.pty});
        let mut changed = |slot: &mut Option<String>, new: Option<&str>, kind: &str| {
            let Some(new) = new.map(str::trim).filter(|s| !s.is_empty()) else {
                return;
            };
            if slot.as_deref() != Some(new) {
                *slot = Some(new.to_owned());
                out.push(event(
                    device,
                    msg.vfo,
                    now_ms,
                    v.ctx.as_ref(),
                    kind,
                    new.to_owned(),
                    meta.clone(),
                ));
            }
        };
        changed(&mut v.pi, msg.pi.as_deref(), "rds_pi");
        changed(&mut v.ps, msg.ps.as_deref(), "rds_ps");
        changed(&mut v.rt, msg.rt.as_deref(), "rds_rt");
        out
    }

    /// Timers: ends DTMF bursts that went quiet and reports confirmed / finished CTCSS tones.
    pub fn tick(&mut self, now_ms: i64) -> Vec<NewEvent> {
        let mut out = Vec::new();
        for ((dev, id), v) in &mut self.vfos {
            if v.burst
                .as_ref()
                .is_some_and(|b| now_ms - b.last_ms > DTMF_GAP_MS)
            {
                flush_burst(dev, *id, v, &mut out);
            }
            advance_ctcss(dev, *id, v, now_ms, &mut out);
        }
        out
    }

    /// Close everything that is open (shutdown).
    pub fn flush_all(&mut self, now_ms: i64) -> Vec<NewEvent> {
        let mut out = Vec::new();
        for ((dev, id), v) in &mut self.vfos {
            close_all(dev, *id, v, now_ms, &mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEV: &str = "rtl0";

    fn ctx(freq_hz: i64) -> Ctx {
        Ctx {
            freq_hz,
            mode: "nfm".into(),
        }
    }

    fn rec() -> Recorder {
        let mut r = Recorder::new();
        assert!(r.set_state(DEV, 0, &[(1, ctx(433_500_000))]).is_empty());
        r
    }

    fn tones(count: u64, tail: &str, ctcss: Option<f32>) -> TonesMessage {
        TonesMessage {
            ctcss_hz: ctcss,
            dtmf_count: count,
            dtmf_tail: tail.into(),
            ..TonesMessage::empty(1)
        }
    }

    fn values(events: &[NewEvent]) -> Vec<(&str, &str)> {
        events
            .iter()
            .map(|e| (e.kind.as_str(), e.value.as_str()))
            .collect()
    }

    #[test]
    fn digits_within_the_gap_form_one_burst() {
        let mut r = rec();
        assert!(r.on_tones(DEV, 1_000, &tones(1, "1", None)).is_empty());
        assert!(r.on_tones(DEV, 1_300, &tones(2, "12", None)).is_empty());
        assert!(r.on_tones(DEV, 2_400, &tones(3, "123", None)).is_empty());
        assert!(r.tick(3_900).is_empty(), "exactly the gap is not yet over");
        let out = r.tick(3_901);
        assert_eq!(values(&out), [("dtmf", "123")]);
        let e = &out[0];
        assert_eq!(e.ts_ms, 1_000, "timestamp of the first digit");
        assert_eq!(e.freq_hz, Some(433_500_000));
        assert_eq!(e.mode.as_deref(), Some("nfm"));
        assert_eq!(e.meta["digits"], 3);
        assert_eq!(e.meta["duration_ms"], 1_400);
        assert!(r.tick(10_000).is_empty(), "reported once");
    }

    #[test]
    fn a_pause_splits_bursts() {
        let mut r = rec();
        r.on_tones(DEV, 1_000, &tones(2, "*1", None));
        let first = r.tick(2_600);
        r.on_tones(DEV, 5_000, &tones(4, "*123", None));
        let second = r.tick(6_600);
        assert_eq!(values(&first), [("dtmf", "*1")]);
        assert_eq!(values(&second), [("dtmf", "23")]);
    }

    #[test]
    fn repeated_messages_do_not_duplicate_digits() {
        let mut r = rec();
        r.on_tones(DEV, 1_000, &tones(2, "AB", None));
        r.on_tones(DEV, 2_000, &tones(2, "AB", None));
        r.on_tones(DEV, 3_000, &tones(2, "AB", None));
        assert_eq!(values(&r.tick(3_100)), [("dtmf", "AB")]);
    }

    #[test]
    fn a_lost_message_uses_the_tail() {
        let mut r = rec();
        r.on_tones(DEV, 1_000, &tones(1, "1", None));
        // Digits 2 and 3 arrived in a message that was dropped; the tail still has them.
        r.on_tones(DEV, 1_200, &tones(4, "1234", None));
        assert_eq!(values(&r.tick(3_000)), [("dtmf", "1234")]);
    }

    #[test]
    fn a_counter_reset_closes_the_burst_and_starts_over() {
        let mut r = rec();
        r.on_tones(DEV, 1_000, &tones(3, "123", None));
        let out = r.on_tones(DEV, 1_400, &tones(0, "", None));
        assert_eq!(values(&out), [("dtmf", "123")]);
        assert!(r.on_tones(DEV, 2_000, &tones(1, "9", None)).is_empty());
        assert_eq!(values(&r.tick(4_000)), [("dtmf", "9")]);
    }

    #[test]
    fn a_long_history_at_start_is_skipped_but_a_fresh_digit_is_not() {
        let mut r = rec();
        let late = r.on_tones(DEV, 1_000, &tones(500, "0123456789", None));
        assert!(late.is_empty());
        assert!(r.tick(5_000).is_empty());
        r.on_tones(DEV, 6_000, &tones(501, "1234567890", None));
        assert_eq!(values(&r.tick(8_000)), [("dtmf", "0")]);

        let mut fresh = rec();
        fresh.on_tones(DEV, 1_000, &tones(1, "5", None));
        assert_eq!(values(&fresh.tick(3_000)), [("dtmf", "5")]);
    }

    #[test]
    fn very_long_bursts_are_split() {
        let mut r = rec();
        let mut count = 0_u64;
        let mut out = Vec::new();
        for i in 0..70 {
            count += 1;
            out.extend(r.on_tones(DEV, 1_000 + i * 100, &tones(count, "7", None)));
        }
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.len(), MAX_BURST);
        assert_eq!(r.tick(20_000)[0].value.len(), 70 - MAX_BURST);
    }

    #[test]
    fn ctcss_is_reported_once_per_tone_with_a_duration() {
        let mut r = rec();
        assert!(r.on_tones(DEV, 1_000, &tones(0, "", Some(88.5))).is_empty());
        assert!(r.tick(1_700).is_empty(), "not confirmed yet");
        let start = r.tick(1_800);
        assert_eq!(values(&start), [("ctcss", "88.5 Hz")]);
        assert_eq!(start[0].ts_ms, 1_000);
        assert_eq!(start[0].meta["edge"], "start");
        // Repeated messages change nothing.
        assert!(r.on_tones(DEV, 2_000, &tones(0, "", Some(88.5))).is_empty());
        assert!(r.on_tones(DEV, 3_000, &tones(0, "", Some(88.5))).is_empty());
        // The tone goes away; the stop comes after the hold time, stamped with the loss.
        assert!(r.on_tones(DEV, 9_000, &tones(0, "", None)).is_empty());
        assert!(r.tick(10_400).is_empty());
        let stop = r.tick(10_500);
        assert_eq!(values(&stop), [("ctcss", "88.5 Hz")]);
        assert_eq!(stop[0].ts_ms, 9_000);
        assert_eq!(stop[0].meta["edge"], "stop");
        assert_eq!(stop[0].meta["duration_ms"], 8_000);
        assert!(r.tick(20_000).is_empty());
    }

    #[test]
    fn a_short_dropout_is_bridged() {
        let mut r = rec();
        r.on_tones(DEV, 1_000, &tones(0, "", Some(100.0)));
        assert_eq!(r.tick(2_000).len(), 1);
        r.on_tones(DEV, 3_000, &tones(0, "", None));
        r.on_tones(DEV, 3_600, &tones(0, "", Some(100.0)));
        assert!(r.tick(10_000).is_empty(), "no stop, no second start");
    }

    #[test]
    fn a_blip_shorter_than_the_confirmation_is_never_reported() {
        let mut r = rec();
        r.on_tones(DEV, 1_000, &tones(0, "", Some(67.0)));
        r.on_tones(DEV, 1_300, &tones(0, "", None));
        assert!(r.tick(10_000).is_empty());
    }

    #[test]
    fn a_different_tone_stops_the_old_one_and_starts_the_new() {
        let mut r = rec();
        r.on_tones(DEV, 1_000, &tones(0, "", Some(88.5)));
        assert_eq!(r.tick(2_000).len(), 1);
        let out = r.on_tones(DEV, 5_000, &tones(0, "", Some(110.9)));
        assert_eq!(values(&out), [("ctcss", "88.5 Hz")]);
        assert_eq!(out[0].meta["edge"], "stop");
        let out = r.tick(6_000);
        assert_eq!(values(&out), [("ctcss", "110.9 Hz")]);
        assert_eq!(out[0].meta["edge"], "start");
    }

    #[test]
    fn retuning_closes_open_items_with_the_old_frequency() {
        let mut r = rec();
        r.on_tones(DEV, 1_000, &tones(0, "", Some(88.5)));
        r.tick(2_000);
        r.on_tones(DEV, 2_100, &tones(2, "12", Some(88.5)));
        let out = r.set_state(DEV, 3_000, &[(1, ctx(145_000_000))]);
        assert_eq!(values(&out), [("dtmf", "12"), ("ctcss", "88.5 Hz")]);
        assert!(out.iter().all(|e| e.freq_hz == Some(433_500_000)));
        // New events carry the new frequency.
        r.on_tones(DEV, 4_000, &tones(1, "7", None));
        assert_eq!(r.tick(6_000)[0].freq_hz, Some(145_000_000));
    }

    #[test]
    fn a_removed_vfo_is_flushed_and_forgotten() {
        let mut r = rec();
        r.on_tones(DEV, 1_000, &tones(1, "3", None));
        let out = r.set_state(DEV, 1_500, &[]);
        assert_eq!(values(&out), [("dtmf", "3")]);
        assert!(r.vfos.is_empty());
    }

    fn rds(ps: Option<&str>, rt: Option<&str>) -> RdsMessage {
        RdsMessage {
            synced: true,
            pi: Some("D220".into()),
            pty: Some(10),
            ps: ps.map(str::to_owned),
            rt: rt.map(str::to_owned),
            ..RdsMessage::empty(1)
        }
    }

    #[test]
    fn rds_events_fire_on_change_only() {
        let mut r = rec();
        let out = r.on_rds(DEV, 1_000, &rds(Some("RADIO X "), None));
        assert_eq!(values(&out), [("rds_pi", "D220"), ("rds_ps", "RADIO X")]);
        assert_eq!(out[1].meta["pi"], "D220");
        assert_eq!(out[1].freq_hz, Some(433_500_000));
        assert!(
            r.on_rds(DEV, 2_000, &rds(Some("RADIO X "), None))
                .is_empty()
        );
        let out = r.on_rds(DEV, 3_000, &rds(Some("RADIO X "), Some("Now playing  ")));
        assert_eq!(values(&out), [("rds_rt", "Now playing")]);
        let out = r.on_rds(DEV, 4_000, &rds(Some("NEWS    "), Some("Now playing  ")));
        assert_eq!(values(&out), [("rds_ps", "NEWS")]);
    }

    #[test]
    fn an_empty_rds_state_forgets_so_the_station_logs_again() {
        let mut r = rec();
        assert_eq!(r.on_rds(DEV, 1_000, &rds(Some("RADIO X "), None)).len(), 2);
        // Signal loss keeps data (synced false): still no duplicate.
        let mut lost = rds(Some("RADIO X "), None);
        lost.synced = false;
        assert!(r.on_rds(DEV, 2_000, &lost).is_empty());
        assert!(r.on_rds(DEV, 3_000, &RdsMessage::empty(1)).is_empty());
        assert_eq!(r.on_rds(DEV, 4_000, &rds(Some("RADIO X "), None)).len(), 2);
    }

    #[test]
    fn flush_all_closes_everything() {
        let mut r = rec();
        r.on_tones(DEV, 1_000, &tones(1, "4", Some(88.5)));
        r.tick(2_000);
        let out = r.flush_all(2_500);
        assert_eq!(values(&out), [("dtmf", "4"), ("ctcss", "88.5 Hz")]);
        assert!(r.flush_all(3_000).is_empty());
    }
}
