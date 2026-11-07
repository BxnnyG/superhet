//! What is on the air: the serving cell, its neighbours, the calls with their parties and the
//! messages (SDS, status). Pure state machine fed with what `tetmon` parsed from `tetra-rx`; the
//! caller (`mod.rs`) runs it and decides, from the answer of `voice`, what is played.
//!
//! Calls are keyed by the call identifier of the CMCE PDUs; voice arrives by traffic channel
//! (usage marker) and is attached to the call that carries that marker. A call whose set-up was
//! missed (the receiver tuned in late) starts from its first voice frame.
//!
//! Encryption: a call is flagged when the MAC header of one of its PDUs says the air interface
//! encryption is on. Nothing is decrypted, and the content of such PDUs is not interpreted
//! (`tetra-rx` runs without `-e`), so the parties of an encrypted call are often unknown.

use std::collections::VecDeque;

use serde_json::{Map, Value, json};

use super::tetmon::{Message, SysInfo, service_names};

/// A call with no sign of life for this long is over (late-entry set-up repeats come more often).
pub const IDLE_END_MS: u64 = 12_000;
/// Without a burst or cell message for this long the receiver has lost the cell.
pub const SYNC_LOST_MS: u64 = 4_000;
/// Length of speech in one traffic slot block (one ETSI speech frame).
pub const VOICE_FRAME_MS: u64 = 60;
/// Usage markers up to this value are not traffic (EN 300 392-2 clause 21.4.7.2).
pub const LAST_CONTROL_USAGE: u32 = 3;
/// Identical SDS or status messages within this window are one message (the cell repeats them).
const REPEAT_WINDOW_MS: u64 = 5_000;
/// Neighbour cells kept in the status document.
const MAX_NEIGHBOURS: usize = 16;

/// An event to publish.
#[derive(Clone, Debug, PartialEq)]
pub struct Emit {
    pub kind: &'static str,
    pub data: Value,
}

/// What to do with one voice block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Voice {
    /// Decode and play it.
    Play,
    /// The call is encrypted and the instance withholds its audio.
    Withhold,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Cell {
    mcc: u16,
    mnc: u16,
    colour_code: u8,
    la: u16,
    dl_hz: u32,
    ul_hz: u32,
    crypt: u8,
    /// Bit n set: a MAC header with encryption mode n was seen.
    seen: u8,
    services: Option<u16>,
}

impl Cell {
    fn describe(&self) -> Value {
        let mut data = Map::new();
        data.insert("mcc".into(), json!(self.mcc));
        data.insert("mnc".into(), json!(self.mnc));
        data.insert("cc".into(), json!(self.colour_code));
        if self.la != 0 {
            data.insert("la".into(), json!(self.la));
        }
        if self.dl_hz != 0 {
            data.insert("dl_hz".into(), json!(self.dl_hz));
        }
        if self.ul_hz != 0 {
            data.insert("ul_hz".into(), json!(self.ul_hz));
        }
        data.insert(
            "encryption".into(),
            json!(match self.crypt {
                1 => "none",
                2 => "air-interface",
                _ => "unknown",
            }),
        );
        data.insert("encrypted_traffic_seen".into(), json!(self.seen & !1 != 0));
        if let Some(services) = self.services {
            data.insert("services".into(), json!(service_names(services)));
        }
        Value::Object(data)
    }
}

struct Call {
    id: String,
    cid: Option<u32>,
    usage: Option<u32>,
    dst: Option<u32>,
    src: Option<u32>,
    talker: Option<u32>,
    encrypted: bool,
    started_unix_ms: u64,
    started: u64,
    last_activity: u64,
    voice_frames: u64,
    played_frames: u64,
    withheld_frames: u64,
}

