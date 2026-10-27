//! POCSAG / FLEX pagers through multimon-ng.
//!
//! multimon-ng reads raw `s16le` mono audio at 22 050 Hz (the NFM audio of the VFO) and prints
//! one line per page, for example
//!
//! ```text
//! POCSAG1200: Address: 1234567  Function: 3  Alpha:   Superhet POCSAG test 1
//! POCSAG512: Address: 2000001  Function: 0  Numeric: 0123456789 -
//! FLEX_NEXT|1600/2|00.000.A|0001234567|SS|5|ALN|3.0.K|Superhet FLEX test message
//! ```
//!
//! The legacy `FLEX` demodulator stamps wall-clock time into its lines and cannot do fragmented
//! messages; `FLEX_NEXT` is used instead. Its line layout changed between multimon-ng versions
//! (a timestamp field and the address field), so the parser anchors on the `baud/levels` and
//! `cycle.frame.phase` fields and on the type tag rather than on field positions.

use serde_json::{Map, Value, json};

use crate::event::{MAX_TEXT, event, truncate};

/// Baud rates multimon-ng can decode.
const POCSAG_BAUDS: [u32; 3] = [512, 1200, 2400];
/// Character sets of `multimon-ng -C`.
const CHARSETS: [&str; 6] = ["US", "FR", "DE", "DK", "SE", "SI"];

/// Instance configuration of the pager plugin (`SUPERHET_CONFIG`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagerConfig {
    /// POCSAG baud rates to decode.
    pub baud: Vec<u32>,
    /// Decode FLEX as well.
    pub flex: bool,
    /// Invert the input polarity (some receivers deliver inverted audio).
    pub invert: bool,
    /// Character set for alphanumeric POCSAG pages.
    pub charset: Option<String>,
}

impl Default for PagerConfig {
    fn default() -> Self {
        Self {
            baud: POCSAG_BAUDS.to_vec(),
            flex: true,
            invert: false,
            charset: None,
        }
    }
}

impl PagerConfig {
    /// Read the instance config: `null` or an object with the optional keys `baud` (array of
    /// 512 / 1200 / 2400), `flex` (bool), `invert` (bool) and `charset` (US, FR, DE, DK, SE, SI).
    ///
    /// # Errors
    /// A message naming the offending key.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let mut config = Self::default();
        let object = match value {
            Value::Null => return Ok(config),
            Value::Object(object) => object,
            _ => return Err("config must be an object".into()),
        };
        for (key, value) in object {
            match key.as_str() {
                "baud" => {
                    let list = value
                        .as_array()
                        .ok_or("baud must be an array of 512, 1200, 2400")?;
                    config.baud = list
                        .iter()
                        .map(|b| {
                            b.as_u64()
                                .and_then(|b| u32::try_from(b).ok())
                                .filter(|b| POCSAG_BAUDS.contains(b))
                                .ok_or("baud entries must be 512, 1200 or 2400")
                        })
                        .collect::<Result<_, _>>()?;
                }
                "flex" => config.flex = value.as_bool().ok_or("flex must be a boolean")?,
                "invert" => config.invert = value.as_bool().ok_or("invert must be a boolean")?,
                "charset" => {
                    let set = value.as_str().ok_or("charset must be a string")?;
                    if !CHARSETS.contains(&set) {
                        return Err(format!("charset must be one of {}", CHARSETS.join(", ")));
                    }
                    config.charset = Some(set.to_owned());
                }
                other => return Err(format!("unknown config key {other:?}")),
            }
        }
        if config.baud.is_empty() && !config.flex {
            return Err("no demodulator enabled".into());
        }
        Ok(config)
    }

    /// Arguments for multimon-ng (input on stdin).
    #[must_use]
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = ["-q", "-t", "raw"].map(String::from).into();
        for baud in &self.baud {
            args.extend(["-a".to_owned(), format!("POCSAG{baud}")]);
        }
        if self.flex {
            args.extend(["-a".to_owned(), "FLEX_NEXT".to_owned()]);
        }
        if self.invert {
            args.push("-i".into());
        }
        if let Some(set) = &self.charset {
            args.extend(["-C".to_owned(), set.clone()]);
        }
        args.push("-".into());
        args
    }
}

/// Map one output line of multimon-ng to an event (`pocsag.message` or `flex.message`).
#[must_use]
pub fn parse_line(line: &str) -> Option<Value> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.starts_with("POCSAG") {
        parse_pocsag(line).map(|data| event("pocsag.message", data))
    } else if line.starts_with("FLEX_NEXT|") {
        parse_flex(line).map(|data| event("flex.message", data))
    } else {
        None
    }
}

fn split_token(text: &str) -> (&str, &str) {
    text.split_once(char::is_whitespace).unwrap_or((text, ""))
}

