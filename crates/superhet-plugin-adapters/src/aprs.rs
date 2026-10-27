//! APRS (AX.25 UI frames, Bell 202 AFSK 1200 baud) through direwolf.
//!
//! direwolf reads raw `s16le` mono audio from stdin (`ADEVICE stdin null`) and prints each
//! received frame in the monitor format of TNC2, preceded by a channel tag:
//!
//! ```text
//! [0.3] N0CALL-9>APRS,WIDE1-1,WIDE2-1:!5230.50N/01324.30E>Superhet APRS test
//! ```
//!
//! Bytes outside printable ASCII are shown as `<0x1c>`. The line is split into source,
//! destination, digipeater path and information field; the information field is decoded as far
//! as it is cheap and well defined: plain and compressed positions, objects and items, Mic-E,
//! messages and status reports. Everything else is passed on as `type: "other"` with the raw
//! text, so nothing is lost.

use serde_json::{Map, Value, json};

use crate::event::{MAX_TEXT, event, round5, truncate};

/// The instance has no options; reject any config so typos do not go unnoticed.
///
/// # Errors
/// When `config` is neither `null` nor an empty object.
pub fn check_config(config: &Value) -> Result<(), String> {
    match config {
        Value::Null => Ok(()),
        Value::Object(object) if object.is_empty() => Ok(()),
        _ => Err("this plugin takes no configuration".into()),
    }
}

/// Map one output line of direwolf to an `aprs.packet` event.
#[must_use]
pub fn parse_line(line: &str) -> Option<Value> {
    let line = line.trim_end_matches(['\r', '\n']);
    let rest = line.strip_prefix('[')?;
    let (_, rest) = rest.split_once("] ")?;
    let (header, info_text) = rest.split_once(':')?;
    let (source, tail) = header.split_once('>')?;
    if !is_callsign(source) {
        return None;
    }
    let mut hops = tail.split(',');
    let dest = hops
        .next()
        .filter(|d| is_callsign(d.trim_end_matches('*')))?;
    let path: Vec<&str> = hops.collect();
    if !path.iter().all(|h| is_callsign(h.trim_end_matches('*'))) {
        return None;
    }
    let info = unescape(info_text);

    let mut data = Map::new();
    data.insert("source".into(), json!(source));
    data.insert("dest".into(), json!(dest));
    data.insert("path".into(), json!(path));
    data.insert("info".into(), json!(truncate(info_text, MAX_TEXT)));
    let summary = decode_info(dest, &info, &mut data);
    let summary = match summary {
        Some(text) => format!("{source}: {text}"),
        None => format!("{source}: {}", truncate(info_text, 200)),
    };
    data.insert("summary".into(), json!(summary));
    Some(event("aprs.packet", data))
}

/// Callsign with optional SSID: 1-9 alphanumerics, then `-` and 1-2 alphanumerics.
fn is_callsign(text: &str) -> bool {
    let (call, ssid) = match text.split_once('-') {
        Some((call, ssid)) => (call, Some(ssid)),
        None => (text, None),
    };
    let ok = |s: &str, max: usize| {
        !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_alphanumeric())
    };
    ok(call, 9) && ssid.is_none_or(|s| ok(s, 2))
}

