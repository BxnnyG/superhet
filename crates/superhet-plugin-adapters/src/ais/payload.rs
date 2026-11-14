//! AIS sentences (NMEA 0183 `!AIVDM`) and the message types a map needs (ITU-R M.1371-5).
//!
//! AIS-catcher prints one `!AIVDM,<count>,<number>,<seq>,<channel>,<payload>,<fill>*<checksum>`
//! line per received message (several lines for long ones). [`parse_sentence`] checks and splits
//! a line, [`Fragments`] joins the parts of a multi-sentence message, and [`decode`] turns the
//! 6-bit armoured payload into a [`Message`]. Only what the map and the vessel list show is
//! decoded: position reports of class A (1, 2, 3), class B (18, 19) and long range (27)
//! transponders, base stations (4, 11), search-and-rescue aircraft (9), aids to navigation (21)
//! and the static data of ships (5, 24).

use std::collections::HashMap;

/// Kind of station behind an MMSI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Class {
    /// Class A transponder (SOLAS ships).
    #[default]
    A,
    /// Class B transponder (small craft).
    B,
    /// Shore base station.
    Base,
    /// Aid to navigation (buoy, light, beacon).
    Aton,
    /// Search-and-rescue aircraft.
    Sar,
}

impl Class {
    /// Short code used in events.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::Base => "base",
            Self::Aton => "aton",
            Self::Sar => "sar",
        }
    }
}

/// What one AIS message tells about a station; `None` means "not in this message" or "not
/// available" (the AIS "not available" values are mapped to `None`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Message {
    pub msg_type: u8,
    pub mmsi: u32,
    pub class: Class,
    /// Navigational status (0 to 15).
    pub status: Option<u8>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// Speed over ground, knots.
    pub sog_kn: Option<f64>,
    /// Course over ground, degrees.
    pub cog_deg: Option<f64>,
    /// True heading, degrees.
    pub heading_deg: Option<u16>,
    /// Altitude of a search-and-rescue aircraft, metres.
    pub altitude_m: Option<u16>,
    pub name: Option<String>,
    pub callsign: Option<String>,
    pub imo: Option<u32>,
    /// Ship and cargo type (20 to 99), or the aid-to-navigation type for `Class::Aton`.
    pub ship_type: Option<u8>,
    pub destination: Option<String>,
    /// `MM-DD HH:MM` (UTC) as reported; an hour or minute that is "not available" is left out.
    pub eta: Option<String>,
    pub draught_m: Option<f64>,
    pub length_m: Option<u16>,
    pub beam_m: Option<u16>,
}

/// One checked `!AIVDM` sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentence {
    pub count: u8,
    pub number: u8,
    /// Sequence id joining the parts of a message (may be empty).
    pub seq: String,
    pub channel: String,
    pub payload: String,
    pub fill: u8,
}

/// Parse one line of AIS-catcher's NMEA output. `None` for anything else (banner lines,
/// statistics, sentences with a wrong checksum or malformed fields).
#[must_use]
pub fn parse_sentence(line: &str) -> Option<Sentence> {
    let line = line.trim();
    let body = line.strip_prefix('!')?;
    let (body, checksum) = body.rsplit_once('*')?;
    let expected = u8::from_str_radix(checksum.get(..2)?, 16).ok()?;
    if body.bytes().fold(0u8, |acc, b| acc ^ b) != expected {
        return None;
    }
    let mut fields = body.split(',');
    let talker = fields.next()?;
    // `VDM`: received from other stations (`VDO` would be the own ship, which a receiver has not).
    if !(talker.len() == 5 && talker.ends_with("VDM")) {
        return None;
    }
    let count = fields.next()?.parse::<u8>().ok()?;
    let number = fields.next()?.parse::<u8>().ok()?;
    let seq = fields.next()?.to_owned();
    let channel = fields.next()?.to_owned();
    let payload = fields.next()?.to_owned();
    let fill = fields.next()?.parse::<u8>().ok()?;
    if count == 0 || number == 0 || number > count || fill > 5 || payload.is_empty() {
        return None;
    }
    Some(Sentence {
        count,
        number,
        seq,
        channel,
        payload,
        fill,
    })
}

/// Collects the parts of multi-sentence messages (a ship's static data takes two).
#[derive(Debug, Default)]
pub struct Fragments {
    pending: HashMap<(String, String), Pending>,
}