/// Page text without the padding multimon-ng prints for the end of a message.
fn clean_message(text: &str) -> String {
    let mut text = text.trim();
    loop {
        let before = text.len();
        for pad in ["<EOT>", "<NUL>"] {
            text = text.strip_suffix(pad).unwrap_or(text).trim_end();
        }
        if text.len() == before {
            break;
        }
    }
    truncate(text, MAX_TEXT)
}

fn parse_pocsag(line: &str) -> Option<Map<String, Value>> {
    let rest = line.strip_prefix("POCSAG")?;
    let (baud, rest) = rest.split_once(':')?;
    let baud: u32 = baud.parse().ok()?;
    let rest = rest.trim_start().strip_prefix("Address:")?.trim_start();
    let (address, rest) = split_token(rest);
    let rest = rest.trim_start().strip_prefix("Function:")?.trim_start();
    let (function, rest) = split_token(rest);
    let (label, text) = rest.trim_start().split_once(':')?;
    let kind = match label {
        "Alpha" => "alpha",
        "Numeric" => "numeric",
        "Skyper" => "skyper",
        _ => return None,
    };
    // "-" is printed for message words that continue a page whose address was not received.
    let address = address.parse::<u64>().ok();
    let function = function.parse::<u8>().ok();
    let message = clean_message(text);
    let summary = format!(
        "{}: {message}",
        address.map_or_else(|| "-".to_owned(), |a| a.to_string())
    );
    let mut data = Map::new();
    data.insert("protocol".into(), json!("pocsag"));
    data.insert("baud".into(), json!(baud));
    data.insert("address".into(), json!(address));
    data.insert("function".into(), json!(function));
    data.insert("kind".into(), json!(kind));
    data.insert("message".into(), json!(message));
    data.insert("summary".into(), json!(summary));
    Some(data)
}

/// `1600/2` -> (1600, 2)
fn baud_levels(field: &str) -> Option<(u32, u32)> {
    let (baud, levels) = field.split_once('/')?;
    Some((baud.parse().ok()?, levels.parse().ok()?))
}

/// `00.000.A` -> (0, 0, 'A')
fn cycle_frame_phase(field: &str) -> Option<(u32, u32, char)> {
    let mut parts = field.split('.');
    let cycle = parts.next()?.parse().ok()?;
    let frame = parts.next()?.parse().ok()?;
    let phase = parts.next()?;
    let mut chars = phase.chars();
    let letter = chars.next().filter(|c| ('A'..='D').contains(c))?;
    if chars.next().is_some() || parts.next().is_some() {
        return None;
    }
    Some((cycle, frame, letter))
}