/// Undo direwolf's `<0xNN>` escapes.
fn unescape(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<'
            && bytes.get(i + 1) == Some(&b'0')
            && bytes.get(i + 2) == Some(&b'x')
            && bytes.get(i + 5) == Some(&b'>')
            && let Some(hex) = text.get(i + 3..i + 5)
            && let Ok(value) = u8::from_str_radix(hex, 16)
        {
            out.push(value);
            i += 6;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

fn lossy(bytes: &[u8]) -> String {
    truncate(String::from_utf8_lossy(bytes).trim(), MAX_TEXT)
}

/// Decode the information field into `data`; returns the human summary (without the source).
fn decode_info(dest: &str, info: &[u8], data: &mut Map<String, Value>) -> Option<String> {
    let dti = *info.first()?;
    match dti {
        b'!' | b'=' => position_report(&info[1..], data),
        b'/' | b'@' => position_report(info.get(8..)?, data),
        b'`' | b'\'' | 0x1c | 0x1d => mic_e(dest, info, data),
        b':' => message(info, data),
        b'>' => {
            data.insert("type".into(), json!("status"));
            let text = lossy(&info[1..]);
            data.insert("status".into(), json!(text));
            Some(text)
        }
        b';' => object(info, data),
        b')' => item(info, data),
        b'_' => {
            data.insert("type".into(), json!("weather"));
            None
        }
        b'T' if info.get(1) == Some(&b'#') => {
            data.insert("type".into(), json!("telemetry"));
            None
        }
        b'$' => {
            data.insert("type".into(), json!("nmea"));
            None
        }
        _ => {
            data.insert("type".into(), json!("other"));
            None
        }
    }
}

struct Position {
    lat: f64,
    lon: f64,
    table: char,
    code: char,
    /// Bytes after the position (and symbol).
    rest: usize,
    /// Course/speed extension of a compressed position.
    course_speed: Option<(u16, f64)>,
}

fn put_position(data: &mut Map<String, Value>, pos: &Position, rest: &[u8]) -> String {
    data.insert("lat".into(), json!(round5(pos.lat)));
    data.insert("lon".into(), json!(round5(pos.lon)));
    data.insert("symbol".into(), json!(format!("{}{}", pos.table, pos.code)));
    let mut comment = rest;
    // Plain positions carry "ccc/sss" right after the symbol.
    let mut extension = pos.course_speed;
    if extension.is_none()
        && let Some(ext) = comment.get(..7)
        && ext[3] == b'/'
        && ext[..3].iter().chain(&ext[4..]).all(u8::is_ascii_digit)
    {
        let number = |s: &[u8]| String::from_utf8_lossy(s).parse::<u16>().unwrap_or(0);
        extension = Some((number(&ext[..3]), f64::from(number(&ext[4..]))));
        comment = &comment[7..];
    }
    if let Some((course, speed)) = extension {
        data.insert("course".into(), json!(course));
        data.insert("speed_kn".into(), json!(speed));
    }
    if let Some(feet) = altitude_feet(comment) {
        data.insert(
            "altitude_m".into(),
            json!((f64::from(feet) * 0.3048).round()),
        );
    }
    let comment = lossy(comment);
    if !comment.is_empty() {
        data.insert("comment".into(), json!(comment));
    }
    let mut summary = format!("{:.5}, {:.5}", round5(pos.lat), round5(pos.lon));
    if !comment.is_empty() {
        summary.push(' ');
        summary.push_str(&truncate(&comment, 200));
    }
    summary
}

/// `/A=001234` anywhere in the comment.
fn altitude_feet(comment: &[u8]) -> Option<i32> {
    let at = comment.windows(3).position(|w| w == b"/A=")?;
    let digits = comment.get(at + 3..at + 9)?;
    String::from_utf8_lossy(digits).parse().ok()
}

fn position_report(body: &[u8], data: &mut Map<String, Value>) -> Option<String> {
    data.insert("type".into(), json!("position"));
    let pos = plain_position(body).or_else(|| compressed_position(body))?;
    let rest = body.get(pos.rest..).unwrap_or_default();
    Some(put_position(data, &pos, rest))
}

/// `DDMM.mmN/DDDMM.mmE$` (spaces mark ambiguity and count as zeros).
fn plain_position(body: &[u8]) -> Option<Position> {
    let lat = body.get(..8)?;
    let table = *body.get(8)?;
    let lon = body.get(9..18)?;
    let code = *body.get(18)?;
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(f64::from(b - b'0')),
        b' ' => Some(0.0),
        _ => None,
    };
    if lat[4] != b'.' || lon[5] != b'.' {
        return None;
    }
    let lat_deg = digit(lat[0])? * 10.0 + digit(lat[1])?;
    let lat_min =
        digit(lat[2])? * 10.0 + digit(lat[3])? + (digit(lat[5])? * 10.0 + digit(lat[6])?) / 100.0;
    let lon_deg = digit(lon[0])? * 100.0 + digit(lon[1])? * 10.0 + digit(lon[2])?;
    let lon_min =
        digit(lon[3])? * 10.0 + digit(lon[4])? + (digit(lon[6])? * 10.0 + digit(lon[7])?) / 100.0;
    let mut latitude = lat_deg + lat_min / 60.0;
    let mut longitude = lon_deg + lon_min / 60.0;
    match lat[7] {
        b'N' => {}
        b'S' => latitude = -latitude,
        _ => return None,
    }
    match lon[8] {
        b'E' => {}
        b'W' => longitude = -longitude,
        _ => return None,
    }
    if latitude.abs() > 90.0 || longitude.abs() > 180.0 {
        return None;
    }
    Some(Position {
        lat: latitude,
        lon: longitude,
        table: char::from(table),
        code: char::from(code),
        rest: 19,
        course_speed: None,
    })
}

fn base91(chars: &[u8]) -> Option<f64> {
    let mut value = 0.0;
    for &c in chars {
        if !(33..=124).contains(&c) {
            return None;
        }
        value = value * 91.0 + f64::from(c - 33);
    }
    Some(value)
}

/// Compressed position: symbol table, 4+4 base-91 characters, symbol code, 2 course/speed
/// characters, compression type (13 bytes).
fn compressed_position(body: &[u8]) -> Option<Position> {
    let table = *body.first()?;
    if !(table == b'/'
        || table == b'\\'
        || table.is_ascii_uppercase()
        || (b'a'..=b'j').contains(&table))
    {
        return None;
    }
    let lat = 90.0 - base91(body.get(1..5)?)? / 380_926.0;
    let lon = -180.0 + base91(body.get(5..9)?)? / 190_463.0;
    let code = *body.get(9)?;
    let (c, s) = (*body.get(10)?, *body.get(11)?);
    let course_speed = if (b'!'..=b'z').contains(&c) {
        let course = u16::from(c - 33) * 4;
        let speed = 1.08_f64.powi(i32::from(s) - 33) - 1.0;
        Some((course, (speed * 10.0).round() / 10.0))
    } else {
        None
    };
    // An uncompressed table character 'a'..'j' stands for overlay digits 0-9.
    let table = if (b'a'..=b'j').contains(&table) {
        char::from(b'0' + (table - b'a'))
    } else {
        char::from(table)
    };
    Some(Position {
        lat,
        lon,
        table,
        code: char::from(code),
        rest: 13,
        course_speed,
    })
}

fn object(info: &[u8], data: &mut Map<String, Value>) -> Option<String> {
    data.insert("type".into(), json!("object"));
    let name = lossy(info.get(1..10)?);
    data.insert("name".into(), json!(name));
    data.insert("live".into(), json!(info.get(10) == Some(&b'*')));
    let body = info.get(18..)?;
    let pos = plain_position(body).or_else(|| compressed_position(body))?;
    let rest = body.get(pos.rest..).unwrap_or_default();
    let text = put_position(data, &pos, rest);
    Some(format!("{name} {text}"))
}

fn item(info: &[u8], data: &mut Map<String, Value>) -> Option<String> {
    data.insert("type".into(), json!("item"));
    let end = info
        .iter()
        .take(11)
        .skip(1)
        .position(|&b| b == b'!' || b == b'_')?
        + 1;
    let name = lossy(&info[1..end]);
    data.insert("name".into(), json!(name));
    data.insert("live".into(), json!(info[end] == b'!'));
    let body = info.get(end + 1..)?;
    let pos = plain_position(body).or_else(|| compressed_position(body))?;
    let rest = body.get(pos.rest..).unwrap_or_default();
    let text = put_position(data, &pos, rest);
    Some(format!("{name} {text}"))
}

fn message(info: &[u8], data: &mut Map<String, Value>) -> Option<String> {
    data.insert("type".into(), json!("message"));
    // ":ADDRESSEE:text{id" with the addressee padded to nine characters
    let addressee = lossy(info.get(1..10)?);
    if info.get(10) != Some(&b':') {
        return None;
    }
    let body = &info[11..];
    let (text, id) = match body.iter().rposition(|&b| b == b'{') {
        Some(at) => (&body[..at], Some(lossy(&body[at + 1..]))),
        None => (body, None),
    };
    let text = lossy(text);
    data.insert("addressee".into(), json!(addressee));
    data.insert("message".into(), json!(text));
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        data.insert("message_id".into(), json!(id));
    }
    Some(format!("to {addressee}: {text}"))
}