#[derive(Debug)]
struct Pending {
    count: u8,
    next: u8,
    payload: String,
}

/// Parts of different messages waiting for their rest; the oldest are dropped beyond this.
const MAX_PENDING: usize = 16;

impl Fragments {
    /// Feed a sentence; returns the complete payload (with its fill bits) once all parts are in.
    pub fn push(&mut self, sentence: Sentence) -> Option<(String, u8)> {
        if sentence.count == 1 {
            return Some((sentence.payload, sentence.fill));
        }
        let key = (sentence.seq, sentence.channel);
        if sentence.number == 1 {
            if self.pending.len() >= MAX_PENDING {
                self.pending.clear();
            }
            self.pending.insert(
                key,
                Pending {
                    count: sentence.count,
                    next: 2,
                    payload: sentence.payload,
                },
            );
            return None;
        }
        let entry = self.pending.get_mut(&key)?;
        if entry.count != sentence.count || entry.next != sentence.number {
            self.pending.remove(&key);
            return None;
        }
        entry.payload.push_str(&sentence.payload);
        entry.next += 1;
        if sentence.number == sentence.count {
            let done = self.pending.remove(&key)?;
            return Some((done.payload, sentence.fill));
        }
        None
    }
}

/// The payload as a bit string.
struct Bits {
    bits: Vec<u8>,
}

impl Bits {
    fn from_payload(payload: &str, fill: u8) -> Option<Self> {
        let mut bits = Vec::with_capacity(payload.len() * 6);
        for c in payload.bytes() {
            let mut v = c.checked_sub(48)?;
            if v > 40 {
                v = v.checked_sub(8)?;
            }
            if v > 63 {
                return None;
            }
            for shift in (0..6).rev() {
                bits.push((v >> shift) & 1);
            }
        }
        let keep = bits.len().checked_sub(usize::from(fill))?;
        bits.truncate(keep);
        Some(Self { bits })
    }

    fn len(&self) -> usize {
        self.bits.len()
    }

    /// Unsigned field `[start, start + len)`; a field that reaches past the end is `None`.
    fn u(&self, start: usize, len: usize) -> Option<u64> {
        let slice = self.bits.get(start..start.checked_add(len)?)?;
        Some(slice.iter().fold(0u64, |acc, b| (acc << 1) | u64::from(*b)))
    }

    /// Two's complement field.
    fn i(&self, start: usize, len: usize) -> Option<i64> {
        let raw = i64::try_from(self.u(start, len)?).ok()?;
        Some(if (raw >> (len - 1)) & 1 == 1 {
            raw - (1i64 << len)
        } else {
            raw
        })
    }

    fn small(&self, start: usize, len: usize) -> Option<u16> {
        self.u(start, len).and_then(|v| u16::try_from(v).ok())
    }

    /// 6-bit text of up to `chars` characters, trailing `@` padding and blanks removed; `None`
    /// when nothing is left. A field cut short by the end of the message reads as far as it goes.
    fn text(&self, start: usize, chars: usize) -> Option<String> {
        let mut out = String::new();
        for n in 0..chars {
            let Some(v) = self.u(start + n * 6, 6) else {
                break;
            };
            let v = u8::try_from(v).ok()?;
            out.push(char::from(if v < 32 { v + 64 } else { v }));
        }
        let trimmed = out.trim_end_matches(['@', ' ']).trim_start();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }
}

/// Degrees from `raw` units; `None` for the "not available" value or out of range.
fn coordinate(raw: Option<i64>, unavailable: i64, per_degree: f64, limit: f64) -> Option<f64> {
    let raw = raw?;
    if raw == unavailable {
        return None;
    }
    let degrees = raw as f64 / per_degree;
    (degrees.abs() <= limit).then_some((degrees * 1e5).round() / 1e5)
}

/// Longitude and latitude in the 1/10000 minute encoding; both or neither.
fn position(bits: &Bits, lon_at: usize, lat_at: usize) -> (Option<f64>, Option<f64>) {
    let lon = coordinate(bits.i(lon_at, 28), 181 * 600_000, 600_000.0, 180.0);
    let lat = coordinate(bits.i(lat_at, 27), 91 * 600_000, 600_000.0, 90.0);
    if lon.is_some() && lat.is_some() {
        (lon, lat)
    } else {
        (None, None)
    }
}

fn tenths(raw: Option<u64>, unavailable: u64) -> Option<f64> {
    raw.filter(|v| *v != unavailable).map(|v| v as f64 / 10.0)
}

