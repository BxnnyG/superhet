//! Message templates with a deliberately tiny syntax: literal text and `{{path}}` placeholders.
//!
//! There are no expressions, filters, loops or includes, so a template cannot do anything but
//! look up values of the event. A path is dotted (`data.text`, `data.address`, `value`,
//! `device`); unknown paths render as nothing. Every substituted value is shortened to
//! [`VALUE_CHARS`] characters, stripped of control characters and escaped for the place it lands
//! in ([`Escape`]), and the whole result is capped, so an event cannot smuggle a header line or
//! break out of a JSON string.

use serde_json::Value;

/// Longest text one placeholder contributes, in characters.
pub const VALUE_CHARS: usize = 500;
/// Most placeholders in one template.
pub const MAX_PLACEHOLDERS: usize = 40;
/// Longest placeholder path.
const MAX_PATH: usize = 64;

/// How substituted values are made safe for their destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escape {
    /// Plain text (titles, bodies): control characters other than line breaks are dropped.
    Text,
    /// Inside a JSON string literal of a request body.
    Json,
    /// An HTTP header value: no control characters at all, so no new header lines.
    Header,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Lit(String),
    Var(String),
}

/// A parsed template.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Template {
    parts: Vec<Part>,
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

impl Template {
    /// Parse `text`.
    ///
    /// # Errors
    /// A message for an unclosed `{{`, a malformed placeholder name or too many placeholders.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut rest = text;
        let mut vars = 0;
        while let Some(open) = rest.find("{{") {
            if open > 0 {
                parts.push(Part::Lit(rest[..open].to_owned()));
            }
            let after = &rest[open + 2..];
            let close = after
                .find("}}")
                .ok_or_else(|| "a placeholder is not closed with }}".to_owned())?;
            let path = after[..close].trim();
            if !valid_path(path) {
                return Err(format!(
                    "placeholder {{{{{}}}}} is not a plain name like data.text",
                    after[..close].chars().take(40).collect::<String>()
                ));
            }
            vars += 1;
            if vars > MAX_PLACEHOLDERS {
                return Err(format!(
                    "at most {MAX_PLACEHOLDERS} placeholders per template"
                ));
            }
            parts.push(Part::Var(path.to_owned()));
            rest = &after[close + 2..];
        }
        if !rest.is_empty() {
            parts.push(Part::Lit(rest.to_owned()));
        }
        Ok(Self { parts })
    }

    /// The placeholder names, in order of appearance.
    #[must_use]
    pub fn variables(&self) -> Vec<&str> {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Var(v) => Some(v.as_str()),
                Part::Lit(_) => None,
            })
            .collect()
    }

    /// Fill the template from `ctx` (a JSON object), capped at `max_chars`.
    #[must_use]
    pub fn render(&self, ctx: &Value, escape: Escape, max_chars: usize) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Lit(text) => out.push_str(text),
                Part::Var(path) => out.push_str(&escape_value(&text_of(lookup(ctx, path)), escape)),
            }
            if out.chars().count() > max_chars {
                break;
            }
        }
        if escape != Escape::Json {
            out = cap(&out, max_chars);
        } else if out.chars().count() > max_chars {
            // Cutting JSON text could split an escape sequence; keep it whole instead.
            out = cap_on_escape(&out, max_chars);
        }
        out
    }
}