impl Call {
    fn describe(&self) -> Map<String, Value> {
        let mut data = Map::new();
        data.insert("call".into(), json!(self.id));
        let mut put = |key: &str, value: Option<u32>| {
            if let Some(value) = value {
                data.insert(key.into(), json!(value));
            }
        };
        put("cid", self.cid);
        put("usage", self.usage);
        put("dst", self.dst);
        put("src", self.src);
        put("talker", self.talker);
        data.insert("encrypted".into(), json!(self.encrypted));
        data
    }

    fn muted(&self, mute_encrypted: bool) -> bool {
        mute_encrypted && self.encrypted
    }
}

pub struct Tracker {
    mute_encrypted: bool,
    cell: Option<Cell>,
    announced: Option<Value>,
    neighbours: Vec<Value>,
    active: Vec<Call>,
    calls: u64,
    last: Option<Value>,
    /// Last burst or cell message (monotonic ms) and whether that counted as "in sync" last time.
    last_sync: Option<u64>,
    in_sync: bool,
    /// Short data and status messages seen lately, for repeat suppression.
    recent: VecDeque<(String, u64)>,
    messages: u64,
    dirty: bool,
}

impl Tracker {
    #[must_use]
    pub fn new(mute_encrypted: bool) -> Self {
        Self {
            mute_encrypted,
            cell: None,
            announced: None,
            neighbours: Vec::new(),
            active: Vec::new(),
            calls: 0,
            last: None,
            last_sync: None,
            in_sync: false,
            recent: VecDeque::new(),
            messages: 0,
            dirty: false,
        }
    }