fn parse_flex(line: &str) -> Option<Map<String, Value>> {
    let fields: Vec<&str> = line.split('|').collect();
    // Anchor: a `baud/levels` field directly followed by `cycle.frame.phase`.
    let at = (1..fields.len().saturating_sub(1)).find(|&i| {
        baud_levels(fields[i]).is_some() && cycle_frame_phase(fields[i + 1]).is_some()
    })?;
    let (baud, levels) = baud_levels(fields[at])?;
    let (cycle, frame, phase) = cycle_frame_phase(fields[at + 1])?;
    let capcodes: Vec<u64> = fields
        .get(at + 2)?
        .split_whitespace()
        .filter_map(|c| c.trim_matches(['[', ']']).parse().ok())
        .collect();
    let capcode = *capcodes.first()?;
    // The type tag follows after the address-class field(s).
    let tag_at =
        (at + 3..fields.len()).find(|&i| matches!(fields[i], "ALN" | "NUM" | "TON" | "BIN"))?;
    let kind = match fields[tag_at] {
        "ALN" => "alpha",
        "NUM" => "numeric",
        "TON" => "tone",
        _ => "binary",
    };
    // fields[tag_at + 1] are the fragment / flag markers; the page text is the rest (it may
    // contain '|' itself).
    let message = fields
        .get(tag_at + 2..)
        .map_or_else(String::new, |m| m.join("|"));
    let message = clean_message(&message);
    let summary = format!("{capcode}: {message}");
    let mut data = Map::new();
    data.insert("protocol".into(), json!("flex"));
    data.insert("baud".into(), json!(baud));
    data.insert("levels".into(), json!(levels));
    data.insert("cycle".into(), json!(cycle));
    data.insert("frame".into(), json!(frame));
    data.insert("phase".into(), json!(phase.to_string()));
    data.insert("capcode".into(), json!(capcode));
    if capcodes.len() > 1 {
        data.insert("capcodes".into(), json!(capcodes));
    }
    data.insert("kind".into(), json!(kind));
    data.insert("message".into(), json!(message));
    data.insert("summary".into(), json!(summary));
    Some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(line: &str) -> Value {
        parse_line(line).expect("parsed")["data"].clone()
    }

    #[test]
    fn pocsag_alpha() {
        let d = data("POCSAG1200: Address: 1234567  Function: 3  Alpha:   Superhet POCSAG test 1");
        assert_eq!(d["baud"], 1200);
        assert_eq!(d["address"], 1234567);
        assert_eq!(d["function"], 3);
        assert_eq!(d["kind"], "alpha");
        assert_eq!(d["message"], "Superhet POCSAG test 1");
        assert_eq!(d["summary"], "1234567: Superhet POCSAG test 1");
    }

    #[test]
    fn pocsag_numeric_trailing_padding_and_eot() {
        let d = data("POCSAG512: Address: 2000001  Function: 0  Numeric: 0123456789 -   ");
        assert_eq!(d["kind"], "numeric");
        assert_eq!(d["message"], "0123456789 -");
        let d = data("POCSAG2400: Address:    1000  Function: 3  Alpha:   Fire alarm<EOT><EOT>");
        assert_eq!(d["address"], 1000);
        assert_eq!(d["message"], "Fire alarm");
    }

    #[test]
    fn pocsag_without_address_is_null() {
        let d = data("POCSAG1200: Address:       -  Function: -  Alpha:   rest of a page");
        assert!(d["address"].is_null());
        assert!(d["function"].is_null());
        assert_eq!(d["summary"], "-: rest of a page");
    }

    #[test]
    fn pocsag_ignores_noise() {
        assert!(parse_line("multimon-ng 1.2.0").is_none());
        assert!(parse_line("Enabled demodulators: POCSAG512").is_none());
        assert!(parse_line("POCSAG1200: Address: 12  Function: 3").is_none());
        assert!(parse_line("POCSAGx: Address: 12  Function: 3  Alpha: a").is_none());
    }

    #[test]
    fn flex_next_layouts() {
        // multimon-ng 1.2.0
        let d = data("FLEX_NEXT|1600/2|00.000.A|0001234567|SS|5|ALN|3.0.K|Superhet FLEX test");
        assert_eq!(d["protocol"], "flex");
        assert_eq!(d["baud"], 1600);
        assert_eq!(d["levels"], 2);
        assert_eq!(d["phase"], "A");
        assert_eq!(d["capcode"], 1234567);
        assert_eq!(d["kind"], "alpha");
        assert_eq!(d["message"], "Superhet FLEX test");
        // newer layout: timestamp first, one address class field
        let d = data(
            "FLEX_NEXT|2026-10-27 12:15:05+00:00|3200/4|12.101.C|0000000042|S|NUM|K.0/3.N0|123 456",
        );
        assert_eq!(d["baud"], 3200);
        assert_eq!(d["levels"], 4);
        assert_eq!(d["cycle"], 12);
        assert_eq!(d["frame"], 101);
        assert_eq!(d["phase"], "C");
        assert_eq!(d["capcode"], 42);
        assert_eq!(d["kind"], "numeric");
        assert_eq!(d["message"], "123 456");
    }

    #[test]
    fn flex_message_may_contain_pipes_and_group_capcodes() {
        let d = data("FLEX_NEXT|1600/2|00.001.B|2029568 1000001 1000002|G|ALN|K.0/3.N1.GRP|a|b");
        assert_eq!(d["capcode"], 2029568);
        assert_eq!(d["capcodes"], json!([2029568, 1000001, 1000002]));
        assert_eq!(d["message"], "a|b");
    }

    #[test]
    fn flex_next_ignores_other_lines() {
        assert!(parse_line("FLEX_NEXT|1600/2|00.000.A|0000000000|B|FLEXTIME|x|y").is_none());
        assert!(parse_line("FLEX_NEXT|garbage").is_none());
    }

    #[test]
    fn config_defaults_and_args() {
        let config = PagerConfig::from_json(&Value::Null).expect("default");
        assert_eq!(
            config.args().join(" "),
            "-q -t raw -a POCSAG512 -a POCSAG1200 -a POCSAG2400 -a FLEX_NEXT -"
        );
        let config = PagerConfig::from_json(
            &json!({"baud": [1200], "flex": false, "invert": true, "charset": "DE"}),
        )
        .expect("custom");
        assert_eq!(
            config.args().join(" "),
            "-q -t raw -a POCSAG1200 -i -C DE -"
        );
    }

    #[test]
    fn config_rejects_bad_input() {
        for bad in [
            json!({"baud": [999]}),
            json!({"baud": 1200}),
            json!({"charset": "XX"}),
            json!({"colour": 1}),
            json!({"baud": [], "flex": false}),
            json!(7),
        ] {
            assert!(PagerConfig::from_json(&bad).is_err(), "{bad}");
        }
    }
}
