//! What `dsd-fme` prints on stderr, as far as the call tracker needs it.
//!
//! `dsd-fme` has no machine-readable output: it writes a human-oriented, ANSI-coloured status log.
//! This parser therefore looks for keywords and `KEY=value` / `Key: value` pairs instead of
//! matching one fixed layout, so small differences between releases do not break it:
//!
//! ```text
//! Sync: +DMR  BS VOICE        | Color Code=01 | VC1
//!  Slot 1 TGT=91 SRC=3101234 FLCO=0x00 FID=0x00 SVC=0x00 Group Voice
//! Sync: +NXDN96 ... RAN=01
//!  NXDN VCALL Group Source: 20017 Target: 100 RAN: 1
//!  Slot 1 TLC
//! ```
//!
//! Everything else (data bursts, other protocols, banners) is ignored. The layouts the tests use
//! are transcripts written by hand in this style, not captures of a particular release; see
//! `docs/plugins-dsd.md`, "Not verified".

use serde_json::Value;

/// The digital voice protocols this plugin follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Dmr,
    Dpmr,
    Nxdn,
}

impl Protocol {
    /// Name used in events and the configuration.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Dmr => "dmr",
            Self::Dpmr => "dpmr",
            Self::Nxdn => "nxdn",
        }
    }
}

/// A `Sync:` line: the decoder locked onto a frame of this protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sync {
    pub protocol: Protocol,
    /// The burst carries voice (as opposed to data or control).
    pub voice: bool,
    /// DMR / dPMR colour code.
    pub cc: Option<u8>,
    /// NXDN radio access number.
    pub ran: Option<u8>,
}

/// What a link-control or call line says about a call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallInfo {
    pub protocol: Option<Protocol>,
    /// DMR time slot (1 or 2).
    pub slot: Option<u8>,
    pub src: Option<u32>,
    /// Target: a talkgroup for group calls, a radio ID for private calls.
    pub dst: Option<u32>,
    pub group: Option<bool>,
    /// Some(true/false) when the line says whether the call is encrypted.
    pub encrypted: Option<bool>,
    /// Algorithm identifier as printed (`0x21`, `AES256`).
    pub algorithm: Option<String>,
    pub cc: Option<u8>,
    pub ran: Option<u8>,
}

/// One interesting line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Sync(Sync),
    Call(CallInfo),
    /// A terminator: the transmission ended (DMR terminator LC, NXDN TX release, ...).
    End {
        slot: Option<u8>,
    },
}

/// Remove ANSI escape sequences (`dsd-fme` colours its log) and surrounding blanks.
#[must_use]
pub fn clean(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // Parameters and intermediates, then one final byte in @..~.
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out.trim().to_owned()
}

/// Parse one line of `dsd-fme`'s log; `None` for everything the plugin does not care about.
#[must_use]
pub fn parse_line(raw: &str) -> Option<Line> {
    let line = clean(raw);
    if line.is_empty() {
        return None;
    }
    let lower = line.to_ascii_lowercase();
    if let Some(at) = lower.find("sync:") {
        return parse_sync(&lower[at + 5..]).map(Line::Sync);
    }
    let slot = slot_of(&lower);
    if is_terminator(&lower) {
        return Some(Line::End { slot });
    }
    parse_call(&line, &lower, slot).map(Line::Call)
}

fn parse_sync(lower: &str) -> Option<Sync> {
    let protocol = protocol_of(lower)?;
    let voice = lower.contains("voice") && !lower.contains("data");
    Some(Sync {
        protocol,
        voice,
        cc: small(lower, &["color code", "colour code", "cc"], 15),
        ran: small(lower, &["ran"], 63),
    })
}

/// The protocol a line talks about, from the first protocol name in it.
fn protocol_of(lower: &str) -> Option<Protocol> {
    [
        ("dmr", Protocol::Dmr),
        ("dpmr", Protocol::Dpmr),
        ("nxdn", Protocol::Nxdn),
    ]
    .into_iter()
    .filter_map(|(name, protocol)| word_at(lower, name).map(|at| (at, protocol)))
    .min_by_key(|(at, _)| *at)
    .map(|(_, protocol)| protocol)
}

