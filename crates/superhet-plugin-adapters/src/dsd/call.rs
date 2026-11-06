//! Calls: who is on the air on which slot, when a call starts and ends, and whether its audio may
//! be played. Pure state machine, fed with what `lines` parsed from `dsd-fme` and with the
//! amount of decoded audio; `mod.rs` runs it.
//!
//! DMR has two time slots that can carry independent calls, so several calls can be active at
//! once; one per slot (`slot` is `None` for the single-channel protocols).

use serde_json::{Map, Value, json};

use super::lines::{CallInfo, Line, Protocol, Sync};

/// A call with no sign of life for this long is over (the transmitter vanished without a
/// terminator).
pub const IDLE_END_MS: u64 = 2000;
/// Audio that arrives this soon after a call ended still belongs to it.
pub const TAIL_MS: u64 = 300;
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
    mode: Option<Protocol>,
    slot: Option<u8>,
    src: Option<u32>,
    dst: Option<u32>,
    group: Option<bool>,
    cc: Option<u8>,
    ran: Option<u8>,
    encrypted: bool,
    algorithm: Option<String>,
    started_unix_ms: u64,
    started: u64,
    last_activity: u64,
    voice_samples: u64,
    muted_samples: u64,
}

impl Call {
    fn describe(&self) -> Map<String, Value> {
        let mut data = Map::new();
        data.insert("call".into(), json!(self.id));
        data.insert(
            "mode".into(),
            json!(self.mode.map_or("unknown", Protocol::name)),
        );
        let mut put = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                data.insert(key.into(), value);
            }
        };
        put("slot", self.slot.map(|v| json!(v)));
        put("src", self.src.map(|v| json!(v)));
        put("dst", self.dst.map(|v| json!(v)));
        put("group", self.group.map(|v| json!(v)));
        put("cc", self.cc.map(|v| json!(v)));
        put("ran", self.ran.map(|v| json!(v)));
        put("algorithm", self.algorithm.clone().map(|v| json!(v)));
        data.insert("encrypted".into(), json!(self.encrypted));
        data
    }

    fn muted(&self, mute_encrypted: bool) -> bool {
        mute_encrypted && self.encrypted
    }
}

pub struct Tracker {
    mute_encrypted: bool,
    /// The protocol the instance is set to; `None` = follow what the decoder reports.
    fixed: Option<Protocol>,
    /// What the last sync line said.
    sync: Option<Sync>,
    /// When voice was last seen on the air (a voice sync line).
    voice_seen: Option<u64>,
    /// The last sync line was of a protocol the instance is not set to: call lines that do not
    /// name their protocol belong to it.
    foreign: bool,
    active: Vec<Call>,
    calls: u64,
    last: Option<Value>,
    /// When the last call ended and whether its audio was withheld.
    ended: Option<(u64, bool)>,
    dirty: bool,
}

impl Tracker {
    #[must_use]
    pub fn new(mute_encrypted: bool, fixed: Option<Protocol>) -> Self {
        Self {
            mute_encrypted,
            fixed,
            sync: None,
            voice_seen: None,
            foreign: false,
            active: Vec::new(),
            calls: 0,
            last: None,
            ended: None,
            dirty: false,
        }
    }

    /// Feed one parsed line. `now` is monotonic ms, `unix_ms` the wall clock.
    pub fn line(&mut self, line: Line, now: u64, unix_ms: u64) -> Vec<Emit> {
        match line {
            Line::Sync(sync) => {
                self.sync(sync, now);
                Vec::new()
            }
            Line::Call(info) => self.call_info(info, now, unix_ms),
            Line::End { slot } => self.terminator(slot, now),
        }
    }

    fn sync(&mut self, sync: Sync, now: u64) {
        // A protocol the instance is not set to is not followed.
        self.foreign = self.fixed.is_some_and(|p| p != sync.protocol);
        if self.foreign {
            return;
        }
        self.sync = Some(sync);
        if sync.voice {
            self.voice_seen = Some(now);
            for call in &mut self.active {
                call.last_activity = now;
            }
        }
    }