    /// Feed one parsed datagram. `now` is monotonic ms, `unix_ms` the wall clock. Voice goes
    /// through `voice`.
    pub fn message(&mut self, message: Message, now: u64, unix_ms: u64) -> Vec<Emit> {
        match message {
            Message::Burst => {
                self.sync_seen(now);
                Vec::new()
            }
            Message::NetInfo {
                colour_code,
                mcc,
                mnc,
                dl_hz,
                ul_hz,
                la,
                crypt,
            } => {
                self.sync_seen(now);
                let services = self.cell.as_ref().and_then(|c| c.services);
                let seen = self.cell.as_ref().map_or(0, |c| c.seen);
                // A different cell starts from scratch; the same one keeps what it learned.
                let same = self
                    .cell
                    .as_ref()
                    .is_some_and(|c| (c.mcc, c.mnc, c.colour_code) == (mcc, mnc, colour_code));
                self.update_cell(
                    Cell {
                        mcc,
                        mnc,
                        colour_code,
                        la,
                        dl_hz,
                        ul_hz,
                        crypt,
                        seen: if same { seen } else { 0 },
                        services: if same { services } else { None },
                    },
                    now,
                )
            }
            Message::EncInfo { crypt, seen } => {
                self.sync_seen(now);
                match self.cell.clone() {
                    Some(mut cell) => {
                        cell.seen = seen;
                        if crypt != 0 {
                            cell.crypt = crypt;
                        }
                        self.update_cell(cell, now)
                    }
                    None => Vec::new(),
                }
            }
            Message::FreqInfoCell {
                dl_hz,
                ul_hz,
                mcc,
                mnc,
                la,
            } => self.neighbour(dl_hz, ul_hz, mcc, mnc, la),
            Message::FreqInfoChannel { .. } => Vec::new(),
            Message::Setup {
                usage,
                called,
                calling,
                cid,
            } => self.call_news(
                CallNews {
                    cid: Some(cid),
                    usage: usage_of(usage),
                    dst: Some(called),
                    src: (calling != 0).then_some(calling),
                    talker: None,
                    encrypted: false,
                },
                now,
                unix_ms,
            ),
            Message::Connect { called, usage, cid } => self.call_news(
                CallNews {
                    cid: Some(cid),
                    usage: usage_of(usage),
                    dst: Some(called),
                    src: None,
                    talker: None,
                    encrypted: false,
                },
                now,
                unix_ms,
            ),
            Message::TxGranted {
                called,
                usage,
                cid,
                talker,
            } => self.call_news(
                CallNews {
                    cid: Some(cid),
                    usage: usage_of(usage),
                    dst: Some(called),
                    src: None,
                    talker,
                    encrypted: false,
                },
                now,
                unix_ms,
            ),
            Message::Release { cid, reason, .. } => {
                let mut out = Vec::new();
                if let Some(i) = self.active.iter().position(|c| c.cid == Some(cid)) {
                    out.extend(self.end_at(i, "release", reason.as_deref(), now));
                }
                out
            }
            Message::Pdu {
                ssi,
                usage,
                encrypted,
                ..
            } => {
                if !encrypted {
                    return Vec::new();
                }
                // The MAC header says the PDU is encrypted: note it for the cell and, when it
                // belongs to a traffic channel, for the call on it.
                let mut out = Vec::new();
                if let Some(mut cell) = self.cell.clone() {
                    cell.seen |= 0b10;
                    out.extend(self.update_cell(cell, now));
                }
                if let Some(usage) = usage_of(usage) {
                    out.extend(self.call_news(
                        CallNews {
                            cid: None,
                            usage: Some(usage),
                            dst: (ssi != 0).then_some(ssi),
                            src: None,
                            talker: None,
                            encrypted: true,
                        },
                        now,
                        unix_ms,
                    ));
                }
                out
            }
            Message::Status {
                called,
                calling,
                status,
            } => {
                let key = format!("status:{called}:{calling}:{status}");
                if self.repeated(key, now) {
                    return Vec::new();
                }
                self.messages += 1;
                let mut data = Map::new();
                data.insert("dst".into(), json!(called));
                if calling != 0 {
                    data.insert("src".into(), json!(calling));
                }
                data.insert("status".into(), json!(status));
                vec![Emit {
                    kind: "tetra.status",
                    data: Value::Object(data),
                }]
            }
            Message::Sds {
                called,
                calling,
                text,
                protocol,
                info,
            } => {
                let key = format!("sds:{called:?}:{calling:?}:{info}");
                if self.repeated(key, now) {
                    return Vec::new();
                }
                self.messages += 1;
                let mut data = Map::new();
                if let Some(dst) = called {
                    data.insert("dst".into(), json!(dst));
                }
                if let Some(src) = calling {
                    data.insert("src".into(), json!(src));
                }
                if let Some(protocol) = protocol {
                    data.insert("protocol".into(), json!(protocol));
                }
                if let Some(text) = text {
                    data.insert("text".into(), json!(text));
                }
                data.insert("info".into(), json!(info));
                vec![Emit {
                    kind: "tetra.sds",
                    data: Value::Object(data),
                }]
            }
            Message::Voice { .. } => Vec::new(),
        }
    }

    /// The system information line of the console: the services the cell offers.
    pub fn sysinfo(&mut self, info: SysInfo, now: u64) -> Vec<Emit> {
        let Some(mut cell) = self.cell.clone() else {
            return Vec::new();
        };
        cell.services = Some(info.service_details);
        if info.la != 0 {
            cell.la = info.la;
        }
        if info.dl_hz != 0 {
            cell.dl_hz = info.dl_hz;
            cell.ul_hz = info.ul_hz;
        }
        self.update_cell(cell, now)
    }