/// Position of `name` as the start of a word (`nxdn96` counts as `nxdn`; `dmrla` does not count
/// as `dmr`).
fn word_at(lower: &str, name: &str) -> Option<usize> {
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let at = from + found;
        let before_ok = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        let end = at + name.len();
        let after = bytes.get(end).copied();
        // "nxdn48", "nxdn96" and "dmr" followed by a blank, "|" or the end are fine; "dmrla" and
        // "dmrx" are other words.
        let after_ok = match after {
            None => true,
            Some(b) => !b.is_ascii_alphabetic(),
        };
        if before_ok && after_ok {
            return Some(at);
        }
        from = at + name.len();
    }
    None
}

fn is_terminator(lower: &str) -> bool {
    word_at(lower, "tlc").is_some()
        || lower.contains("terminator")
        || lower.contains("tx_rel")
        || lower.contains("tx release")
        || lower.contains("call end")
        || lower.contains("end of call")
        || lower.contains("end of transmission")
}

fn slot_of(lower: &str) -> Option<u8> {
    let at = word_at(lower, "slot")?;
    let rest = lower[at + 4..].trim_start_matches([' ', ':', '=']);
    match rest.as_bytes().first() {
        Some(b'1') => Some(1),
        Some(b'2') => Some(2),
        _ => None,
    }
}

fn parse_call(line: &str, lower: &str, slot: Option<u8>) -> Option<CallInfo> {
    let src = number(lower, &["src", "source"]);
    let dst = number(
        lower,
        &["tgt", "target", "tg", "dst", "dest", "destination"],
    );
    let flco = hex(lower, &["flco"]);
    let group = if word_at(lower, "group").is_some() {
        Some(true)
    } else if word_at(lower, "private").is_some()
        || word_at(lower, "individual").is_some()
        || lower.contains("unit-to-unit")
    {
        Some(false)
    } else {
        // Link control opcodes 0 (group voice) and 3 (unit to unit voice).
        flco.and_then(|f| match f {
            0x00 => Some(true),
            0x03 => Some(false),
            _ => None,
        })
    };
    let (encrypted, algorithm) = encryption(line, lower);
    if src.is_none() && dst.is_none() && encrypted.is_none() {
        return None;
    }
    // Encryption-only lines are only meaningful in the context of a call (a slot or an
    // algorithm), not any line that happens to say "encrypted".
    if src.is_none() && dst.is_none() && slot.is_none() && algorithm.is_none() {
        return None;
    }
    Some(CallInfo {
        protocol: protocol_of(lower),
        slot,
        src,
        dst,
        group,
        encrypted,
        algorithm,
        cc: small(lower, &["color code", "colour code", "cc"], 15),
        ran: small(lower, &["ran"], 63),
    })
}