    fn call_info(&mut self, info: CallInfo, now: u64, unix_ms: u64) -> Vec<Emit> {
        if info.protocol.is_none() && self.foreign {
            return Vec::new();
        }
        let protocol = info
            .protocol
            .or(self.sync.map(|s| s.protocol))
            .or(self.fixed);
        if self
            .fixed
            .is_some_and(|fixed| protocol.is_some_and(|p| p != fixed))
        {
            return Vec::new();
        }
        let mut out = Vec::new();
        let at = self.active.iter().position(|c| c.slot == info.slot);
        // A line without a slot but with encryption news belongs to the only call going on.
        let at = match at {
            None if info.slot.is_none() && info.src.is_none() && info.dst.is_none() => {
                (self.active.len() == 1).then_some(0)
            }
            other => other,
        };
        if let Some(i) = at {
            let call = &self.active[i];
            let differs =
                |old: Option<u32>, new: Option<u32>| old.is_some() && new.is_some() && old != new;
            if differs(call.src, info.src) || differs(call.dst, info.dst) {
                out.extend(self.end_at(i, "new-call", now));
            } else {
                self.active[i].last_activity = now;
                self.refine(i, &info);
                return out;
            }
        }
        // Something to identify the call by is required to start one.
        if info.src.is_none() && info.dst.is_none() {
            return out;
        }
        self.calls += 1;
        let sync = self.sync;
        let call = Call {
            id: match info.slot {
                Some(slot) => format!("{unix_ms}-s{slot}"),
                None => unix_ms.to_string(),
            },
            mode: protocol,
            slot: info.slot,
            src: info.src,
            dst: info.dst,
            group: info.group,
            cc: info.cc.or_else(|| sync.and_then(|s| s.cc)),
            ran: info.ran.or_else(|| sync.and_then(|s| s.ran)),
            encrypted: info.encrypted == Some(true),
            algorithm: info.algorithm,
            started_unix_ms: unix_ms,
            started: now,
            last_activity: now,
            voice_samples: 0,
            muted_samples: 0,
        };
        out.push(Emit {
            kind: "dsd.call.start",
            data: Value::Object(call.describe()),
        });
        self.active.push(call);
        out
    }

    /// Later lines of the same call fill in what the first one lacked.
    fn refine(&mut self, i: usize, info: &CallInfo) {
        let sync = self.sync;
        let call = &mut self.active[i];
        let before = call.describe();
        call.src = call.src.or(info.src);
        call.dst = call.dst.or(info.dst);
        call.group = call.group.or(info.group);
        call.cc = call.cc.or(info.cc).or_else(|| sync.and_then(|s| s.cc));
        call.ran = call.ran.or(info.ran).or_else(|| sync.and_then(|s| s.ran));
        if info.encrypted == Some(true) {
            call.encrypted = true;
        }
        if info.algorithm.is_some() {
            call.algorithm.clone_from(&info.algorithm);
        }
        if call.describe() != before {
            self.dirty = true;
        }
    }

    fn terminator(&mut self, slot: Option<u8>, now: u64) -> Vec<Emit> {
        let mut out = Vec::new();
        while let Some(i) = self
            .active
            .iter()
            .position(|c| slot.is_none() || c.slot == slot)
        {
            out.extend(self.end_at(i, "eos", now));
        }
        out
    }

    /// `samples` of decoded voice (8 kHz) arrived. Returns whether they may be played.
    pub fn audio(&mut self, samples: u64, now: u64) -> bool {
        if self.active.is_empty() {
            // The tail of a call that just ended keeps that call's fate.
            return !self
                .ended
                .is_some_and(|(at, muted)| muted && now.saturating_sub(at) <= TAIL_MS);
        }
        let mute = self.mute_encrypted;
        let mut play = false;
        for call in &mut self.active {
            call.last_activity = now;
            if call.muted(mute) {
                call.muted_samples += samples;
            } else {
                call.voice_samples += samples;
                play = true;
            }
        }
        play
    }

