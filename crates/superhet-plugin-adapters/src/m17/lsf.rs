//! The link setup frame (LSF) of an M17 stream as `m17-demod -l` prints it on stderr, and the
//! TYPE / META fields behind it (M17 specification v2.0, "Link Setup Frame").
//!
//! Upstream prints `SRC: DL1ABC, DEST: DB0XYZ, STR:V/V CAN:03, NONCE: <28 hex digits>, CRC: 167a`
//! (NONCE is the 14-byte META field). The image patches the tool to add `, TYPE: 0x0185` after the
//! CAN (`deploy/docker/m17-demod-type.sed`); with the raw TYPE word the encryption bits and the
//! META contents can be told apart, without it only the stream bit, data type and CAN are known.

use serde_json::{Map, Value, json};

use crate::event::round5;

/// What the stream carries (TYPE bits 1..2 of a stream).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataType {
    Unknown,
    Data,
    Voice,
    VoiceData,
}

impl DataType {
    /// Name used in events.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Data => "data",
            Self::Voice => "voice",
            Self::VoiceData => "voice+data",
        }
    }
}

/// Encryption type (TYPE bits 3..4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encryption {
    None,
    Scrambler,
    Aes,
    Reserved,
}

impl Encryption {
    /// Name used in events.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Scrambler => "scrambler",
            Self::Aes => "aes",
            Self::Reserved => "reserved",
        }
    }
}

/// A decoded link setup frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Lsf {
    pub src: String,
    pub dst: String,
    /// Stream (voice and/or data) rather than packet mode.
    pub stream: bool,
    pub data_type: DataType,
    /// Channel access number, 0..=15.
    pub can: u8,
    /// Encryption, when the raw TYPE word was printed.
    pub encryption: Option<Encryption>,
    /// Encryption subtype (TYPE bits 5..6), when the raw TYPE word was printed.
    pub subtype: Option<u8>,
    /// The 14-byte META field.
    pub meta: Option<[u8; 14]>,
}

/// A position from the GNSS META data (subtype 01 of an unencrypted stream).
#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    pub lat: f64,
    pub lon: f64,
    pub altitude_m: Option<f64>,
    pub speed_kmh: Option<f64>,
    pub bearing_deg: Option<u16>,
    pub radius_m: Option<u32>,
    pub source: &'static str,
    pub station: &'static str,
}

impl Lsf {
    /// Whether the stream is encrypted; `None` when the raw TYPE word was not printed.
    #[must_use]
    pub fn encrypted(&self) -> Option<bool> {
        self.encryption.map(|e| e != Encryption::None)
    }

    /// The position the META field carries, if it is unencrypted GNSS data with a valid fix.
    #[must_use]
    pub fn position(&self) -> Option<Position> {
        if self.encryption != Some(Encryption::None) || self.subtype != Some(1) {
            return None;
        }
        decode_gnss(&self.meta?)
    }

    /// The text the META field carries when it holds a complete one-block message.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        if self.encryption != Some(Encryption::None) || self.subtype != Some(0) {
            return None;
        }
        let meta = self.meta?;
        // Control byte: high nibble = blocks of the message (bit map), low nibble = this block.
        if meta[0] != 0x11 {
            return None;
        }
        let text = String::from_utf8_lossy(&meta[1..]);
        let text = text.trim_end_matches(['\0', ' ']);
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        (!text.is_empty()).then_some(text)
    }

    /// Same call: nothing but the META contents may differ between two LSFs of one stream.
    #[must_use]
    pub fn same_call(&self, other: &Self) -> bool {
        self.src == other.src
            && self.dst == other.dst
            && self.can == other.can
            && self.stream == other.stream
            && self.data_type == other.data_type
            && self.encryption == other.encryption
    }

    /// The fields every call event carries.
    #[must_use]
    pub fn describe(&self) -> Map<String, Value> {
        let mut data = Map::new();
        data.insert("src".into(), json!(self.src));
        data.insert("dst".into(), json!(self.dst));
        data.insert("can".into(), json!(self.can));
        data.insert("mode".into(), json!(self.data_type.name()));
        if let Some(encryption) = self.encryption {
            data.insert("encrypted".into(), json!(encryption != Encryption::None));
            data.insert("encryption".into(), json!(encryption.name()));
        }
        if let Some(pos) = self.position() {
            let mut p = Map::new();
            p.insert("lat".into(), json!(round5(pos.lat)));
            p.insert("lon".into(), json!(round5(pos.lon)));
            if let Some(v) = pos.altitude_m {
                p.insert("altitude_m".into(), json!(v));
            }
            if let Some(v) = pos.speed_kmh {
                p.insert("speed_kmh".into(), json!(v));
            }
            if let Some(v) = pos.bearing_deg {
                p.insert("bearing_deg".into(), json!(v));
            }
            if let Some(v) = pos.radius_m {
                p.insert("radius_m".into(), json!(v));
            }
            p.insert("source".into(), json!(pos.source));
            p.insert("station".into(), json!(pos.station));
            data.insert("position".into(), Value::Object(p));
        }
        if let Some(text) = self.text() {
            data.insert("text".into(), json!(text));
        }
        data
    }
}