/// Length and beam from the four reference-point distances at `start`.
fn dimensions(bits: &Bits, start: usize) -> (Option<u16>, Option<u16>) {
    let sum = |a: Option<u16>, b: Option<u16>| match (a, b) {
        (Some(a), Some(b)) if a + b > 0 => Some(a + b),
        _ => None,
    };
    (
        sum(bits.small(start, 9), bits.small(start + 9, 9)),
        sum(bits.small(start + 18, 6), bits.small(start + 24, 6)),
    )
}

fn ship_type(raw: Option<u16>) -> Option<u8> {
    raw.and_then(|v| u8::try_from(v).ok()).filter(|v| *v != 0)
}

fn heading(raw: Option<u16>) -> Option<u16> {
    raw.filter(|v| *v < 360)
}

fn eta(bits: &Bits, start: usize) -> Option<String> {
    let month = bits.u(start, 4)?;
    let day = bits.u(start + 4, 5)?;
    let hour = bits.u(start + 9, 5)?;
    let minute = bits.u(start + 14, 6)?;
    if month == 0 || day == 0 {
        return None;
    }
    let mut out = format!("{month:02}-{day:02}");
    if hour < 24 && minute < 60 {
        out.push_str(&format!(" {hour:02}:{minute:02}"));
    }
    Some(out)
}

/// Decode a complete payload. `None` for message types the plugin does not use, truncated
/// messages and an MMSI of zero.
#[must_use]
pub fn decode(payload: &str, fill: u8) -> Option<Message> {
    let bits = Bits::from_payload(payload, fill)?;
    let msg_type = u8::try_from(bits.u(0, 6)?).ok()?;
    let mmsi = u32::try_from(bits.u(8, 30)?).ok().filter(|m| *m != 0)?;
    let mut m = Message {
        msg_type,
        mmsi,
        ..Message::default()
    };
    match msg_type {
        1..=3 => {
            if bits.len() < 137 {
                return None;
            }
            m.status = bits.small(38, 4).and_then(|v| u8::try_from(v).ok());
            m.sog_kn = tenths(bits.u(50, 10), 1023);
            (m.lon, m.lat) = position(&bits, 61, 89);
            m.cog_deg = tenths(bits.u(116, 12), 3600);
            m.heading_deg = heading(bits.small(128, 9));
        }
        4 | 11 => {
            if bits.len() < 134 {
                return None;
            }
            m.class = Class::Base;
            (m.lon, m.lat) = position(&bits, 79, 107);
        }
        5 => {
            if bits.len() < 420 {
                return None;
            }
            m.imo = bits
                .u(40, 30)
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v != 0);
            m.callsign = bits.text(70, 7);
            m.name = bits.text(112, 20);
            m.ship_type = ship_type(bits.small(232, 8));
            (m.length_m, m.beam_m) = dimensions(&bits, 240);
            m.eta = eta(&bits, 274);
            m.draught_m = tenths(bits.u(294, 8), 0);
            m.destination = bits.text(302, 20);
        }
        9 => {
            if bits.len() < 133 {
                return None;
            }
            m.class = Class::Sar;
            m.altitude_m = bits.small(38, 12).filter(|v| *v != 4095);
            m.sog_kn = bits.u(50, 10).filter(|v| *v != 1023).map(|v| v as f64);
            (m.lon, m.lat) = position(&bits, 61, 89);
            m.cog_deg = tenths(bits.u(116, 12), 3600);
        }
        18 | 19 => {
            if bits.len() < 133 {
                return None;
            }
            m.class = Class::B;
            m.sog_kn = tenths(bits.u(46, 10), 1023);
            (m.lon, m.lat) = position(&bits, 57, 85);
            m.cog_deg = tenths(bits.u(112, 12), 3600);
            m.heading_deg = heading(bits.small(124, 9));
            if msg_type == 19 && bits.len() >= 300 {
                m.name = bits.text(143, 20);
                m.ship_type = ship_type(bits.small(263, 8));
                (m.length_m, m.beam_m) = dimensions(&bits, 271);
            }
        }
        21 => {
            if bits.len() < 272 {
                return None;
            }
            m.class = Class::Aton;
            m.ship_type = bits.small(38, 5).and_then(|v| u8::try_from(v).ok());
            m.name = bits.text(43, 20);
            (m.lon, m.lat) = position(&bits, 164, 192);
            (m.length_m, m.beam_m) = dimensions(&bits, 219);
            // A name longer than 20 characters continues after the fixed part.
            if bits.len() > 272
                && let Some(name) = m.name.as_mut()
                && name.chars().count() == 20
                && let Some(more) = bits.text(272, (bits.len() - 272) / 6)
            {
                name.push_str(&more);
            }
        }
        24 => {
            m.class = Class::B;
            match bits.u(38, 2)? {
                0 => {
                    if bits.len() < 160 {
                        return None;
                    }
                    m.name = bits.text(40, 20);
                }
                1 => {
                    if bits.len() < 162 {
                        return None;
                    }
                    m.ship_type = ship_type(bits.small(40, 8));
                    m.callsign = bits.text(90, 7);
                    (m.length_m, m.beam_m) = dimensions(&bits, 132);
                }
                _ => return None,
            }
        }
        27 => {
            if bits.len() < 94 {
                return None;
            }
            m.status = bits.small(40, 4).and_then(|v| u8::try_from(v).ok());
            // 1/10 minute units in 18 and 17 bits.
            let lon = coordinate(bits.i(44, 18), 181 * 600, 600.0, 180.0);
            let lat = coordinate(bits.i(62, 17), 91 * 600, 600.0, 90.0);
            if let (Some(lon), Some(lat)) = (lon, lat) {
                m.lon = Some(lon);
                m.lat = Some(lat);
            }
            m.sog_kn = bits.u(79, 6).filter(|v| *v != 63).map(|v| v as f64);
            m.cog_deg = bits.u(85, 9).filter(|v| *v != 511).map(|v| v as f64);
        }
        _ => return None,
    }
    Some(m)
}

