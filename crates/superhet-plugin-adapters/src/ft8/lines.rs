//! Output lines of `jt9` (FT8 / FT4) and `wsprd` (WSPR) as contract events.
//!
//! jt9 prints `HHMMSS SNR DT FREQ MODE-CHAR MESSAGE`, for example
//! `000000  41  0.7 1499 ~  CQ DL1ABC JO62`; wsprd prints `HHMM SNR DT FREQ_MHZ DRIFT MESSAGE`,
//! for example `0200  -5 -0.0   0.001500  0  K1ABC FN42 37` (the frequency is the audio offset
//! in MHz because the adapter does not pass the dial frequency to the tool).

use serde_json::{Map, Value, json};

use super::message::{parties, watts};
use crate::event::{event, truncate};

/// What the adapter knows about a slot when it parses the tool's output.
pub struct SlotInfo<'a> {
    /// `ft8` or `ft4` (the `mode` field of the events).
    pub mode: &'a str,
    /// `YYYY-MM-DDTHH:MM:SS[.mmm]Z` of the slot start.
    pub time: &'a str,
    /// Dial frequency in Hz, when known.
    pub dial_hz: Option<u64>,
}

fn int(token: &str) -> Option<i64> {
    token.parse::<i64>().ok()
}

fn insert_rf(data: &mut Map<String, Value>, dial_hz: Option<u64>, audio_hz: i64) {
    if let Some(dial) = dial_hz
        && let Ok(audio) = u64::try_from(audio_hz)
    {
        data.insert("dial_hz".into(), json!(dial));
        data.insert("rf_hz".into(), json!(dial + audio));
    }
}

/// One jt9 line as an `ft8.message` event; `None` for anything else (`<DecodeFinished>`, blank).
#[must_use]
pub fn parse_jt9(line: &str, slot: &SlotInfo<'_>) -> Option<Value> {
    let mut tokens = line.split_whitespace();
    let time = tokens.next()?;
    if time.len() != 6 || !time.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let snr = int(tokens.next()?)?;
    let dt: f64 = tokens.next()?.parse().ok()?;
    let freq = int(tokens.next()?)?;
    // Decoder type marker: `~` FT8, `+` FT4, `:` JT65, ...
    let marker = tokens.next()?;
    if marker.chars().count() != 1 {
        return None;
    }
    let mut words: Vec<&str> = tokens.collect();
    // `?` marks a low-confidence decode, `aN` the a-priori decoding stage.
    while words.len() > 1
        && words.last().is_some_and(|w| {
            *w == "?" || (w.len() == 2 && w.starts_with('a') && w.as_bytes()[1].is_ascii_digit())
        })
    {
        words.pop();
    }
    if words.is_empty() {
        return None;
    }
    let message = truncate(&words.join(" "), 64);
    let who = parties(&message);

    let mut data = Map::new();
    data.insert("mode".into(), json!(slot.mode));
    data.insert("time".into(), json!(slot.time));
    data.insert("snr".into(), json!(snr));
    data.insert("dt".into(), json!(dt));
    data.insert("freq_hz".into(), json!(freq));
    insert_rf(&mut data, slot.dial_hz, freq);
    data.insert("message".into(), json!(message));
    data.insert("cq".into(), json!(who.cq));
    if let Some(call) = &who.callsign {
        data.insert("callsign".into(), json!(call));
    }
    if let Some(to) = &who.to {
        data.insert("to".into(), json!(to));
    }
    if let Some(grid) = &who.grid {
        data.insert("grid".into(), json!(grid));
    }
    data.insert(
        "summary".into(),
        json!(truncate(
            &format!(
                "{} {} {snr:+} dB {freq} Hz {message}",
                slot.mode.to_uppercase(),
                slot.time.get(11..19).unwrap_or("")
            ),
            256
        )),
    );
    Some(event("ft8.message", data))
}

