//! Calls: which M17 station is on the air, when a call starts and ends, and whether its audio
//! may be played. Pure state machine; the adapter feeds it what `m17-demod` prints and produces
//! (`mod.rs`) and sends what comes back.

use serde_json::{Map, Value, json};

use super::lsf::Lsf;

/// A call with no audio and no link setup frame for this long is over (the transmitter vanished
/// without an end-of-stream marker).
pub const IDLE_END_MS: u64 = 1500;
/// The demodulator prints `EOS` before it writes the audio of the last frame: audio that arrives
/// this soon after a call ended still belongs to it.
pub const TAIL_MS: u64 = 200;
/// Sample rate of the decoded voice.
pub const VOICE_RATE: u64 = 8000;

/// An event to publish.
#[derive(Clone, Debug, PartialEq)]
pub struct Emit {
    pub kind: &'static str,
    pub data: Value,
}

struct Call {
    id: String,
    lsf: Lsf,
    started_unix_ms: u64,
    started: u64,
    last_activity: u64,
    /// Decoded voice samples (8 kHz) so far.
    voice_samples: u64,
    /// Voice samples dropped because the call is encrypted.
    muted_samples: u64,
}

/// What a call looks like in events and in the status document.
fn describe(call: &Call) -> Map<String, Value> {
    let mut data = call.lsf.describe();
    data.insert("call".into(), json!(call.id));
    data
}

pub struct Tracker {
    mute_encrypted: bool,
    current: Option<Call>,
    /// Calls seen since the plugin started.
    calls: u64,
    last: Option<Value>,
    /// When the last call ended and whether its audio was withheld.
    ended: Option<(u64, bool)>,
}

impl Tracker {
    #[must_use]
    pub fn new(mute_encrypted: bool) -> Self {
        Self {
            mute_encrypted,
            current: None,
            calls: 0,
            last: None,
            ended: None,
        }
    }