/// Human text of a navigational status.
#[must_use]
pub fn status_text(status: u8) -> &'static str {
    match status {
        0 => "Under way using engine",
        1 => "At anchor",
        2 => "Not under command",
        3 => "Restricted manoeuvrability",
        4 => "Constrained by her draught",
        5 => "Moored",
        6 => "Aground",
        7 => "Engaged in fishing",
        8 => "Under way sailing",
        11 => "Towing astern",
        12 => "Pushing ahead or towing alongside",
        14 => "AIS-SART / MOB / EPIRB active",
        15 => "Not defined",
        _ => "Reserved",
    }
}

/// Human text of a ship and cargo type (20 to 99).
#[must_use]
pub fn ship_type_text(code: u8) -> &'static str {
    match code {
        20..=29 => "Wing in ground",
        30 => "Fishing",
        31 => "Towing",
        32 => "Towing (large)",
        33 => "Dredging or underwater operations",
        34 => "Diving operations",
        35 => "Military operations",
        36 => "Sailing",
        37 => "Pleasure craft",
        40..=49 => "High-speed craft",
        50 => "Pilot vessel",
        51 => "Search and rescue vessel",
        52 => "Tug",
        53 => "Port tender",
        54 => "Anti-pollution vessel",
        55 => "Law enforcement",
        58 => "Medical transport",
        59 => "Non-combatant ship",
        60..=69 => "Passenger",
        70..=79 => "Cargo",
        80..=89 => "Tanker",
        _ => "Other",
    }
}