    /// Time passes; ends calls that went silent.
    pub fn tick(&mut self, now: u64) -> Vec<Emit> {
        let mut out = Vec::new();
        while let Some(i) = self
            .active
            .iter()
            .position(|c| now.saturating_sub(c.last_activity) >= IDLE_END_MS)
        {
            out.extend(self.end_at(i, "lost", now));
        }
        out
    }

    /// The adapter is going away.
    pub fn shutdown(&mut self, now: u64) -> Vec<Emit> {
        let mut out = Vec::new();
        while !self.active.is_empty() {
            out.extend(self.end_at(0, "stop", now));
        }
        out
    }

    fn end_at(&mut self, i: usize, reason: &'static str, now: u64) -> Vec<Emit> {
        let call = self.active.remove(i);
        let until = if reason == "lost" {
            call.last_activity
        } else {
            now
        };
        self.ended = Some((now, call.muted(self.mute_encrypted)));
        let mut data = call.describe();
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
            kind: "dsd.call.end",
            data,
        }]
    }

    /// Whether the status document changed without an event (late encryption news).
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The tool part of the status document is added by the caller.
    #[must_use]
    pub fn status(&self, tool: &Value) -> Value {
        let now: Vec<Value> = self
            .active
            .iter()
            .map(|call| {
                let mut data = call.describe();
                data.insert("started_unix_ms".into(), json!(call.started_unix_ms));
                if call.muted(self.mute_encrypted) {
                    data.insert("muted".into(), json!(true));
                }
                Value::Object(data)
            })
            .collect();
        json!({ "tool": tool, "now": now, "last": self.last, "calls": self.calls })
    }
}

#[cfg(test)]
mod tests {
    use super::super::lines::parse_line;
    use super::*;

    fn feed(t: &mut Tracker, text: &str, now: u64) -> Vec<Emit> {
        let line = parse_line(text).unwrap_or_else(|| panic!("{text:?} is not a line"));
        t.line(line, now, 1_700_000_000_000 + now)
    }

    const DMR_S1: &str = "Slot 1 TGT=91 SRC=3101234 FLCO=0x00 FID=0x00 SVC=0x00 Group Voice";

    #[test]
    fn a_dmr_call_starts_plays_and_ends_with_the_terminator() {
        let mut t = Tracker::new(true, None);
        assert!(feed(&mut t, "Sync: +DMR  BS VOICE | Color Code=02", 900).is_empty());
        let out = feed(&mut t, DMR_S1, 1000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "dsd.call.start");
        let d = &out[0].data;
        assert_eq!(d["mode"], "dmr");
        assert_eq!(
            (d["slot"].as_u64(), d["src"].as_u64(), d["dst"].as_u64()),
            (Some(1), Some(3_101_234), Some(91))
        );
        assert_eq!(
            (
                d["group"].as_bool(),
                d["cc"].as_u64(),
                d["encrypted"].as_bool()
            ),
            (Some(true), Some(2), Some(false))
        );
        assert_eq!(d["call"], "1700000001000-s1");
        // The same link control again is the same call.
        assert!(feed(&mut t, DMR_S1, 1360).is_empty());
        assert!(t.audio(320, 1400));
        assert!(t.audio(320, 1440));
        let out = feed(&mut t, "Slot 1 TLC", 1500);
        assert_eq!(out[0].kind, "dsd.call.end");
        assert_eq!(out[0].data["reason"], "eos");
        assert_eq!(out[0].data["duration_ms"], 500);
        assert_eq!(out[0].data["audio_ms"], 80);
        assert_eq!(t.status(&Value::Null)["last"]["src"], 3_101_234);
        assert_eq!(t.status(&Value::Null)["calls"], 1);
        assert_eq!(t.status(&Value::Null)["now"], json!([]));
    }