fn decode_gnss(meta: &[u8; 14]) -> Option<Position> {
    let validity = meta[1] >> 4;
    if validity & 0x8 == 0 {
        return None;
    }
    let signed24 = |b: &[u8]| -> f64 {
        let raw = (i32::from(b[0]) << 16) | (i32::from(b[1]) << 8) | i32::from(b[2]);
        f64::from(if raw & 0x80_0000 != 0 {
            raw - 0x100_0000
        } else {
            raw
        })
    };
    let lat = signed24(&meta[3..6]) / 8_388_607.0 * 90.0;
    let lon = signed24(&meta[6..9]) / 8_388_607.0 * 180.0;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    let altitude_m = (validity & 0x4 != 0)
        .then(|| f64::from((u16::from(meta[9]) << 8) | u16::from(meta[10])) * 0.5 - 500.0);
    let velocity = validity & 0x2 != 0;
    let bearing_deg = velocity
        .then(|| (u16::from(meta[1] & 1) << 8) | u16::from(meta[2]))
        .filter(|b| *b < 360);
    let speed_kmh =
        velocity.then(|| f64::from((u16::from(meta[11]) << 4) | u16::from(meta[12] >> 4)) * 0.5);
    let radius_m = (validity & 0x1 != 0).then(|| 1u32 << ((meta[1] >> 1) & 0x7));
    let source = match meta[0] >> 4 {
        0 => "m17-client",
        1 => "openrtx",
        _ => "other",
    };
    let station = match meta[0] & 0xF {
        0 => "fixed",
        1 => "mobile",
        2 => "handheld",
        _ => "other",
    };
    Some(Position {
        lat,
        lon,
        altitude_m,
        speed_kmh,
        bearing_deg,
        radius_m,
        source,
        station,
    })
}

/// What a line of `m17-demod`'s stderr means.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// A link setup frame (a call starts or is picked up).
    Lsf(Box<Lsf>),
    /// The end-of-stream marker of the transmitting station.
    EndOfStream,
}

/// Interpret one stderr line; `None` for everything else (LICH progress, packet output, noise).
#[must_use]
pub fn parse_line(line: &str) -> Option<Line> {
    let line = line.trim();
    if line == "EOS" {
        return Some(Line::EndOfStream);
    }
    parse_lsf(line).map(|lsf| Line::Lsf(Box::new(lsf)))
}