    /// One traffic slot of voice arrived on `usage`. Returns what to do with it and the events it
    /// caused (a call whose set-up was missed starts here).
    pub fn voice(&mut self, usage: u32, now: u64, unix_ms: u64) -> (Voice, Vec<Emit>) {
        self.sync_seen(now);
        let mut out = Vec::new();
        let at = match self.active.iter().position(|c| c.usage == Some(usage)) {
            Some(i) => i,
            None => {
                // A set-up that named no traffic channel: the only such call gets this one.
                let unbound: Vec<usize> = self
                    .active
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.usage.is_none())
                    .map(|(i, _)| i)
                    .collect();
                if let [only] = unbound[..] {
                    self.active[only].usage = Some(usage);
                    self.dirty = true;
                    only
                } else {
                    out.extend(self.call_news(
                        CallNews {
                            cid: None,
                            usage: Some(usage),
                            dst: None,
                            src: None,
                            talker: None,
                            encrypted: false,
                        },
                        now,
                        unix_ms,
                    ));
                    self.active
                        .iter()
                        .position(|c| c.usage == Some(usage))
                        .unwrap_or(0)
                }
            }
        };
        let mute = self.mute_encrypted;
        let Some(call) = self.active.get_mut(at) else {
            return (Voice::Play, out);
        };
        call.last_activity = now;
        call.voice_frames += 1;
        if call.muted(mute) {
            call.withheld_frames += 1;
            (Voice::Withhold, out)
        } else {
            (Voice::Play, out)
        }
    }

    /// A voice block was handed to the decoder (so the call has that much audio).
    pub fn played(&mut self, usage: u32) {
        if let Some(call) = self.active.iter_mut().find(|c| c.usage == Some(usage)) {
            call.played_frames += 1;
        }
    }

    /// Time passes; ends calls that went silent and notices a lost cell.
    pub fn tick(&mut self, now: u64) -> Vec<Emit> {
        let sync = self
            .last_sync
            .is_some_and(|at| now.saturating_sub(at) < SYNC_LOST_MS);
        if sync != self.in_sync {
            self.in_sync = sync;
            self.dirty = true;
        }
        let mut out = Vec::new();
        while let Some(i) = self
            .active
            .iter()
            .position(|c| now.saturating_sub(c.last_activity) >= IDLE_END_MS)
        {
            out.extend(self.end_at(i, "lost", None, now));
        }
        out
    }

    /// The adapter is going away.
    pub fn shutdown(&mut self, now: u64) -> Vec<Emit> {
        let mut out = Vec::new();
        while !self.active.is_empty() {
            out.extend(self.end_at(0, "stop", None, now));
        }
        out
    }

    /// Whether the status document changed without an event.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The status document; `tool`, `codec` and `offset_hz` come from the caller.
    #[must_use]
    pub fn status(&self, tool: &Value, codec: &Value, offset_hz: Option<i32>) -> Value {
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
        let mut signal = json!({ "sync": self.in_sync });
        if self.in_sync
            && let Some(offset) = offset_hz
        {
            signal["offset_hz"] = json!(offset);
        }
        json!({
            "tool": tool,
            "codec": codec,
            "signal": signal,
            "cell": self.cell.as_ref().map(Cell::describe),
            "neighbours": self.neighbours,
            "now": now,
            "last": self.last,
            "calls": self.calls,
            "messages": self.messages,
        })
    }

    fn sync_seen(&mut self, now: u64) {
        self.last_sync = Some(now);
        if !self.in_sync {
            self.in_sync = true;
            self.dirty = true;
        }
    }

    fn repeated(&mut self, key: String, now: u64) -> bool {
        while self
            .recent
            .front()
            .is_some_and(|(_, at)| now.saturating_sub(*at) > REPEAT_WINDOW_MS)
        {
            self.recent.pop_front();
        }
        if self.recent.iter().any(|(k, _)| *k == key) {
            return true;
        }
        self.recent.push_back((key, now));
        if self.recent.len() > 64 {
            self.recent.pop_front();
        }
        false
    }

    fn update_cell(&mut self, cell: Cell, _now: u64) -> Vec<Emit> {
        let data = cell.describe();
        self.cell = Some(cell);
        if self.announced.as_ref() == Some(&data) {
            return Vec::new();
        }
        self.announced = Some(data.clone());
        self.dirty = true;
        vec![Emit {
            kind: "tetra.cell",
            data,
        }]
    }

    fn neighbour(
        &mut self,
        dl_hz: u32,
        ul_hz: Option<u32>,
        mcc: Option<u16>,
        mnc: Option<u16>,
        la: Option<u16>,
    ) -> Vec<Emit> {
        // The serving cell's own carrier is not a neighbour.
        if self.cell.as_ref().is_some_and(|c| c.dl_hz == dl_hz) && mcc.is_none() {
            return Vec::new();
        }
        let mut data = Map::new();
        data.insert("dl_hz".into(), json!(dl_hz));
        if let Some(ul) = ul_hz.filter(|v| *v != 0) {
            data.insert("ul_hz".into(), json!(ul));
        }
        if let Some(mcc) = mcc {
            data.insert("mcc".into(), json!(mcc));
        }
        if let Some(mnc) = mnc {
            data.insert("mnc".into(), json!(mnc));
        }
        if let Some(la) = la {
            data.insert("la".into(), json!(la));
        }
        let data = Value::Object(data);
        if self.neighbours.contains(&data) {
            return Vec::new();
        }
        if self.neighbours.len() == MAX_NEIGHBOURS {
            self.neighbours.remove(0);
        }
        self.neighbours.push(data.clone());
        self.dirty = true;
        vec![Emit {
            kind: "tetra.neighbour",
            data,
        }]
    }

    /// News about a call: starts it when it is new, otherwise fills in what it lacked.
    fn call_news(&mut self, news: CallNews, now: u64, unix_ms: u64) -> Vec<Emit> {
        let mut out = Vec::new();
        let at = self
            .active
            .iter()
            .position(|c| news.cid.is_some() && c.cid == news.cid)
            .or_else(|| {
                news.usage
                    .and_then(|u| self.active.iter().position(|c| c.usage == Some(u)))
            });
        if let Some(i) = at {
            let call = &self.active[i];
            // The same traffic channel with another call identifier: the old call is over.
            if news.cid.is_some() && call.cid.is_some() && call.cid != news.cid {
                out.extend(self.end_at(i, "new-call", None, now));
            } else {
                self.refine(i, &news, now);
                return out;
            }
        }
        self.calls += 1;
        let call = Call {
            id: match news.cid {
                Some(cid) => format!("{unix_ms}-{cid}"),
                None => format!("{unix_ms}-u{}", news.usage.unwrap_or(0)),
            },
            cid: news.cid,
            usage: news.usage,
            dst: news.dst,
            src: news.src,
            talker: news.talker,
            encrypted: news.encrypted,
            started_unix_ms: unix_ms,
            started: now,
            last_activity: now,
            voice_frames: 0,
            played_frames: 0,
            withheld_frames: 0,
        };
        out.push(Emit {
            kind: "tetra.call.start",
            data: Value::Object(call.describe()),
        });
        self.active.push(call);
        out
    }

    fn refine(&mut self, i: usize, news: &CallNews, now: u64) {
        let call = &mut self.active[i];
        let before = call.describe();
        call.last_activity = now;
        call.cid = call.cid.or(news.cid);
        call.usage = call.usage.or(news.usage);
        call.dst = call.dst.or(news.dst);
        call.src = call.src.or(news.src);
        if news.talker.is_some() {
            call.talker = news.talker;
        }
        if news.encrypted {
            call.encrypted = true;
        }
        if call.describe() != before {
            self.dirty = true;
        }
    }

    fn end_at(
        &mut self,
        i: usize,
        reason: &'static str,
        detail: Option<&str>,
        now: u64,
    ) -> Vec<Emit> {
        let call = self.active.remove(i);
        let until = if reason == "lost" {
            call.last_activity
        } else {
            now
        };
        let mut data = call.describe();
        data.insert(
            "duration_ms".into(),
            json!(until.saturating_sub(call.started)),
        );
        data.insert("voice_ms".into(), json!(call.voice_frames * VOICE_FRAME_MS));
        data.insert(
            "audio_ms".into(),
            json!(call.played_frames * VOICE_FRAME_MS),
        );
        if call.withheld_frames > 0 {
            data.insert("muted".into(), json!(true));
        }
        data.insert("reason".into(), json!(reason));
        if let Some(detail) = detail.filter(|d| !d.is_empty()) {
            data.insert("detail".into(), json!(detail));
        }
        let data = Value::Object(data);
        self.last = Some(data.clone());
        vec![Emit {
            kind: "tetra.call.end",
            data,
        }]
    }
}