/// Whether the line says the call is encrypted, and with which algorithm.
fn encryption(line: &str, lower: &str) -> (Option<bool>, Option<String>) {
    let mut algorithm = None;
    let mut encrypted = None;
    if let Some(at) = word_at(lower, "alg") {
        // "ALG ID: 0x21", "ALG: AES256", "ALG=0x00"
        let rest = lower[at + 3..].trim_start();
        let rest = rest.strip_prefix("id").map_or(rest, str::trim_start);
        let rest = rest.trim_start_matches([':', '=', ' ']);
        let token: String = line[line.len() - rest.len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if !token.is_empty() {
            let digits = token
                .strip_prefix("0x")
                .or_else(|| token.strip_prefix("0X"));
            let clear = match digits {
                Some(hex) => u32::from_str_radix(hex, 16).is_ok_and(|v| v == 0x00 || v == 0x80),
                None => matches!(
                    token.to_ascii_lowercase().as_str(),
                    "none" | "clear" | "off"
                ),
            };
            encrypted = Some(!clear);
            algorithm = Some(token);
        }
    }
    if let Some(svc) = hex(lower, &["svc"]) {
        // DMR service options, bit 6: privacy.
        if svc & 0x40 != 0 {
            encrypted = Some(true);
        } else if encrypted.is_none() {
            encrypted = Some(false);
        }
    }
    if lower.contains("unencrypted") || lower.contains("not encrypted") {
        encrypted = Some(false);
    } else if word_at(lower, "encrypted").is_some() || word_at(lower, "enc").is_some() {
        encrypted = Some(true);
    }
    (encrypted, algorithm)
}

/// The value after `key` (`key=12`, `key: 12`, `key 12`) for the first of `keys` found.
fn value_after<'a>(lower: &'a str, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        let bytes = lower.as_bytes();
        let mut from = 0;
        while let Some(found) = lower[from..].find(key) {
            let at = from + found;
            from = at + key.len();
            if at > 0 && bytes[at - 1].is_ascii_alphanumeric() {
                continue;
            }
            let rest = &lower[at + key.len()..];
            // The key must be followed by a separator: "tg" must not match "tgt", "cc" must not
            // match "ccx".
            match rest.as_bytes().first() {
                Some(b'=' | b':' | b' ') => {}
                _ => continue,
            }
            let value = rest.trim_start_matches([' ', ':', '=']);
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn number(lower: &str, keys: &[&str]) -> Option<u32> {
    let digits: String = value_after(lower, keys)?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn small(lower: &str, keys: &[&str], max: u32) -> Option<u8> {
    number(lower, keys)
        .filter(|v| *v <= max)
        .and_then(|v| u8::try_from(v).ok())
}

fn hex(lower: &str, keys: &[&str]) -> Option<u32> {
    let value = value_after(lower, keys)?;
    let value = value.strip_prefix("0x").unwrap_or(value);
    let digits: String = value.chars().take_while(char::is_ascii_hexdigit).collect();
    u32::from_str_radix(&digits, 16).ok()
}

/// A `Line` as JSON, for tests and the golden transcript files.
#[must_use]
pub fn describe(line: &Line) -> Value {
    match line {
        Line::Sync(s) => serde_json::json!({
            "line": "sync", "protocol": s.protocol.name(), "voice": s.voice, "cc": s.cc, "ran": s.ran,
        }),
        Line::Call(c) => serde_json::json!({
            "line": "call", "protocol": c.protocol.map(Protocol::name), "slot": c.slot,
            "src": c.src, "dst": c.dst, "group": c.group, "encrypted": c.encrypted,
            "algorithm": c.algorithm, "cc": c.cc, "ran": c.ran,
        }),
        Line::End { slot } => serde_json::json!({ "line": "end", "slot": slot }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(line: &str) -> CallInfo {
        match parse_line(line) {
            Some(Line::Call(c)) => c,
            other => panic!("{line:?} -> {other:?}"),
        }
    }

    #[test]
    fn colours_are_stripped() {
        assert_eq!(
            clean("\u{1b}[32m Slot 1 \u{1b}[0m TGT=9\r"),
            "Slot 1  TGT=9"
        );
    }

    #[test]
    fn dmr_sync_gives_protocol_voice_and_colour_code() {
        let line = parse_line("Sync: +DMR  BS VOICE        | Color Code=05 | VC1");
        assert_eq!(
            line,
            Some(Line::Sync(Sync {
                protocol: Protocol::Dmr,
                voice: true,
                cc: Some(5),
                ran: None
            }))
        );
        // A data burst is not voice; inverted polarity ("-") still is DMR.
        let Some(Line::Sync(data)) = parse_line("12:00:01 Sync: -DMR  MS DATA | Color Code=01")
        else {
            panic!("sync")
        };
        assert!(!data.voice);
        assert_eq!(data.protocol, Protocol::Dmr);
    }

    #[test]
    fn nxdn_and_dpmr_syncs() {
        let Some(Line::Sync(n)) = parse_line("Sync: +NXDN96  VOICE | RAN=12") else {
            panic!("sync")
        };
        assert_eq!((n.protocol, n.ran, n.cc), (Protocol::Nxdn, Some(12), None));
        let Some(Line::Sync(d)) = parse_line("Sync: +dPMR  VOICE | CC=3") else {
            panic!("sync")
        };
        assert_eq!((d.protocol, d.cc), (Protocol::Dpmr, Some(3)));
        // Other protocols are none of our business.
        assert_eq!(parse_line("Sync: +P25p1 LDU1"), None);
    }

    #[test]
    fn dmr_link_control_names_slot_source_and_talkgroup() {
        let c = call(" Slot 2 TGT=91 SRC=3101234 FLCO=0x00 FID=0x00 SVC=0x00 Group Voice");
        assert_eq!(
            (c.slot, c.src, c.dst, c.group, c.encrypted),
            (Some(2), Some(3_101_234), Some(91), Some(true), Some(false))
        );
    }

    #[test]
    fn private_calls_and_flco_without_words() {
        let c = call("Slot 1 TGT=4000001 SRC=3101234 FLCO=0x03 SVC=0x00");
        assert_eq!(c.group, Some(false));
        let c = call("Slot 1 Source: 77 Target: 8 Private");
        assert_eq!((c.src, c.dst, c.group), (Some(77), Some(8), Some(false)));
    }

    #[test]
    fn the_privacy_bit_and_algorithm_mark_encryption() {
        let c = call("Slot 1 TGT=9 SRC=5 FLCO=0x00 SVC=0x40 Group Voice");
        assert_eq!(c.encrypted, Some(true));
        let c = call("Slot 1 PI Header ALG ID: 0x21 KEY ID: 0x01");
        assert_eq!(
            (c.encrypted, c.algorithm.as_deref()),
            (Some(true), Some("0x21"))
        );
        assert_eq!((c.src, c.dst), (None, None));
        let c = call("Slot 1 PI Header ALG ID: 0x00");
        assert_eq!(c.encrypted, Some(false));
        let c = call("NXDN VCALL Source: 10 Target: 20 Encrypted");
        assert_eq!(c.encrypted, Some(true));
        let c = call("NXDN VCALL Source: 10 Target: 20 Unencrypted");
        assert_eq!(c.encrypted, Some(false));
        // A bare "Slot 1 Encrypted" updates the call of that slot.
        let c = call("Slot 1 Encrypted");
        assert_eq!((c.slot, c.encrypted, c.src), (Some(1), Some(true), None));
    }

    #[test]
    fn terminators_end_the_call_of_their_slot() {
        assert_eq!(
            parse_line("Slot 1 TLC TGT=91 SRC=3101234"),
            Some(Line::End { slot: Some(1) })
        );
        assert_eq!(
            parse_line("NXDN TX_REL Source: 10"),
            Some(Line::End { slot: None })
        );
    }

    #[test]
    fn short_keys_do_not_match_inside_longer_ones() {
        // "TG" is a key, "TGT" must not be read as "TG T..."; "cc" is not "ccx".
        let c = call("Slot 1 TGT=7 SRC=1");
        assert_eq!(c.dst, Some(7));
        assert_eq!(small("ccx=3", &["cc"], 15), None);
        assert_eq!(small("cc=3", &["cc"], 15), Some(3));
        // "DMRLA" is not "DMR".
        assert_eq!(protocol_of("dmrla"), None);
    }

    #[test]
    fn chatter_is_ignored() {
        for line in [
            "",
            "Digital Speech Decoder: Florida Man Edition",
            "Audio Out Device: -",
            "DMR PDU: 0x1234",
            "Encrypted traffic is not decoded",
        ] {
            assert_eq!(parse_line(line), None, "{line:?}");
        }
    }
}