fn parse_lsf(line: &str) -> Option<Lsf> {
    let rest = line.strip_prefix("SRC: ")?;
    let (src, rest) = rest.split_once(", DEST: ")?;
    let mut fields = rest.split(", ");
    let dst = fields.next()?;
    let kind = fields.next()?;
    let (mode, can_text) = kind.split_once(" CAN:")?;
    let (class, detail) = mode.split_once(':')?;
    let stream = class == "STR";
    let can: u8 = can_text.trim().parse().ok().filter(|c| *c <= 15)?;
    let mut type_word = None;
    let mut meta = None;
    for field in fields {
        if let Some(hex) = field.strip_prefix("TYPE: 0x") {
            type_word = u16::from_str_radix(hex.trim(), 16).ok();
        } else if let Some(hex) = field.strip_prefix("NONCE: ") {
            meta = parse_meta(hex.trim());
        }
    }
    let callsign_ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 12
            && s.chars()
                .all(|c| c.is_ascii_graphic() && !matches!(c, ',' | '"' | '\\'))
    };
    if !callsign_ok(src) || !callsign_ok(dst) {
        return None;
    }
    let (data_type, encryption, subtype) = match type_word {
        Some(word) => {
            let encryption = match (word >> 3) & 3 {
                0 => Encryption::None,
                1 => Encryption::Scrambler,
                2 => Encryption::Aes,
                _ => Encryption::Reserved,
            };
            (
                data_type_of((word >> 1) & 3),
                Some(encryption),
                Some(u8::try_from((word >> 5) & 3).unwrap_or(0)),
            )
        }
        None => (
            match detail {
                "D/D" => DataType::Data,
                "V/V" => DataType::Voice,
                "V/D" => DataType::VoiceData,
                _ => DataType::Unknown,
            },
            None,
            None,
        ),
    };
    Some(Lsf {
        src: src.to_owned(),
        dst: dst.to_owned(),
        stream,
        data_type,
        can,
        encryption,
        subtype,
        meta,
    })
}

fn data_type_of(bits: u16) -> DataType {
    match bits {
        1 => DataType::Data,
        2 => DataType::Voice,
        3 => DataType::VoiceData,
        _ => DataType::Unknown,
    }
}