/// The value at the dotted `path`; numeric segments index arrays.
#[must_use]
pub fn lookup<'a>(ctx: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = ctx;
    for seg in path.split('.') {
        cur = match cur {
            Value::Object(map) => map.get(seg)?,
            Value::Array(items) => items.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// The text form of a value: strings as they are, scalars printed, containers as compact JSON,
/// absent / null as nothing.
#[must_use]
pub fn text_of(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

fn cap(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_owned()
    } else {
        let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn cap_on_escape(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    // Do not end in the middle of a `\` escape or a `\uXXXX`.
    while let Some(pos) = out.rfind('\\') {
        let tail = out.len() - pos;
        let complete = match out.as_bytes().get(pos + 1) {
            Some(b'u') => tail >= 6,
            Some(_) => tail >= 2,
            None => false,
        };
        // An escaped backslash (`\\`) leaves an even number of trailing backslashes.
        let slashes = out.bytes().rev().take_while(|b| *b == b'\\').count();
        if complete || slashes % 2 == 0 {
            break;
        }
        out.truncate(pos);
    }
    out
}

fn escape_value(text: &str, escape: Escape) -> String {
    let short = cap(text, VALUE_CHARS);
    match escape {
        Escape::Text => short
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect(),
        Escape::Header => short
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect(),
        Escape::Json => {
            let mut out = String::with_capacity(short.len());
            for c in short.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c.is_control() => {
                        use std::fmt::Write as _;
                        let _ = write!(out, "\\u{:04x}", u32::from(c));
                    }
                    c => out.push(c),
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> Value {
        json!({
            "value": "FIRE at main street",
            "device": "rtl0",
            "data": {"address": 1234, "text": "line1\nline2\u{7}", "nested": {"a": [1, 2]}},
            "nothing": null,
        })
    }

    #[test]
    fn placeholders_fill_from_the_event() {
        let t = Template::parse("{{ value }} / RIC {{data.address}} on {{device}}").expect("ok");
        assert_eq!(
            t.render(&ctx(), Escape::Text, 200),
            "FIRE at main street / RIC 1234 on rtl0"
        );
        assert_eq!(t.variables(), ["value", "data.address", "device"]);
    }

    #[test]
    fn unknown_and_null_paths_are_empty_and_containers_are_json() {
        let t = Template::parse("[{{nope}}][{{nothing}}][{{data.nested}}][{{data.nested.a.1}}]")
            .expect("ok");
        assert_eq!(
            t.render(&ctx(), Escape::Text, 200),
            r#"[][][{"a":[1,2]}][2]"#
        );
    }

    #[test]
    fn malformed_templates_are_rejected() {
        assert!(Template::parse("{{value").is_err());
        assert!(Template::parse("{{ va lue }}").is_err());
        assert!(Template::parse("{{}}").is_err());
        assert!(Template::parse("{{a;b}}").is_err());
        let many = "{{x}}".repeat(MAX_PLACEHOLDERS + 1);
        assert!(Template::parse(&many).is_err());
        // There is no expression syntax: this is just a (rejected) odd name, not code.
        assert!(Template::parse("{{ value | upper }}").is_err());
    }

    #[test]
    fn text_keeps_line_breaks_but_drops_other_control_characters() {
        let t = Template::parse("{{data.text}}").expect("ok");
        assert_eq!(t.render(&ctx(), Escape::Text, 200), "line1\nline2");
    }

    #[test]
    fn header_values_cannot_start_a_new_header_line() {
        let evil = json!({"value": "x\r\nBcc: attacker@example.com"});
        let t = Template::parse("Bearer {{value}}").expect("ok");
        let out = t.render(&evil, Escape::Header, 200);
        assert!(!out.contains('\r') && !out.contains('\n'), "{out:?}");
    }

    #[test]
    fn json_escaping_keeps_the_document_valid() {
        let evil = json!({"value": "he said \"hi\"\\ \n\u{1}</script>"});
        let t = Template::parse(r#"{"text": "{{value}}", "n": 1}"#).expect("ok");
        let out = t.render(&evil, Escape::Json, 1000);
        let parsed: Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed["text"], "he said \"hi\"\\ \n\u{1}</script>");
    }

    #[test]
    fn values_and_results_are_capped() {
        let long = json!({"value": "ä".repeat(5000)});
        let t = Template::parse("{{value}}").expect("ok");
        let out = t.render(&long, Escape::Text, 10_000);
        assert_eq!(out.chars().count(), VALUE_CHARS);
        assert!(out.ends_with('…'));
        let t = Template::parse("abc {{value}}").expect("ok");
        assert_eq!(t.render(&long, Escape::Text, 20).chars().count(), 20);
    }

    #[test]
    fn capped_json_never_ends_in_a_broken_escape() {
        let quotes = json!({"value": "\"".repeat(400)});
        let t = Template::parse("{{value}}").expect("ok");
        let out = t.render(&quotes, Escape::Json, 101);
        let wrapped = format!("\"{out}\"");
        assert!(serde_json::from_str::<Value>(&wrapped).is_ok(), "{out}");
    }
}