    /// A link setup frame arrived at `now` (monotonic ms); `unix_ms` is the wall clock.
    pub fn lsf(&mut self, lsf: Lsf, now: u64, unix_ms: u64) -> Vec<Emit> {
        // Packet-mode frames carry no voice; nothing to follow.
        if !lsf.stream {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Some(call) = &mut self.current {
            if call.lsf.same_call(&lsf) {
                // The same call picked up again; a later LSF may carry the position or text the
                // first one did not.
                call.last_activity = now;
                let better = (lsf.position().is_some() && call.lsf.position().is_none())
                    || (lsf.text().is_some() && call.lsf.text().is_none());
                if better {
                    call.lsf = lsf;
                }
                return out;
            }
            out.extend(self.end("new-call", now));
        }
        self.calls += 1;
        let call = Call {
            id: unix_ms.to_string(),
            lsf,
            started_unix_ms: unix_ms,
            started: now,
            last_activity: now,
            voice_samples: 0,
            muted_samples: 0,
        };
        out.push(Emit {
            kind: "m17.call.start",
            data: Value::Object(describe(&call)),
        });
        self.current = Some(call);
        out
    }

    /// `samples` of decoded voice (8 kHz) arrived. Returns whether they may be played.
    pub fn audio(&mut self, samples: u64, now: u64) -> bool {
        let Some(call) = &mut self.current else {
            // The tail of a call that just ended keeps that call's fate.
            return !self
                .ended
                .is_some_and(|(at, muted)| muted && now.saturating_sub(at) <= TAIL_MS);
        };
        call.last_activity = now;
        let mute = self.mute_encrypted && call.lsf.encrypted() == Some(true);
        if mute {
            call.muted_samples += samples;
        } else {
            call.voice_samples += samples;
        }
        !mute
    }

    /// The transmitter's end-of-stream marker.
    pub fn end_of_stream(&mut self, now: u64) -> Vec<Emit> {
        self.end("eos", now)
    }

    /// Time passes; ends a call that went silent.
    pub fn tick(&mut self, now: u64) -> Vec<Emit> {
        match &self.current {
            Some(call) if now.saturating_sub(call.last_activity) >= IDLE_END_MS => {
                self.end("lost", now)
            }
            _ => Vec::new(),
        }
    }

    /// The adapter is going away.
    pub fn shutdown(&mut self, now: u64) -> Vec<Emit> {
        self.end("stop", now)
    }

    fn end(&mut self, reason: &'static str, now: u64) -> Vec<Emit> {
        let Some(call) = self.current.take() else {
            return Vec::new();
        };
        // A call that ended by itself lasted until its last sign of life, not until the timeout.
        let until = if reason == "lost" {
            call.last_activity
        } else {
            now
        };
        self.ended = Some((
            now,
            self.mute_encrypted && call.lsf.encrypted() == Some(true),
        ));
        let mut data = describe(&call);
        data.insert(
            "duration_ms".into(),
            json!(until.saturating_sub(call.started)),
        );
        data.insert(
            "audio_ms".into(),
            json!(call.voice_samples * 1000 / VOICE_RATE),
        );
        if call.muted_samples > 0 {
            data.insert("muted".into(), json!(true));
        }
        data.insert("reason".into(), json!(reason));
        let data = Value::Object(data);
        self.last = Some(data.clone());
        vec![Emit {
            kind: "m17.call.end",
            data,
        }]
    }

    /// The plugin's complete state for the `status` frame.
    #[must_use]
    pub fn status(&self) -> Value {
        let now = self.current.as_ref().map(|call| {
            let mut data = describe(call);
            data.insert("started_unix_ms".into(), json!(call.started_unix_ms));
            if self.mute_encrypted && call.lsf.encrypted() == Some(true) {
                data.insert("muted".into(), json!(true));
            }
            Value::Object(data)
        });
        json!({ "now": now, "last": self.last, "calls": self.calls })
    }

    /// Whether a call is going on.
    #[must_use]
    pub fn active(&self) -> bool {
        self.current.is_some()
    }

    /// Whether audio arriving at `now` should wait for a link setup frame that is about to be
    /// printed: no call, and none that just ended.
    #[must_use]
    pub fn awaiting_call(&self, now: u64) -> bool {
        self.current.is_none()
            && !self
                .ended
                .is_some_and(|(at, _)| now.saturating_sub(at) <= TAIL_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::super::lsf::{Line, parse_line};
    use super::*;

    fn lsf(src: &str, word: u16) -> Lsf {
        let line = format!(
            "SRC: {src}, DEST: ALL, STR:V/V CAN:00, TYPE: 0x{word:04x}, NONCE: 0000000000000000000000000000, CRC: 0000"
        );
        match parse_line(&line) {
            Some(Line::Lsf(l)) => *l,
            other => panic!("{other:?}"),
        }
    }

    const CLEAR: u16 = 1 | (2 << 1);
    const AES: u16 = 1 | (2 << 1) | (2 << 3);

    #[test]
    fn a_call_starts_plays_and_ends_with_the_marker() {
        let mut t = Tracker::new(true);
        let out = t.lsf(lsf("DL1ABC", CLEAR), 1000, 1_700_000_000_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "m17.call.start");
        assert_eq!(out[0].data["src"], "DL1ABC");
        assert_eq!(out[0].data["call"], "1700000000000");
        assert!(t.active());
        assert!(t.audio(320, 1040));
        assert!(t.audio(320, 1080));
        let out = t.end_of_stream(1100);
        assert_eq!(out[0].kind, "m17.call.end");
        assert_eq!(out[0].data["reason"], "eos");
        assert_eq!(out[0].data["duration_ms"], 100);
        assert_eq!(out[0].data["audio_ms"], 80);
        assert!(!t.active());
        assert_eq!(t.status()["last"]["src"], "DL1ABC");
        assert_eq!(t.status()["now"], Value::Null);
        assert_eq!(t.status()["calls"], 1);
    }

    #[test]
    fn the_same_call_again_is_not_a_new_call() {
        let mut t = Tracker::new(true);
        assert_eq!(t.lsf(lsf("DL1ABC", CLEAR), 0, 1).len(), 1);
        assert!(t.lsf(lsf("DL1ABC", CLEAR), 500, 2).is_empty());
        assert_eq!(t.status()["calls"], 1);
    }

    #[test]
    fn another_station_ends_the_previous_call() {
        let mut t = Tracker::new(true);
        t.lsf(lsf("DL1ABC", CLEAR), 0, 1);
        let out = t.lsf(lsf("DL2XYZ", CLEAR), 400, 2);
        let kinds: Vec<_> = out.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["m17.call.end", "m17.call.start"]);
        assert_eq!(out[0].data["reason"], "new-call");
        assert_eq!(out[0].data["src"], "DL1ABC");
        assert_eq!(out[1].data["src"], "DL2XYZ");
    }

    #[test]
    fn silence_ends_the_call_at_its_last_sign_of_life() {
        let mut t = Tracker::new(true);
        t.lsf(lsf("DL1ABC", CLEAR), 0, 1);
        t.audio(320, 400);
        assert!(t.tick(1000).is_empty());
        let out = t.tick(400 + IDLE_END_MS);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data["reason"], "lost");
        assert_eq!(out[0].data["duration_ms"], 400);
        assert!(t.tick(10_000).is_empty());
    }

    #[test]
    fn encrypted_calls_are_muted_but_counted() {
        let mut t = Tracker::new(true);
        let out = t.lsf(lsf("DL1ABC", AES), 0, 1);
        assert_eq!(out[0].data["encrypted"], true);
        assert!(!t.audio(320, 40));
        let end = t.end_of_stream(80);
        assert_eq!(end[0].data["audio_ms"], 0);
        assert_eq!(end[0].data["muted"], true);
    }

    #[test]
    fn encrypted_calls_play_when_muting_is_off() {
        let mut t = Tracker::new(false);
        t.lsf(lsf("DL1ABC", AES), 0, 1);
        assert!(t.audio(320, 40));
    }

    #[test]
    fn the_last_frame_after_an_encrypted_call_is_muted_too() {
        let mut t = Tracker::new(true);
        t.lsf(lsf("DL1ABC", AES), 0, 1);
        t.end_of_stream(80);
        assert!(
            !t.audio(320, 100),
            "audio printed after EOS belongs to the call"
        );
        assert!(!t.awaiting_call(100));
        assert!(t.audio(320, 80 + TAIL_MS + 1), "later audio is nobody's");
        assert!(t.awaiting_call(80 + TAIL_MS + 1));
    }

    #[test]
    fn audio_without_a_call_is_passed_on() {
        let mut t = Tracker::new(true);
        assert!(t.audio(320, 0));
        assert!(t.end_of_stream(10).is_empty());
    }

    #[test]
    fn packet_mode_is_ignored() {
        let mut t = Tracker::new(true);
        let mut l = lsf("DL1ABC", CLEAR);
        l.stream = false;
        assert!(t.lsf(l, 0, 1).is_empty());
        assert!(!t.active());
    }

    #[test]
    fn shutdown_ends_the_call() {
        let mut t = Tracker::new(true);
        t.lsf(lsf("DL1ABC", CLEAR), 0, 1);
        let out = t.shutdown(900);
        assert_eq!(out[0].data["reason"], "stop");
        assert_eq!(out[0].data["duration_ms"], 900);
    }
}