/// One wsprd line as a `wspr.spot` event; `None` for anything else.
#[must_use]
pub fn parse_wsprd(line: &str, slot: &SlotInfo<'_>) -> Option<Value> {
    let mut tokens = line.split_whitespace();
    let time = tokens.next()?;
    if time.len() != 4 || !time.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let snr = int(tokens.next()?)?;
    let dt: f64 = tokens.next()?.parse().ok()?;
    let freq_mhz: f64 = tokens.next()?.parse().ok()?;
    let drift = int(tokens.next()?)?;
    let words: Vec<&str> = tokens.collect();
    // `K1ABC FN42 37` (type 1), `PJ4/K1ABC 37` (type 2), `<PJ4/K1ABC> FK52UD 37` (type 3).
    let (call, grid, power) = match words.as_slice() {
        [call, power] => (*call, None, *power),
        [call, grid, power] => (*call, Some(*grid), *power),
        _ => return None,
    };
    let power_dbm = int(power)?;
    let call = call.trim_start_matches('<').trim_end_matches('>');
    let audio_hz = (freq_mhz * 1e6).round() as i64;

    let mut data = Map::new();
    data.insert("mode".into(), json!("wspr"));
    data.insert("time".into(), json!(slot.time));
    data.insert("snr".into(), json!(snr));
    data.insert("dt".into(), json!(dt));
    data.insert("freq_hz".into(), json!(audio_hz));
    insert_rf(&mut data, slot.dial_hz, audio_hz);
    data.insert("drift".into(), json!(drift));
    data.insert("callsign".into(), json!(call));
    if let Some(grid) = grid {
        data.insert("grid".into(), json!(grid));
    }
    data.insert("power_dbm".into(), json!(power_dbm));
    data.insert("message".into(), json!(words.join(" ")));
    let place = grid.map_or(String::new(), |g| format!(" {g}"));
    data.insert(
        "summary".into(),
        json!(truncate(
            &format!(
                "WSPR {call}{place} {power_dbm} dBm ({}) {snr:+} dB drift {drift} Hz/min",
                watts(i32::try_from(power_dbm).unwrap_or(0))
            ),
            256
        )),
    );
    Some(event("wspr.spot", data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(dial: Option<u64>) -> SlotInfo<'static> {
        SlotInfo {
            mode: "ft8",
            time: "2026-11-14T12:00:15Z",
            dial_hz: dial,
        }
    }

    #[test]
    fn jt9_cq() {
        let e = parse_jt9(
            "000000  41  0.7 1499 ~  CQ DL1ABC JO62                          ",
            &slot(Some(14_074_000)),
        )
        .expect("event");
        assert_eq!(e["type"], "ft8.message");
        let d = &e["data"];
        assert_eq!(d["mode"], "ft8");
        assert_eq!(d["time"], "2026-11-14T12:00:15Z");
        assert_eq!(d["snr"], 41);
        assert_eq!(d["dt"], 0.7);
        assert_eq!(d["freq_hz"], 1499);
        assert_eq!(d["rf_hz"], 14_075_499u64);
        assert_eq!(d["dial_hz"], 14_074_000u64);
        assert_eq!(d["message"], "CQ DL1ABC JO62");
        assert_eq!(d["callsign"], "DL1ABC");
        assert_eq!(d["grid"], "JO62");
        assert_eq!(d["cq"], true);
        assert!(d.get("to").is_none());
        assert_eq!(d["summary"], "FT8 12:00:15 +41 dB 1499 Hz CQ DL1ABC JO62");
    }

    #[test]
    fn jt9_directed_with_marks_and_negative_values() {
        let e = parse_jt9(
            " 120015 -12 -0.3  820 ~  DL1ABC K1ABC R-05  a2 ",
            &slot(None),
        )
        .expect("event");
        let d = &e["data"];
        assert_eq!(d["snr"], -12);
        assert_eq!(d["dt"], -0.3);
        assert_eq!(d["message"], "DL1ABC K1ABC R-05");
        assert_eq!(d["to"], "DL1ABC");
        assert_eq!(d["callsign"], "K1ABC");
        assert_eq!(d["cq"], false);
        assert!(d.get("rf_hz").is_none());
        let e = parse_jt9("000000  72  0.8 1500 +  CQ DL1ABC JO62 ?", &slot(None)).expect("event");
        assert_eq!(e["data"]["message"], "CQ DL1ABC JO62");
    }

    #[test]
    fn jt9_ignores_everything_else() {
        for line in [
            "<DecodeFinished>   0   1        0",
            "",
            "EOF on input file",
            "000000 x 0.7 1499 ~ CQ",
            "000000  41  0.7 1499 ~",
        ] {
            assert!(parse_jt9(line, &slot(None)).is_none(), "{line:?}");
        }
    }

    #[test]
    fn wsprd_type1() {
        let e = parse_wsprd(
            "0200  -5 -0.0   0.001500  0  K1ABC FN42 37 ",
            &slot(Some(14_095_600)),
        )
        .expect("event");
        assert_eq!(e["type"], "wspr.spot");
        let d = &e["data"];
        assert_eq!(d["mode"], "wspr");
        assert_eq!(d["snr"], -5);
        assert_eq!(d["freq_hz"], 1500);
        assert_eq!(d["rf_hz"], 14_097_100u64);
        assert_eq!(d["drift"], 0);
        assert_eq!(d["callsign"], "K1ABC");
        assert_eq!(d["grid"], "FN42");
        assert_eq!(d["power_dbm"], 37);
        assert_eq!(d["message"], "K1ABC FN42 37");
        assert_eq!(
            d["summary"],
            "WSPR K1ABC FN42 37 dBm (5.0 W) -5 dB drift 0 Hz/min"
        );
    }

    #[test]
    fn wsprd_type2_and_3() {
        let e =
            parse_wsprd("0202  -5 -0.0   0.001450  1  PJ4/K1ABC 37 ", &slot(None)).expect("event");
        assert_eq!(e["data"]["callsign"], "PJ4/K1ABC");
        assert!(e["data"].get("grid").is_none());
        assert_eq!(e["data"]["freq_hz"], 1450);
        let e = parse_wsprd(
            "0204  -5  0.0   0.001550  0  <PJ4/K1ABC> FK52UD 37",
            &slot(None),
        )
        .expect("event");
        assert_eq!(e["data"]["callsign"], "PJ4/K1ABC");
        assert_eq!(e["data"]["grid"], "FK52UD");
        assert_eq!(e["data"]["message"], "<PJ4/K1ABC> FK52UD 37");
    }

    #[test]
    fn wsprd_ignores_everything_else() {
        for line in [
            "<DecodeFinished>",
            "",
            "Error opening file",
            "0200 -5 0.0 0.0015 0 K1ABC",
        ] {
            assert!(parse_wsprd(line, &slot(None)).is_none(), "{line:?}");
        }
    }
}