/// Human text of an aid-to-navigation type (message 21).
#[must_use]
pub fn aton_type_text(code: u8) -> &'static str {
    match code {
        1 => "Reference point",
        2 => "RACON",
        3 => "Fixed structure off shore",
        5 => "Light without sectors",
        6 => "Light with sectors",
        7 => "Leading light front",
        8 => "Leading light rear",
        9 | 21 => "Cardinal mark north",
        10 | 22 => "Cardinal mark east",
        11 | 23 => "Cardinal mark south",
        12 | 24 => "Cardinal mark west",
        13 | 25 => "Port hand mark",
        14 | 26 => "Starboard hand mark",
        15 | 27 => "Preferred channel port hand",
        16 | 28 => "Preferred channel starboard hand",
        17 | 29 => "Isolated danger",
        18 | 30 => "Safe water",
        19 | 31 => "Special mark",
        20 => "Light vessel / LANBY / rig",
        _ => "Aid to navigation",
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! Builds AIS sentences from fields, for the tests of this and the tracker.

    /// Append `value` as a `len`-bit field.
    pub fn push(bits: &mut Vec<u8>, value: u64, len: usize) {
        for shift in (0..len).rev() {
            // Fields wider than the value are zero padded.
            bits.push(u8::from(shift < 64 && (value >> shift) & 1 == 1));
        }
    }

    pub fn push_signed(bits: &mut Vec<u8>, value: i64, len: usize) {
        let mask = (1u64 << len) - 1;
        push(bits, (value as u64) & mask, len);
    }

    /// `chars` 6-bit characters of `text` (upper case ASCII), padded with `@`.
    pub fn push_text(bits: &mut Vec<u8>, text: &str, chars: usize) {
        let mut bytes: Vec<u8> = text.bytes().collect();
        bytes.resize(chars, b'@');
        for b in bytes {
            push(bits, u64::from(if b >= 64 { b - 64 } else { b }), 6);
        }
    }

    /// Armour `bits` and wrap them in sentence(s) of at most `per` payload characters.
    pub fn sentences(bits: &[u8], seq: &str, per: usize) -> Vec<String> {
        let fill = (6 - bits.len() % 6) % 6;
        let mut padded = bits.to_vec();
        padded.extend(std::iter::repeat_n(0, fill));
        let chars: String = padded
            .chunks(6)
            .map(|c| {
                let v = c.iter().fold(0u8, |a, b| (a << 1) | b);
                char::from(if v < 40 { v + 48 } else { v + 56 })
            })
            .collect();
        let parts: Vec<&str> = chars
            .as_bytes()
            .chunks(per)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            .collect();
        let count = parts.len();
        parts
            .iter()
            .enumerate()
            .map(|(i, part)| {
                let fill = if i + 1 == count { fill } else { 0 };
                let seq = if count > 1 { seq } else { "" };
                let body = format!("AIVDM,{count},{},{seq},A,{part},{fill}", i + 1);
                let sum = body.bytes().fold(0u8, |a, b| a ^ b);
                format!("!{body}*{sum:02X}")
            })
            .collect()
    }

    /// A class A position report of `mmsi` at `lat`/`lon`.
    pub fn position_report(
        mmsi: u32,
        lat: f64,
        lon: f64,
        sog_tenths: u64,
        cog_tenths: u64,
    ) -> String {
        let mut b = Vec::new();
        push(&mut b, 1, 6);
        push(&mut b, 0, 2);
        push(&mut b, u64::from(mmsi), 30);
        push(&mut b, 0, 4);
        push_signed(&mut b, 0, 8);
        push(&mut b, sog_tenths, 10);
        push(&mut b, 1, 1);
        push_signed(&mut b, (lon * 600_000.0).round() as i64, 28);
        push_signed(&mut b, (lat * 600_000.0).round() as i64, 27);
        push(&mut b, cog_tenths, 12);
        push(&mut b, 511, 9);
        push(&mut b, 30, 6);
        push(&mut b, 0, 25);
        sentences(&b, "", 60).remove(0)
    }

    /// The two sentences of a type 5 message.
    pub fn static_report(mmsi: u32, name: &str, destination: &str) -> Vec<String> {
        let mut b = Vec::new();
        push(&mut b, 5, 6);
        push(&mut b, 0, 2);
        push(&mut b, u64::from(mmsi), 30);
        push(&mut b, 0, 2);
        push(&mut b, 9_321_483, 30);
        push_text(&mut b, "PH1234", 7);
        push_text(&mut b, name, 20);
        push(&mut b, 70, 8);
        push(&mut b, 300, 9);
        push(&mut b, 100, 9);
        push(&mut b, 20, 6);
        push(&mut b, 38, 6);
        push(&mut b, 1, 4);
        push(&mut b, 11, 4);
        push(&mut b, 24, 5);
        push(&mut b, 18, 5);
        push(&mut b, 30, 6);
        push(&mut b, 145, 8);
        push_text(&mut b, destination, 20);
        push(&mut b, 0, 2);
        sentences(&b, "3", 40)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    /// A class A position report as gpsd documents it (MMSI 477553000, moored in Seattle).
    const GPSD: &str = "!AIVDM,1,1,,B,177KQJ5000G?tO`K>RA1wUbN0TKH,0*5C";

    fn decode_one(line: &str) -> Option<Message> {
        let s = parse_sentence(line)?;
        decode(&s.payload, s.fill)
    }

    #[test]
    fn gpsd_position_report() {
        let m = decode_one(GPSD).expect("message");
        assert_eq!((m.msg_type, m.mmsi, m.class), (1, 477_553_000, Class::A));
        assert_eq!(m.status, Some(5));
        assert_eq!(m.sog_kn, Some(0.0));
        assert_eq!(m.cog_deg, Some(51.0));
        assert_eq!(m.heading_deg, Some(181));
        let (lat, lon) = (m.lat.expect("lat"), m.lon.expect("lon"));
        assert!((lat - 47.58283).abs() < 1e-4, "{lat}");
        assert!((lon + 122.34583).abs() < 1e-4, "{lon}");
    }

    #[test]
    fn bad_checksum_and_foreign_sentences_are_ignored() {
        assert!(parse_sentence(&GPSD.replace("*5C", "*5D")).is_none());
        assert!(parse_sentence("$GPGGA,1,2,3*00").is_none());
        assert!(parse_sentence("AIS-catcher (build 123)").is_none());
        assert!(parse_sentence("!AIVDM,1,1,,B,,0*00").is_none());
    }

    #[test]
    fn class_a_report_round_trip() {
        let line = position_report(211_234_560, 54.25, -5.5, 123, 2704);
        let m = decode_one(&line).expect("message");
        assert_eq!(m.mmsi, 211_234_560);
        assert_eq!(m.sog_kn, Some(12.3));
        assert_eq!(m.cog_deg, Some(270.4));
        assert_eq!(m.heading_deg, None);
        assert_eq!((m.lat, m.lon), (Some(54.25), Some(-5.5)));
    }

    #[test]
    fn unavailable_position_and_speed_are_none() {
        let mut b = Vec::new();
        push(&mut b, 1, 6);
        push(&mut b, 0, 2);
        push(&mut b, 244_000_001, 30);
        push(&mut b, 15, 4);
        push_signed(&mut b, 0, 8);
        push(&mut b, 1023, 10);
        push(&mut b, 0, 1);
        push_signed(&mut b, 108_600_000, 28);
        push_signed(&mut b, 54_600_000, 27);
        push(&mut b, 3600, 12);
        push(&mut b, 511, 9);
        push(&mut b, 60, 6);
        push(&mut b, 0, 25);
        let m = decode_one(&sentences(&b, "", 60)[0]).expect("message");
        assert_eq!(
            (m.lat, m.lon, m.sog_kn, m.cog_deg),
            (None, None, None, None)
        );
    }

    #[test]
    fn static_data_comes_in_two_sentences() {
        let lines = static_report(244_670_316, "EVER GIVEN", "NLRTM");
        assert_eq!(lines.len(), 2);
        let mut frags = Fragments::default();
        assert!(
            frags
                .push(parse_sentence(&lines[0]).expect("first"))
                .is_none()
        );
        let (payload, fill) = frags
            .push(parse_sentence(&lines[1]).expect("second"))
            .expect("done");
        let m = decode(&payload, fill).expect("message");
        assert_eq!(m.name.as_deref(), Some("EVER GIVEN"));
        assert_eq!(m.callsign.as_deref(), Some("PH1234"));
        assert_eq!(m.imo, Some(9_321_483));
        assert_eq!(m.ship_type, Some(70));
        assert_eq!((m.length_m, m.beam_m), (Some(400), Some(58)));
        assert_eq!(m.destination.as_deref(), Some("NLRTM"));
        assert_eq!(m.eta.as_deref(), Some("11-24 18:30"));
        assert_eq!(m.draught_m, Some(14.5));
    }

    #[test]
    fn lost_fragment_does_not_poison_the_next_message() {
        let mut frags = Fragments::default();
        let part = |number: u8, payload: &str| Sentence {
            count: 2,
            number,
            seq: "1".into(),
            channel: "A".into(),
            payload: payload.into(),
            fill: 0,
        };
        assert!(frags.push(part(1, "AAAA")).is_none());
        assert!(frags.push(part(1, "BBBB")).is_none());
        assert_eq!(frags.push(part(2, "CC")), Some(("BBBBCC".into(), 0)));
        assert!(frags.push(part(2, "DD")).is_none());
    }

    #[test]
    fn class_b_aton_and_base_station() {
        let mut b = Vec::new();
        push(&mut b, 18, 6);
        push(&mut b, 0, 2);
        push(&mut b, 338_123_456, 30);
        push(&mut b, 0, 8);
        push(&mut b, 55, 10); // 5.5 kn
        push(&mut b, 1, 1);
        push_signed(&mut b, 10 * 600_000, 28);
        push_signed(&mut b, -33 * 600_000, 27);
        push(&mut b, 900, 12);
        push(&mut b, 90, 9);
        push(&mut b, 5, 6);
        push(&mut b, 0, 30);
        let m = decode_one(&sentences(&b, "", 60)[0]).expect("class B");
        assert_eq!(
            (m.class, m.sog_kn, m.heading_deg),
            (Class::B, Some(5.5), Some(90))
        );
        assert_eq!((m.lat, m.lon), (Some(-33.0), Some(10.0)));

        let mut b = Vec::new();
        push(&mut b, 21, 6);
        push(&mut b, 0, 2);
        push(&mut b, 992_111_000, 30);
        push(&mut b, 14, 5);
        push_text(&mut b, "FAIRWAY 3", 20);
        push(&mut b, 0, 1);
        push_signed(&mut b, 5 * 600_000, 28);
        push_signed(&mut b, 53 * 600_000, 27);
        push(&mut b, 0, 9 + 9 + 6 + 6 + 4 + 6 + 1 + 8 + 4);
        let m = decode_one(&sentences(&b, "", 60)[0]).expect("aton");
        assert_eq!((m.class, m.ship_type), (Class::Aton, Some(14)));
        assert_eq!(m.name.as_deref(), Some("FAIRWAY 3"));
        assert_eq!(aton_type_text(14), "Starboard hand mark");

        let mut b = Vec::new();
        push(&mut b, 4, 6);
        push(&mut b, 0, 2);
        push(&mut b, 2_320_001, 30);
        push(&mut b, 2026, 14);
        push(&mut b, 11, 4);
        push(&mut b, 14, 5);
        push(&mut b, 9, 5);
        push(&mut b, 30, 6);
        push(&mut b, 0, 6);
        push(&mut b, 1, 1);
        push_signed(&mut b, -3 * 600_000, 28);
        push_signed(&mut b, 55 * 600_000, 27);
        push(&mut b, 0, 34);
        let m = decode_one(&sentences(&b, "", 60)[0]).expect("base station");
        assert_eq!(
            (m.class, m.lat, m.lon),
            (Class::Base, Some(55.0), Some(-3.0))
        );
    }

    #[test]
    fn class_b_static_data_has_two_parts() {
        let mut a = Vec::new();
        push(&mut a, 24, 6);
        push(&mut a, 0, 2);
        push(&mut a, 338_123_456, 30);
        push(&mut a, 0, 2);
        push_text(&mut a, "SEA BREEZE", 20);
        let m = decode_one(&sentences(&a, "", 60)[0]).expect("part A");
        assert_eq!((m.class, m.name.as_deref()), (Class::B, Some("SEA BREEZE")));

        let mut b = Vec::new();
        push(&mut b, 24, 6);
        push(&mut b, 0, 2);
        push(&mut b, 338_123_456, 30);
        push(&mut b, 1, 2);
        push(&mut b, 37, 8);
        push_text(&mut b, "ABC", 3); // vendor id, 18 bits
        push(&mut b, 0, 4 + 20); // model and serial
        push_text(&mut b, "DE1234", 7);
        push(&mut b, 10, 9);
        push(&mut b, 2, 9);
        push(&mut b, 1, 6);
        push(&mut b, 2, 6);
        push(&mut b, 0, 6);
        let m = decode_one(&sentences(&b, "", 60)[0]).expect("part B");
        assert_eq!(m.ship_type, Some(37));
        assert_eq!(m.callsign.as_deref(), Some("DE1234"));
        assert_eq!((m.length_m, m.beam_m), (Some(12), Some(3)));
    }

    #[test]
    fn truncated_and_unused_messages_are_dropped() {
        assert!(decode("15", 0).is_none());
        let mut b = Vec::new();
        push(&mut b, 8, 6); // binary broadcast: not used
        push(&mut b, 0, 2);
        push(&mut b, 123_456_789, 30);
        push(&mut b, 0, 40);
        assert!(decode_one(&sentences(&b, "", 60)[0]).is_none());
        let mut b = Vec::new();
        push(&mut b, 1, 6);
        push(&mut b, 0, 2);
        push(&mut b, 0, 30); // MMSI 0
        push(&mut b, 0, 130);
        assert!(decode_one(&sentences(&b, "", 60)[0]).is_none());
    }
}