    #[test]
    fn the_two_dmr_slots_are_independent_calls() {
        let mut t = Tracker::new(true, Some(Protocol::Dmr));
        feed(&mut t, DMR_S1, 1000);
        feed(&mut t, "Slot 2 TGT=9 SRC=77 FLCO=0x00 SVC=0x00 Group", 1010);
        assert_eq!(
            t.status(&Value::Null)["now"].as_array().map(Vec::len),
            Some(2)
        );
        let out = feed(&mut t, "Slot 2 TLC", 1200);
        assert_eq!(out[0].data["src"], 77);
        assert_eq!(
            t.status(&Value::Null)["now"].as_array().map(Vec::len),
            Some(1)
        );
        // Another talker on slot 1 replaces the first.
        let out = feed(&mut t, "Slot 1 TGT=91 SRC=555 FLCO=0x00 Group", 1300);
        assert_eq!(out[0].kind, "dsd.call.end");
        assert_eq!(out[0].data["reason"], "new-call");
        assert_eq!(out[1].kind, "dsd.call.start");
        assert_eq!(out[1].data["src"], 555);
    }

    #[test]
    fn encryption_news_after_the_start_marks_the_call_and_mutes_it() {
        let mut t = Tracker::new(true, None);
        feed(&mut t, DMR_S1, 1000);
        assert!(t.audio(320, 1020));
        assert!(feed(&mut t, "Slot 1 PI Header ALG ID: 0x21 KEY ID: 0x01", 1100).is_empty());
        assert!(t.take_dirty());
        assert!(!t.take_dirty());
        assert!(!t.audio(320, 1120));
        assert_eq!(t.status(&Value::Null)["now"][0]["muted"], true);
        let out = feed(&mut t, "Slot 1 TLC", 1200);
        assert_eq!(out[0].data["encrypted"], true);
        assert_eq!(out[0].data["algorithm"], "0x21");
        assert_eq!(out[0].data["muted"], true);
        // The tail of the withheld call stays withheld.
        assert!(!t.audio(320, 1300));
        assert!(t.audio(320, 2000));
    }

    #[test]
    fn encrypted_audio_plays_when_muting_is_off() {
        let mut t = Tracker::new(false, None);
        feed(&mut t, "Slot 1 TGT=9 SRC=5 FLCO=0x00 SVC=0x40 Group", 1000);
        assert!(t.audio(320, 1020));
        let out = t.tick(10_000);
        assert_eq!(out[0].data["encrypted"], true);
        assert!(out[0].data.get("muted").is_none());
    }

    #[test]
    fn a_vanished_transmitter_ends_by_timeout_at_its_last_sign_of_life() {
        let mut t = Tracker::new(true, None);
        feed(&mut t, "NXDN VCALL Source: 10 Target: 20 RAN: 4", 1000);
        assert!(t.audio(320, 1500));
        assert!(t.tick(2000).is_empty());
        let out = t.tick(1500 + IDLE_END_MS);
        assert_eq!(out[0].data["reason"], "lost");
        assert_eq!(out[0].data["duration_ms"], 500);
        assert_eq!(out[0].data["mode"], "nxdn");
        assert_eq!(out[0].data["ran"], 4);
        assert!(out[0].data.get("slot").is_none());
    }

    #[test]
    fn a_protocol_the_instance_is_not_set_to_is_ignored() {
        let mut t = Tracker::new(true, Some(Protocol::Nxdn));
        assert!(feed(&mut t, "Sync: +DMR  BS VOICE | Color Code=02", 900).is_empty());
        assert!(feed(&mut t, DMR_S1, 1000).is_empty());
        assert_eq!(t.status(&Value::Null)["calls"], 0);
    }

    #[test]
    fn audio_without_a_call_plays_and_shutdown_closes_open_calls() {
        let mut t = Tracker::new(true, None);
        assert!(t.audio(320, 10));
        feed(&mut t, DMR_S1, 1000);
        let out = t.shutdown(1100);
        assert_eq!(out[0].data["reason"], "stop");
        assert!(t.shutdown(1200).is_empty());
    }
}