/// Mic-E: latitude in the destination address, longitude / speed / course in the information
/// field (APRS 1.0.1 chapter 10).
fn mic_e(dest: &str, info: &[u8], data: &mut Map<String, Value>) -> Option<String> {
    data.insert("type".into(), json!("mic-e"));
    let dest = dest.trim_end_matches('*').as_bytes();
    if dest.len() < 6 || info.len() < 9 {
        return None;
    }
    let decode = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'J' => Some(c - b'A'),
        b'P'..=b'Y' => Some(c - b'P'),
        b'K' | b'L' | b'Z' => Some(0), // position ambiguity
        _ => None,
    };
    let mut digits = [0_u8; 6];
    for (digit, &c) in digits.iter_mut().zip(dest) {
        *digit = decode(c)?;
    }
    // The N/S, longitude offset and E/W bits are set by the characters P-Z.
    let flags: Vec<bool> = dest[..6].iter().map(|&c| c >= b'P').collect();
    let lat_deg = f64::from(digits[0]) * 10.0 + f64::from(digits[1]);
    let lat_min = f64::from(digits[2]) * 10.0
        + f64::from(digits[3])
        + (f64::from(digits[4]) * 10.0 + f64::from(digits[5])) / 100.0;
    let mut latitude = lat_deg + lat_min / 60.0;
    if !flags[3] {
        latitude = -latitude;
    }
    let mut degrees = i32::from(info[1]) - 28;
    if flags[4] {
        degrees += 100;
    }
    if degrees >= 190 {
        degrees -= 190;
    } else if degrees >= 180 {
        degrees -= 80;
    }
    let mut minutes = i32::from(info[2]) - 28;
    if minutes >= 60 {
        minutes -= 60;
    }
    let hundredths = i32::from(info[3]) - 28;
    let mut longitude =
        f64::from(degrees) + (f64::from(minutes) + f64::from(hundredths) / 100.0) / 60.0;
    if flags[5] {
        longitude = -longitude;
    }
    let speed_tens = i32::from(info[4]) - 28;
    let combined = i32::from(info[5]) - 28;
    let course_low = i32::from(info[6]) - 28;
    let mut speed = speed_tens * 10 + combined / 10;
    if speed >= 800 {
        speed -= 800;
    }
    let mut course = (combined % 10) * 100 + course_low;
    if course >= 400 {
        course -= 400;
    }
    if latitude.abs() > 90.0 || longitude.abs() > 180.0 || !(0..=360).contains(&course) {
        return None;
    }
    let pos = Position {
        lat: latitude,
        lon: longitude,
        table: char::from(info[8]),
        code: char::from(info[7]),
        rest: 0,
        course_speed: Some((u16::try_from(course).ok()?, f64::from(speed))),
    };
    let text = put_position(data, &pos, info.get(9..).unwrap_or_default());
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(line: &str) -> Value {
        parse_line(line).expect("parsed")["data"].clone()
    }

    #[test]
    fn plain_position_with_comment() {
        let d = data("[0.3] N0CALL-9>APRS,WIDE1-1,WIDE2-1:!5230.50N/01324.30E>Superhet APRS test");
        assert_eq!(d["source"], "N0CALL-9");
        assert_eq!(d["dest"], "APRS");
        assert_eq!(d["path"], json!(["WIDE1-1", "WIDE2-1"]));
        assert_eq!(d["type"], "position");
        assert_eq!(d["lat"], 52.50833);
        assert_eq!(d["lon"], 13.405);
        assert_eq!(d["symbol"], "/>");
        assert_eq!(d["comment"], "Superhet APRS test");
        assert_eq!(
            d["summary"],
            "N0CALL-9: 52.50833, 13.40500 Superhet APRS test"
        );
    }

    #[test]
    fn southern_western_with_extension_and_altitude() {
        let d = data("[0L] N0CALL>APRS:=3345.00S/07012.00W>088/036Rolling/A=001000");
        assert_eq!(d["lat"], -33.75);
        assert_eq!(d["lon"], -70.2);
        assert_eq!(d["course"], 88);
        assert_eq!(d["speed_kn"], 36.0);
        assert_eq!(d["altitude_m"], 305.0);
        assert_eq!(d["comment"], "Rolling/A=001000");
    }

    #[test]
    fn position_with_timestamp() {
        let d = data("[0.4] N0CALL>APRS,WIDE2-1*:@092345z4903.50N/07201.75W>Test");
        assert_eq!(d["path"], json!(["WIDE2-1*"]));
        assert_eq!(d["lat"], 49.05833);
        assert_eq!(d["lon"], -72.02917);
    }

    #[test]
    fn compressed_position_example() {
        // APRS 1.0.1 chapter 9 example: 49 30.00 N, 72 45.00 W
        let d = data("[0.3] N0CALL>APRS:!/5L!!<*e7> sT");
        assert_eq!(d["type"], "position");
        assert_eq!(d["lat"], 49.5);
        assert_eq!(d["lon"], -72.75);
        assert_eq!(d["symbol"], "/>");
    }

    #[test]
    fn message_with_id() {
        let d = data("[0.3] N0CALL-7>APRS,WIDE2-1::N0CALL-1 :Hello from Superhet{42");
        assert_eq!(d["type"], "message");
        assert_eq!(d["addressee"], "N0CALL-1");
        assert_eq!(d["message"], "Hello from Superhet");
        assert_eq!(d["message_id"], "42");
        assert_eq!(d["summary"], "N0CALL-7: to N0CALL-1: Hello from Superhet");
    }

    #[test]
    fn status_and_unknown() {
        let d = data("[0.3] N0CALL>APRS:>Testing without digipeaters");
        assert_eq!(d["type"], "status");
        assert_eq!(d["status"], "Testing without digipeaters");
        let d = data("[0.3] N0CALL>APRS:{ user defined");
        assert_eq!(d["type"], "other");
        assert_eq!(d["summary"], "N0CALL: { user defined");
    }

    #[test]
    fn object_and_item() {
        let d = data("[0.3] N0CALL>APRS:;LEADER   *092345z4903.50N/07201.75W>088/036");
        assert_eq!(d["type"], "object");
        assert_eq!(d["name"], "LEADER");
        assert_eq!(d["live"], true);
        assert_eq!(d["lat"], 49.05833);
        let d = data("[0.3] N0CALL>APRS:)AID #2!4903.50N/07201.75WA");
        assert_eq!(d["type"], "item");
        assert_eq!(d["name"], "AID #2");
    }

    #[test]
    fn mic_e_example() {
        // 52 30.50 N 013 24.30 E, 20 kn, course 251, symbol "/>" (built by
        // scripts/gen-plugin-recordings.py; direwolf decodes the frame to the same values).
        let d = data("[0.3] N0CALL-5>523P50,WIDE1-1:`)4:<0x1e><0x1e>O>/Mic-E test");
        assert_eq!(d["type"], "mic-e");
        assert_eq!(d["lat"], 52.50833);
        assert_eq!(d["lon"], 13.405);
        assert_eq!(d["speed_kn"], 20.0);
        assert_eq!(d["course"], 251);
        assert_eq!(d["symbol"], "/>");
        assert_eq!(d["comment"], "Mic-E test");
    }

    #[test]
    fn escapes_are_undone() {
        assert_eq!(unescape("a<0x1c>b<0xff>"), vec![b'a', 0x1c, b'b', 0xff]);
        assert_eq!(unescape("<0xzz><0x1"), b"<0xzz><0x1".to_vec());
    }

    #[test]
    fn rejects_non_frames() {
        assert!(parse_line("Dire Wolf version 1.6").is_none());
        assert!(parse_line("N0CALL audio level = 80(42/41)   [NONE]   |||||||__").is_none());
        assert!(parse_line("Position, normal car (side view)").is_none());
        assert!(parse_line("[0.3] not a callsign>APRS:!x").is_none());
        assert!(parse_line("[0.3] N0CALL>APRS,bad path!:x").is_none());
    }

    #[test]
    fn config_check() {
        assert!(check_config(&Value::Null).is_ok());
        assert!(check_config(&json!({})).is_ok());
        assert!(check_config(&json!({"a": 1})).is_err());
    }
}