struct CallNews {
    cid: Option<u32>,
    usage: Option<u32>,
    dst: Option<u32>,
    src: Option<u32>,
    talker: Option<u32>,
    encrypted: bool,
}

/// The usage marker when it names a traffic channel.
fn usage_of(marker: u32) -> Option<u32> {
    (marker > LAST_CONTROL_USAGE && marker < 64).then_some(marker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tetra::tetmon::parse_datagram;

    fn feed(t: &mut Tracker, line: &str, now: u64) -> Vec<Emit> {
        let message = parse_datagram(line.as_bytes()).unwrap_or_else(|| panic!("{line:?}"));
        t.message(message, now, 1_700_000_000_000 + now)
    }

    const SETUP: &str =
        "TETMON_begin FUNC:DSETUPDEC IDX:5 SSI:9001 SSI2:2630001 CID:77 NID:0 RX:0 TETMON_end";
    const NETINFO: &str = "TETMON_begin FUNC:NETINFO1 CCODE:01 MCC:0106 MNC:002a DLF:392012500 ULF:382012500 LA:1 CRYPT:1 RX:0 TETMON_end";

    #[test]
    fn the_cell_is_announced_once_and_again_when_it_changes() {
        let mut t = Tracker::new(true);
        let out = feed(&mut t, NETINFO, 1000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "tetra.cell");
        let d = &out[0].data;
        assert_eq!(
            (d["mcc"].as_u64(), d["mnc"].as_u64(), d["cc"].as_u64()),
            (Some(262), Some(42), Some(1))
        );
        assert_eq!(d["encryption"], "none");
        assert_eq!(d["dl_hz"], 392_012_500);
        assert!(feed(&mut t, NETINFO, 2000).is_empty());
        let out = t.sysinfo(
            SysInfo {
                dl_hz: 392_012_500,
                ul_hz: 382_012_500,
                service_details: 0x0262,
                la: 1,
            },
            2100,
        );
        assert_eq!(
            out[0].data["services"],
            json!([
                "priority-cell",
                "system-wide-services",
                "voice",
                "air-interface-encryption"
            ])
        );
        // Another cell resets what was learned about the first.
        let other = NETINFO.replace("MNC:002a", "MNC:0001");
        let out = feed(&mut t, &other, 3000);
        assert_eq!(out[0].data["mnc"], 1);
        assert!(out[0].data.get("services").is_none());
    }

    #[test]
    fn a_group_call_from_setup_to_release() {
        let mut t = Tracker::new(true);
        feed(&mut t, NETINFO, 900);
        let out = feed(&mut t, SETUP, 1000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "tetra.call.start");
        let d = &out[0].data;
        assert_eq!(
            (
                d["dst"].as_u64(),
                d["src"].as_u64(),
                d["usage"].as_u64(),
                d["cid"].as_u64()
            ),
            (Some(9001), Some(2_630_001), Some(5), Some(77))
        );
        assert_eq!(d["encrypted"], false);
        assert_eq!(d["call"], "1700000001000-77");
        // The cell repeats the set-up for late entry.
        assert!(feed(&mut t, SETUP, 4000).is_empty());
        let (action, emits) = t.voice(5, 4100, 0);
        assert_eq!(action, Voice::Play);
        assert!(emits.is_empty());
        t.played(5);
        t.voice(5, 4160, 0);
        t.played(5);
        let out = feed(
            &mut t,
            "TETMON_begin FUNC:DTXGRANTDEC SSI:9001 IDX:5 CID:77 TXGRANT:0 TXPERM:0 ENCC:0 TPTI:1 SSI2:2630002 RX:0 TETMON_end",
            4200,
        );
        assert!(out.is_empty());
        assert!(t.take_dirty());
        assert_eq!(
            t.status(&Value::Null, &Value::Null, None)["now"][0]["talker"],
            2_630_002
        );
        let out = feed(
            &mut t,
            "TETMON_begin FUNC:DRELEASEDEC SSI:9001 CID:77 NID:12 [Call time-out] RX:0 TETMON_end",
            5000,
        );
        assert_eq!(out[0].kind, "tetra.call.end");
        let d = &out[0].data;
        assert_eq!(d["reason"], "release");
        assert_eq!(d["detail"], "Call time-out");
        assert_eq!(
            (
                d["duration_ms"].as_u64(),
                d["voice_ms"].as_u64(),
                d["audio_ms"].as_u64()
            ),
            (Some(4000), Some(120), Some(120))
        );
        assert_eq!(d["talker"], 2_630_002);
        let status = t.status(&Value::Null, &Value::Null, None);
        assert_eq!(status["now"], json!([]));
        assert_eq!(status["calls"], 1);
        assert_eq!(status["last"]["src"], 2_630_001);
    }

    #[test]
    fn voice_without_a_setup_starts_a_call_on_its_traffic_channel() {
        let mut t = Tracker::new(true);
        let (action, emits) = t.voice(9, 1000, 1_700_000_001_000);
        assert_eq!(action, Voice::Play);
        assert_eq!(emits[0].kind, "tetra.call.start");
        assert_eq!(emits[0].data["usage"], 9);
        assert!(emits[0].data.get("cid").is_none());
        // The set-up that arrives later names it.
        let late = SETUP.replace("IDX:5", "IDX:9");
        assert!(feed(&mut t, &late, 2000).is_empty());
        assert_eq!(
            t.status(&Value::Null, &Value::Null, None)["now"][0]["dst"],
            9001
        );
    }

    #[test]
    fn a_setup_without_a_channel_adopts_the_first_voice() {
        let mut t = Tracker::new(true);
        feed(&mut t, &SETUP.replace("IDX:5", "IDX:0"), 1000);
        let (_, emits) = t.voice(12, 1100, 0);
        assert!(emits.is_empty());
        assert_eq!(
            t.status(&Value::Null, &Value::Null, None)["now"][0]["usage"],
            12
        );
    }

    #[test]
    fn encrypted_headers_flag_the_call_and_withhold_its_audio() {
        let mut t = Tracker::new(true);
        feed(&mut t, NETINFO, 900);
        feed(&mut t, SETUP, 1000);
        assert_eq!(t.voice(5, 1100, 0).0, Voice::Play);
        // The cell hears of encrypted traffic; the call itself only gets flagged.
        let out = feed(
            &mut t,
            "TETMON_begin FUNC:D-TX GRANTED SSI:00009001 IDX:005 IDT:4 ENCR:1 RX:0 TETMON_end",
            1200,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "tetra.cell");
        assert_eq!(out[0].data["encrypted_traffic_seen"], true);
        assert_eq!(t.voice(5, 1300, 0).0, Voice::Withhold);
        let out = t.tick(1300 + IDLE_END_MS);
        assert_eq!(out[0].data["encrypted"], true);
        assert_eq!(out[0].data["muted"], true);
        assert_eq!(out[0].data["reason"], "lost");
        assert_eq!(out[0].data["audio_ms"], 0);
        // The cell now says encrypted traffic was seen.
        assert_eq!(
            t.status(&Value::Null, &Value::Null, None)["cell"]["encrypted_traffic_seen"],
            true
        );
    }

    #[test]
    fn encrypted_audio_plays_when_withholding_is_off() {
        let mut t = Tracker::new(false);
        feed(
            &mut t,
            "TETMON_begin FUNC:D-SETUP SSI:00009001 IDX:007 IDT:4 ENCR:2 RX:0 TETMON_end",
            1000,
        );
        assert_eq!(t.voice(7, 1100, 0).0, Voice::Play);
    }

    #[test]
    fn another_call_on_the_same_channel_replaces_the_old_one() {
        let mut t = Tracker::new(true);
        feed(&mut t, SETUP, 1000);
        let out = feed(&mut t, &SETUP.replace("CID:77", "CID:78"), 2000);
        assert_eq!(out[0].kind, "tetra.call.end");
        assert_eq!(out[0].data["reason"], "new-call");
        assert_eq!(out[1].kind, "tetra.call.start");
        assert_eq!(out[1].data["cid"], 78);
    }

    #[test]
    fn short_messages_are_events_and_repeats_are_dropped() {
        let mut t = Tracker::new(true);
        let status =
            "TETMON_begin FUNC:DSTATUSDEC SSI:9001 SSI2:2630001 STATUS:32770 RX:0 TETMON_end";
        let out = feed(&mut t, status, 1000);
        assert_eq!(out[0].kind, "tetra.status");
        assert_eq!(out[0].data["status"], 32770);
        assert!(feed(&mut t, status, 2000).is_empty());
        assert_eq!(feed(&mut t, status, 1000 + REPEAT_WINDOW_MS + 1).len(), 1);
        let sds = "TETMON_begin FUNC:SDSDEC [CPTI:1 CalledSSI:9001 CallingSSI:2630001 CallingEXT:0  UserData4: len:48 protoid:82(TEXT) coding_scheme:01 DATA:[Hi]] RX:0 TETMON_end";
        let out = feed(&mut t, sds, 20_000);
        assert_eq!(out[0].kind, "tetra.sds");
        assert_eq!(out[0].data["text"], "Hi");
        assert_eq!(out[0].data["src"], 2_630_001);
    }

    #[test]
    fn neighbours_are_listed_once_and_the_serving_carrier_is_not_one() {
        let mut t = Tracker::new(true);
        feed(&mut t, NETINFO, 900);
        let n = "TETMON_begin FUNC:FREQINFO1 DLF:392037500 ULF:382037500 MCC:0106 MNC:002a LA:2 RX:0 TETMON_end";
        let out = feed(&mut t, n, 1000);
        assert_eq!(out[0].kind, "tetra.neighbour");
        assert_eq!(out[0].data["la"], 2);
        assert!(feed(&mut t, n, 2000).is_empty());
        assert!(
            feed(
                &mut t,
                "TETMON_begin FUNC:FREQINFO1 DLF:392012500 RX:0 TETMON_end",
                3000
            )
            .is_empty()
        );
        assert_eq!(
            t.status(&Value::Null, &Value::Null, None)["neighbours"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn sync_is_lost_when_bursts_stop() {
        let mut t = Tracker::new(true);
        feed(&mut t, "TETMON_begin FUNC:BURST RX:0 TETMON_end", 1000);
        assert!(t.take_dirty());
        assert_eq!(
            t.status(&Value::Null, &Value::Null, Some(120))["signal"],
            json!({"sync": true, "offset_hz": 120})
        );
        t.tick(2000);
        assert!(!t.take_dirty());
        t.tick(1000 + SYNC_LOST_MS);
        assert!(t.take_dirty());
        assert_eq!(
            t.status(&Value::Null, &Value::Null, Some(120))["signal"],
            json!({"sync": false})
        );
    }

    #[test]
    fn shutdown_closes_open_calls() {
        let mut t = Tracker::new(true);
        feed(&mut t, SETUP, 1000);
        let out = t.shutdown(1100);
        assert_eq!(out[0].data["reason"], "stop");
        assert!(t.shutdown(1200).is_empty());
    }
}