fn parse_meta(hex: &str) -> Option<[u8; 14]> {
    if hex.len() != 28 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 14];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &str = "SRC: DL1ABC, DEST: DB0XYZ, STR:V/V CAN:03, TYPE: 0x0185, NONCE: 0000000000000000000000000000, CRC: 167a";

    fn lsf(line: &str) -> Lsf {
        match parse_line(line) {
            Some(Line::Lsf(lsf)) => *lsf,
            other => panic!("not an LSF: {other:?}"),
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The line the patched tool prints for a stream with `word` as TYPE and `meta` as META.
    fn line_for(word: u16, meta: &[u8; 14]) -> String {
        format!(
            "SRC: DL1ABC, DEST: ALL, STR:V/V CAN:{:02}, TYPE: 0x{word:04x}, NONCE: {}, CRC: 0000",
            (word >> 7) & 0xF,
            hex(meta)
        )
    }

    #[test]
    fn parses_the_patched_line() {
        let l = lsf(PLAIN);
        assert_eq!((l.src.as_str(), l.dst.as_str()), ("DL1ABC", "DB0XYZ"));
        assert!(l.stream);
        assert_eq!(l.data_type, DataType::Voice);
        assert_eq!(l.can, 3);
        assert_eq!(l.encryption, Some(Encryption::None));
        assert_eq!(l.encrypted(), Some(false));
        assert_eq!(l.meta, Some([0; 14]));
    }

    #[test]
    fn parses_the_upstream_line_without_the_type_word() {
        let l = lsf(
            "SRC: AB1CDE, DEST: BROADCAST, STR:V/V CAN:10, NONCE: 0000000000000000000000000000, CRC: a1f1",
        );
        assert_eq!(l.dst, "BROADCAST");
        assert_eq!(l.can, 10);
        assert_eq!(l.data_type, DataType::Voice);
        assert_eq!(l.encryption, None);
        assert_eq!(l.encrypted(), None);
        assert!(l.position().is_none());
    }

    #[test]
    fn tolerates_carriage_returns_and_other_output() {
        assert!(matches!(parse_line("\r\nEOS\r\n"), Some(Line::EndOfStream)));
        assert!(parse_line("   ").is_none());
        assert!(parse_line("LICH").is_none());
        assert!(parse_line("SRC: A, DEST: B").is_none());
        assert!(parse_line("SRC: A, DEST: B, STR:V/V CAN:99, CRC: 0").is_none());
    }

    #[test]
    fn encryption_bits_are_read_from_the_type_word() {
        // Stream, voice, AES (type 10), subtype 00, CAN 5.
        let word: u16 = 1 | (2 << 1) | (2 << 3) | (5 << 7);
        let l = lsf(&line_for(word, &[7; 14]));
        assert_eq!(l.encryption, Some(Encryption::Aes));
        assert_eq!(l.encrypted(), Some(true));
        assert_eq!(l.can, 5);
        assert!(l.position().is_none(), "no position from encrypted META");
        assert!(l.text().is_none());
        assert_eq!(l.describe()["encryption"], "aes");
    }

    #[test]
    fn packet_mode_is_not_a_stream() {
        let l = lsf(
            "SRC: DL1ABC, DEST: DB0XYZ, PKT:RAW CAN:01, NONCE: 0000000000000000000000000000, CRC: 0000",
        );
        assert!(!l.stream);
    }

    fn gnss_meta() -> [u8; 14] {
        // Data source 1 (OpenRTX), station type 2 (handheld); validity 1111, radius 3 (8 m),
        // bearing 90; latitude 48.0 deg; longitude -9.0 deg; altitude 600.0 m; speed 12.5 km/h.
        let lat = (48.0 / 90.0 * 8_388_607.0_f64).round() as i32;
        let lon = (-9.0 / 180.0 * 8_388_607.0_f64).round() as i32;
        let alt = ((600.0 + 500.0) / 0.5) as u16;
        let speed = (12.5 / 0.5) as u16;
        let mut m = [0u8; 14];
        m[0] = 0x12;
        m[1] = 0xF0 | (3 << 1);
        m[2] = 90;
        m[3..6].copy_from_slice(&lat.to_be_bytes()[1..]);
        m[6..9].copy_from_slice(&lon.to_be_bytes()[1..]);
        m[9..11].copy_from_slice(&alt.to_be_bytes());
        m[11] = (speed >> 4) as u8;
        m[12] = ((speed & 0xF) << 4) as u8;
        m
    }

    #[test]
    fn gnss_meta_becomes_a_position() {
        // Stream, voice, no encryption, subtype 01 (GNSS).
        let word: u16 = 1 | (2 << 1) | (1 << 5);
        let l = lsf(&line_for(word, &gnss_meta()));
        let p = l.position().expect("position");
        assert!(
            (p.lat - 48.0).abs() < 1e-4 && (p.lon + 9.0).abs() < 1e-4,
            "{p:?}"
        );
        assert_eq!(p.altitude_m, Some(600.0));
        assert_eq!(p.speed_kmh, Some(12.5));
        assert_eq!(p.bearing_deg, Some(90));
        assert_eq!(p.radius_m, Some(8));
        assert_eq!((p.source, p.station), ("openrtx", "handheld"));
        let described = l.describe();
        assert_eq!(described["position"]["lat"], 48.0);
        assert_eq!(described["position"]["station"], "handheld");
    }

    #[test]
    fn southern_and_western_coordinates_are_negative() {
        let mut m = gnss_meta();
        let lat = (-33.5 / 90.0 * 8_388_607.0_f64).round() as i32;
        m[3..6].copy_from_slice(&lat.to_be_bytes()[1..]);
        let word: u16 = 1 | (2 << 1) | (1 << 5);
        let p = lsf(&line_for(word, &m)).position().expect("position");
        assert!((p.lat + 33.5).abs() < 1e-4, "{p:?}");
    }

    #[test]
    fn invalid_fix_gives_no_position() {
        let mut meta = gnss_meta();
        meta[1] &= 0x0F; // nothing valid
        let word: u16 = 1 | (2 << 1) | (1 << 5);
        assert!(lsf(&line_for(word, &meta)).position().is_none());
    }

    #[test]
    fn one_block_text_is_decoded() {
        // Subtype 00 = text. Control byte 0x11: one block, block 1.
        let mut meta = [b' '; 14];
        meta[0] = 0x11;
        meta[1..8].copy_from_slice(b"Hi Bob!");
        let word: u16 = 1 | (2 << 1);
        assert_eq!(
            lsf(&line_for(word, &meta)).text().as_deref(),
            Some("Hi Bob!")
        );
        // One block of a longer message is not shown on its own.
        meta[0] = 0x31;
        assert!(lsf(&line_for(word, &meta)).text().is_none());
    }

    #[test]
    fn same_call_ignores_meta() {
        let a = lsf(PLAIN);
        let mut b = a.clone();
        b.meta = Some([1; 14]);
        assert!(a.same_call(&b));
        b.src = "DL2XYZ".into();
        assert!(!a.same_call(&b));
    }
}
