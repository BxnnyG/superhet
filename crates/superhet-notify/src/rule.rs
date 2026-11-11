//! Rules: which events match, and how often a rule may fire.
//!
//! An event is the JSON row of the decoder history (`id`, `ts_ms`, `device`, `vfo`, `freq_hz`,
//! `mode`, `kind`, `source`, `value`, `meta`). Conditions look at fields of its [`context`]: the
//! top-level columns plus `data`, the decoder's own payload (`data.address`, `data.text`,
//! `data.callsign`, ...).

use std::collections::{HashMap, VecDeque};

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::template::{Escape, Template, lookup, text_of};

/// Most conditions on one rule.
pub const MAX_CONDITIONS: usize = 16;
/// Longest field path.
const MAX_FIELD: usize = 128;
/// Longest comparison text (and regex pattern), in characters.
pub const MAX_VALUE_CHARS: usize = 256;
/// Most alternatives in a list value.
const MAX_ALTERNATIVES: usize = 50;
/// Compiled-program size limit of a regex in bytes.
const REGEX_SIZE_LIMIT: usize = 100_000;
/// Lazy-DFA cache limit of a regex in bytes.
const REGEX_DFA_LIMIT: usize = 1 << 20;
/// Longest text a regex or substring test looks at, in bytes. Together with the regex engine's
/// guarantee of linear time (no backtracking) this bounds the work of one test.
pub const HAYSTACK_BYTES: usize = 4096;
/// Events older than this are not notified about (a long outage must not replay stale alerts).
pub const MAX_EVENT_AGE_MS: i64 = 3_600_000;

/// Comparison of a field with a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Text equals (any of a list).
    Equals,
    /// Text does not equal.
    NotEquals,
    /// Text contains (any of a list).
    Contains,
    /// Text does not contain.
    NotContains,
    /// Regular expression found in the text.
    Regex,
    /// Number greater than.
    Gt,
    /// Number greater than or equal.
    Gte,
    /// Number less than.
    Lt,
    /// Number less than or equal.
    Lte,
    /// The field is present and not null.
    Exists,
}

/// One condition as stored and edited (`{field, op, value, case_sensitive}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionDoc {
    /// Dotted path into the event context.
    pub field: String,
    /// The comparison.
    pub op: Op,
    /// Text, number or (for `equals` / `contains`) a list of alternatives; absent for `exists`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// Compare text exactly (default: ignore case).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub case_sensitive: bool,
}

#[derive(Debug, Clone)]
enum Matcher {
    Any(Vec<String>),
    Regex(Regex),
    Number(f64),
    None,
}

/// A validated, compiled condition.
#[derive(Debug, Clone)]
pub struct Condition {
    field: String,
    op: Op,
    case_sensitive: bool,
    matcher: Matcher,
}

fn value_texts(value: &Value) -> Result<Vec<String>, String> {
    let items: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    if items.is_empty() || items.len() > MAX_ALTERNATIVES {
        return Err(format!("give 1 to {MAX_ALTERNATIVES} values"));
    }
    items
        .into_iter()
        .map(|v| match v {
            Value::String(s) if !s.is_empty() => Ok(s.clone()),
            Value::Number(n) => Ok(n.to_string()),
            Value::Bool(b) => Ok(b.to_string()),
            _ => Err("values must be non-empty text or numbers".to_owned()),
        })
        .collect()
}

fn too_long(text: &str) -> bool {
    text.chars().count() > MAX_VALUE_CHARS
}

fn number_of(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

impl Condition {
    /// Validate and compile `doc`.
    ///
    /// # Errors
    /// A message naming what is wrong (shown to the admin).
    pub fn compile(doc: &ConditionDoc) -> Result<Self, String> {
        if doc.field.is_empty() || doc.field.len() > MAX_FIELD || doc.field.split('.').count() > 8 {
            return Err("field must be a path like data.text (at most 8 parts)".to_owned());
        }
        if !doc
            .field
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            return Err(format!(
                "field {:?} may only use letters, digits, _ - and .",
                doc.field.chars().take(40).collect::<String>()
            ));
        }
        let matcher = match doc.op {
            Op::Exists => {
                if doc.value.is_some() {
                    return Err("exists takes no value".to_owned());
                }
                Matcher::None
            }
            Op::Equals | Op::NotEquals | Op::Contains | Op::NotContains => {
                let value = doc.value.as_ref().ok_or("a value is required")?;
                let mut texts = value_texts(value)?;
                if texts.iter().any(|t| too_long(t)) {
                    return Err(format!(
                        "values are limited to {MAX_VALUE_CHARS} characters"
                    ));
                }
                if !doc.case_sensitive {
                    texts = texts.iter().map(|t| t.to_lowercase()).collect();
                }
                Matcher::Any(texts)
            }
            Op::Regex => {
                let pattern = doc
                    .value
                    .as_ref()
                    .and_then(Value::as_str)
                    .filter(|p| !p.is_empty())
                    .ok_or("the pattern must be non-empty text")?;
                if too_long(pattern) {
                    return Err(format!(
                        "patterns are limited to {MAX_VALUE_CHARS} characters"
                    ));
                }
                let regex = RegexBuilder::new(pattern)
                    .case_insensitive(!doc.case_sensitive)
                    .size_limit(REGEX_SIZE_LIMIT)
                    .dfa_size_limit(REGEX_DFA_LIMIT)
                    .nest_limit(32)
                    .build()
                    .map_err(|e| match e {
                        regex::Error::CompiledTooBig(_) => {
                            "the pattern is too complex (compiled size limit)".to_owned()
                        }
                        other => format!("invalid pattern: {}", first_line(&other.to_string())),
                    })?;
                Matcher::Regex(regex)
            }
            Op::Gt | Op::Gte | Op::Lt | Op::Lte => {
                let n = doc
                    .value
                    .as_ref()
                    .and_then(number_of)
                    .ok_or("a number is required")?;
                Matcher::Number(n)
            }
        };
        Ok(Self {
            field: doc.field.clone(),
            op: doc.op,
            case_sensitive: doc.case_sensitive,
            matcher,
        })
    }

    /// Whether the event context satisfies the condition. A missing field satisfies only
    /// `not_equals`, `not_contains`.
    #[must_use]
    pub fn matches(&self, ctx: &Value) -> bool {
        let field = lookup(ctx, &self.field).filter(|v| !v.is_null());
        if self.op == Op::Exists {
            return field.is_some();
        }
        let Some(field) = field else {
            return matches!(self.op, Op::NotEquals | Op::NotContains);
        };
        match (&self.matcher, self.op) {
            (Matcher::Number(limit), op) => number_of(field).is_some_and(|n| match op {
                Op::Gt => n > *limit,
                Op::Gte => n >= *limit,
                Op::Lt => n < *limit,
                _ => n <= *limit,
            }),
            (Matcher::Regex(re), _) => re.is_match(clip(&text_of(Some(field)))),
            (Matcher::Any(alternatives), op) => {
                let text = text_of(Some(field));
                let text = clip(&text);
                let folded;
                let text = if self.case_sensitive {
                    text
                } else {
                    folded = text.to_lowercase();
                    &folded
                };
                match op {
                    Op::Equals => alternatives.iter().any(|a| a == text),
                    Op::NotEquals => alternatives.iter().all(|a| a != text),
                    Op::Contains => alternatives.iter().any(|a| text.contains(a.as_str())),
                    _ => alternatives.iter().all(|a| !text.contains(a.as_str())),
                }
            }
            (Matcher::None, _) => false,
        }
    }
}

fn first_line(text: &str) -> String {
    text.lines().last().unwrap_or("").trim().to_owned()
}

/// The first [`HAYSTACK_BYTES`] bytes of `text`, cut on a character boundary.
fn clip(text: &str) -> &str {
    if text.len() <= HAYSTACK_BYTES {
        return text;
    }
    let mut end = HAYSTACK_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Parse and compile the stored `conditions` array.
///
/// # Errors
/// A message naming the offending condition.
pub fn compile_conditions(raw: &Value) -> Result<Vec<Condition>, String> {
    let docs: Vec<ConditionDoc> = serde_json::from_value(raw.clone())
        .map_err(|e| format!("conditions: {}", first_line(&e.to_string())))?;
    if docs.len() > MAX_CONDITIONS {
        return Err(format!("at most {MAX_CONDITIONS} conditions per rule"));
    }
    docs.iter()
        .enumerate()
        .map(|(i, d)| Condition::compile(d).map_err(|e| format!("condition {}: {e}", i + 1)))
        .collect()
}

/// Quiet hours resolved for evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietWindow {
    /// Start, minutes after local midnight.
    pub start_min: i32,
    /// End (exclusive), minutes after local midnight.
    pub end_min: i32,
    /// Current UTC offset of the zone, seconds.
    pub offset_secs: i32,
}

impl QuietWindow {
    /// Whether `now_ms` lies inside the window; it may wrap past midnight.
    #[must_use]
    pub fn contains(&self, now_ms: i64) -> bool {
        let local = now_ms.div_euclid(1000) + i64::from(self.offset_secs);
        let minute = i32::try_from(local.div_euclid(60).rem_euclid(1440)).unwrap_or(0);
        if self.start_min <= self.end_min {
            (self.start_min..self.end_min).contains(&minute)
        } else {
            minute >= self.start_min || minute < self.end_min
        }
    }
}

/// A rule ready to evaluate.
#[derive(Debug, Clone)]
pub struct CompiledRule {
    /// Row id.
    pub id: i64,
    /// Name, for the log.
    pub name: String,
    /// Source filter.
    pub source: Option<String>,
    /// Event type filter.
    pub kind: Option<String>,
    conditions: Vec<Condition>,
    /// Minimum seconds between two notifications.
    pub cooldown_secs: i64,
    /// Hourly cap.
    pub max_per_hour: Option<usize>,
    /// Quiet hours, if any.
    pub quiet: Option<QuietWindow>,
    title: Template,
    body: Template,
    /// Targets that receive the notification.
    pub target_ids: Vec<i64>,
}

/// Default title when the rule has none.
pub const DEFAULT_TITLE: &str = "{{rule}}";
/// Default body when the rule has none.
pub const DEFAULT_BODY: &str = "{{kind}}: {{value}}\n{{where}}";

impl CompiledRule {
    /// Compile from the stored pieces.
    ///
    /// # Errors
    /// A message naming what is wrong.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: i64,
        name: &str,
        source: Option<String>,
        kind: Option<String>,
        conditions: &Value,
        cooldown_secs: i32,
        max_per_hour: Option<i32>,
        quiet: Option<QuietWindow>,
        title: &str,
        body: &str,
        target_ids: Vec<i64>,
    ) -> Result<Self, String> {
        let title = Template::parse(if title.is_empty() {
            DEFAULT_TITLE
        } else {
            title
        })
        .map_err(|e| format!("title: {e}"))?;
        let body = Template::parse(if body.is_empty() { DEFAULT_BODY } else { body })
            .map_err(|e| format!("body: {e}"))?;
        Ok(Self {
            id,
            name: name.to_owned(),
            source,
            kind,
            conditions: compile_conditions(conditions)?,
            cooldown_secs: i64::from(cooldown_secs),
            max_per_hour: max_per_hour.and_then(|m| usize::try_from(m).ok()),
            quiet,
            title,
            body,
            target_ids,
        })
    }

    /// Whether the event (as [`context`] built it) matches source, kind and every condition.
    #[must_use]
    pub fn matches(&self, ctx: &Value) -> bool {
        let text = |key: &str| ctx.get(key).and_then(Value::as_str);
        self.source
            .as_deref()
            .is_none_or(|s| text("source") == Some(s))
            && self.kind.as_deref().is_none_or(|k| text("kind") == Some(k))
            && self.conditions.iter().all(|c| c.matches(ctx))
    }

    /// Render the title and body for an event.
    #[must_use]
    pub fn render(&self, ctx: &Value) -> (String, String) {
        let ctx = with_extras(ctx, &self.name);
        (
            self.title.render(&ctx, Escape::Text, 200).trim().to_owned(),
            self.body.render(&ctx, Escape::Text, 1500).trim().to_owned(),
        )
    }
}

/// The lookup context of an event row: the row's top-level fields plus `data`, which is the
/// decoder payload (`meta.data` of plugin events, `meta` of the built-in detectors).
#[must_use]
pub fn context(event: &Value) -> Value {
    let mut ctx = event.as_object().cloned().unwrap_or_default();
    let data = event
        .get("meta")
        .map(|meta| meta.get("data").unwrap_or(meta).clone())
        .unwrap_or(Value::Null);
    ctx.insert("data".to_owned(), data);
    Value::Object(ctx)
}

/// Derived values every template may use: `rule`, `time`, `freq_mhz`, `where`.
#[must_use]
pub fn with_extras(ctx: &Value, rule: &str) -> Value {
    let mut out = ctx.as_object().cloned().unwrap_or_default();
    out.insert("rule".to_owned(), json!(rule));
    let ts = ctx.get("ts_ms").and_then(Value::as_i64);
    if let Some(ts) = ts {
        out.insert("time".to_owned(), json!(iso_utc(ts)));
    }
    let mhz = ctx
        .get("freq_hz")
        .and_then(Value::as_f64)
        .map(|hz| format!("{:.4}", hz / 1e6));
    if let Some(mhz) = &mhz {
        out.insert("freq_mhz".to_owned(), json!(mhz));
    }
    let device = ctx
        .get("device")
        .and_then(Value::as_str)
        .filter(|d| *d != "-");
    let place: Vec<String> = [
        device.map(str::to_owned),
        mhz.map(|m| format!("{m} MHz")),
        ts.map(iso_utc),
    ]
    .into_iter()
    .flatten()
    .collect();
    out.insert("where".to_owned(), json!(place.join(" · ")));
    Value::Object(out)
}

/// `2026-11-09T12:00:00Z` for Unix milliseconds.
#[must_use]
pub fn iso_utc(ts_ms: i64) -> String {
    let secs = ts_ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Why a rule did not fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Send it.
    Fire,
    /// Inside the rule's quiet hours.
    Quiet,
    /// Too soon after the previous notification.
    Cooldown,
    /// The hourly cap is used up.
    RateLimited,
}

#[derive(Debug, Default)]
struct RuleState {
    last_fired_ms: Option<i64>,
    fired: VecDeque<i64>,
}

/// Remembers when each rule last fired. Held in memory by the worker: a restart forgets the
/// cooldowns (the first match after it fires), which errs on the side of notifying.
#[derive(Debug, Default)]
pub struct Limiter {
    state: HashMap<i64, RuleState>,
}

const HOUR_MS: i64 = 3_600_000;

impl Limiter {
    /// Decide for `rule` at `now_ms`; a [`Verdict::Fire`] is recorded.
    pub fn check(&mut self, rule: &CompiledRule, now_ms: i64) -> Verdict {
        if rule.quiet.is_some_and(|q| q.contains(now_ms)) {
            return Verdict::Quiet;
        }
        let state = self.state.entry(rule.id).or_default();
        while state.fired.front().is_some_and(|t| now_ms - t >= HOUR_MS) {
            state.fired.pop_front();
        }
        if rule.cooldown_secs > 0
            && state
                .last_fired_ms
                .is_some_and(|last| now_ms - last < rule.cooldown_secs * 1000)
        {
            return Verdict::Cooldown;
        }
        if rule
            .max_per_hour
            .is_some_and(|max| state.fired.len() >= max)
        {
            return Verdict::RateLimited;
        }
        state.last_fired_ms = Some(now_ms);
        state.fired.push_back(now_ms);
        Verdict::Fire
    }

    /// Forget rules that no longer exist.
    pub fn retain(&mut self, ids: &[i64]) {
        self.state.retain(|id, _| ids.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pocsag() -> Value {
        json!({
            "id": 42, "ts_ms": 1_800_000_000_123_i64, "device": "rtl0", "vfo": 2,
            "freq_hz": 153_350_000, "mode": null, "kind": "pocsag.message",
            "source": "org.superhet.decoder.pocsag", "value": "FIRE at Main Street",
            "meta": {"instance": "i-1", "data": {"address": 1234, "text": "FIRE at Main Street", "baud": 1200}},
        })
    }

    fn rule_with(conditions: Value) -> CompiledRule {
        CompiledRule::new(
            1,
            "pager",
            Some("org.superhet.decoder.pocsag".into()),
            Some("pocsag.message".into()),
            &conditions,
            0,
            None,
            None,
            "",
            "",
            vec![1],
        )
        .expect("rule")
    }

    fn doc(field: &str, op: &str, value: Value) -> Value {
        json!({"field": field, "op": op, "value": value})
    }

    #[test]
    fn source_kind_and_conditions_must_all_hold() {
        let ctx = context(&pocsag());
        assert!(rule_with(json!([])).matches(&ctx));
        assert!(rule_with(json!([doc("data.address", "equals", json!(1234))])).matches(&ctx));
        // The RIC as text works too, and a different RIC does not.
        assert!(rule_with(json!([doc("data.address", "equals", json!("1234"))])).matches(&ctx));
        assert!(!rule_with(json!([doc("data.address", "equals", json!(99))])).matches(&ctx));
        // Both conditions are required.
        let both = json!([
            doc("data.address", "equals", json!(1234)),
            doc("value", "contains", json!("flood"))
        ]);
        assert!(!rule_with(both).matches(&ctx));
        // Source and kind filters.
        let mut other = rule_with(json!([]));
        other.source = Some("org.superhet.decoder.adsb".into());
        assert!(!other.matches(&ctx));
        let mut other = rule_with(json!([]));
        other.kind = Some("dtmf".into());
        assert!(!other.matches(&ctx));
    }

    #[test]
    fn text_operators_ignore_case_by_default() {
        let ctx = context(&pocsag());
        assert!(rule_with(json!([doc("value", "contains", json!("main street"))])).matches(&ctx));
        assert!(
            rule_with(json!([doc(
                "value",
                "equals",
                json!("fire at main street")
            )]))
            .matches(&ctx)
        );
        assert!(
            rule_with(json!([doc("value", "contains", json!(["flood", "FIRE"]))])).matches(&ctx)
        );
        assert!(!rule_with(json!([doc("value", "not_contains", json!("fire"))])).matches(&ctx));
        assert!(rule_with(json!([doc("value", "not_equals", json!("other"))])).matches(&ctx));
        let exact = json!([{"field": "value", "op": "contains", "value": "main street", "case_sensitive": true}]);
        assert!(!rule_with(exact).matches(&ctx));
        // A missing field passes only the negative operators.
        assert!(rule_with(json!([doc("data.nope", "not_equals", json!("x"))])).matches(&ctx));
        assert!(!rule_with(json!([doc("data.nope", "equals", json!("x"))])).matches(&ctx));
    }

    #[test]
    fn numbers_compare_numerically_not_as_text() {
        let ctx = context(&pocsag());
        assert!(rule_with(json!([doc("data.address", "gt", json!(1000))])).matches(&ctx));
        assert!(rule_with(json!([doc("data.address", "gte", json!(1234))])).matches(&ctx));
        assert!(!rule_with(json!([doc("data.address", "lt", json!(1234))])).matches(&ctx));
        assert!(rule_with(json!([doc("freq_hz", "lte", json!("153350000"))])).matches(&ctx));
        // Text that is not a number never satisfies a numeric comparison.
        assert!(!rule_with(json!([doc("value", "gt", json!(1))])).matches(&ctx));
        assert!(rule_with(json!([doc("data.baud", "exists", Value::Null)])).matches(&ctx));
    }

    #[test]
    fn exists_checks_presence() {
        let ctx = context(&pocsag());
        let exists = |f: &str| rule_with(json!([{"field": f, "op": "exists"}])).matches(&ctx);
        assert!(exists("data.text"));
        assert!(!exists("data.missing"));
        assert!(!exists("mode"), "null counts as absent");
    }

    #[test]
    fn regex_conditions_search_the_text() {
        let ctx = context(&pocsag());
        assert!(
            rule_with(json!([doc(
                "value",
                "regex",
                json!(r"fire\s+at\s+\w+ street")
            )]))
            .matches(&ctx)
        );
        assert!(!rule_with(json!([doc("value", "regex", json!(r"^street"))])).matches(&ctx));
        let strict =
            json!([{"field": "value", "op": "regex", "value": "main", "case_sensitive": true}]);
        assert!(!rule_with(strict).matches(&ctx));
    }

    #[test]
    fn regex_work_is_bounded() {
        // Invalid, oversize and exploding patterns are refused when the rule is saved.
        for bad in [
            json!([doc("value", "regex", json!("("))]),
            json!([doc(
                "value",
                "regex",
                json!("a".repeat(MAX_VALUE_CHARS + 1))
            )]),
            json!([doc("value", "regex", json!(r"\p{L}{1000}{1000}"))]),
            json!([doc(
                "value",
                "regex",
                json!(
                    r"(((((((((((a{100}){100}){100}){100}){100}){100}){100}){100}){100}){100}){100}"
                )
            )]),
        ] {
            assert!(compile_conditions(&bad).is_err(), "{bad}");
        }
        // The classic catastrophic pattern runs in linear time on the clipped text.
        let c = compile_conditions(&json!([doc("value", "regex", json!(r"^(a+)+$"))])).expect("ok");
        let evil = json!({"value": format!("{}!{}", "a".repeat(2000), "a".repeat(100_000))});
        let started = std::time::Instant::now();
        assert!(!c[0].matches(&evil));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn invalid_conditions_are_rejected_with_a_reason() {
        let cases = [
            (
                json!([{"field": "", "op": "equals", "value": "x"}]),
                "field",
            ),
            (
                json!([{"field": "a b", "op": "equals", "value": "x"}]),
                "letters",
            ),
            (json!([{"field": "a", "op": "equals"}]), "value"),
            (
                json!([{"field": "a", "op": "gt", "value": "abc"}]),
                "number",
            ),
            (
                json!([{"field": "a", "op": "exists", "value": 1}]),
                "no value",
            ),
            (
                json!([{"field": "a", "op": "nope", "value": 1}]),
                "conditions",
            ),
            (
                json!([{"field": "a", "op": "equals", "value": "x", "extra": 1}]),
                "conditions",
            ),
            (
                json!([{"field": "a", "op": "equals", "value": "x".repeat(300)}]),
                "256",
            ),
            (
                json!([{"field": "a", "op": "equals", "value": []}]),
                "values",
            ),
        ];
        for (raw, needle) in cases {
            let err = compile_conditions(&raw).expect_err(&raw.to_string());
            assert!(err.contains(needle), "{raw}: {err}");
        }
        let many = Value::Array(vec![doc("a", "exists", Value::Null); MAX_CONDITIONS + 1]);
        assert!(compile_conditions(&many).is_err());
    }

    #[test]
    fn the_context_exposes_decoder_payloads_as_data() {
        let ctx = context(&pocsag());
        assert_eq!(ctx["data"]["address"], 1234);
        assert_eq!(ctx["kind"], "pocsag.message");
        // Built-in detectors keep their extras directly in meta.
        let dtmf =
            json!({"kind": "dtmf", "source": "core", "value": "5", "meta": {"duration_ms": 80}});
        assert_eq!(context(&dtmf)["data"]["duration_ms"], 80);
    }

    #[test]
    fn titles_and_bodies_use_the_event() {
        let rule = CompiledRule::new(
            1,
            "Fire pager",
            None,
            None,
            &json!([]),
            0,
            None,
            None,
            "{{rule}}: RIC {{data.address}}",
            "{{data.text}}\n{{where}}",
            vec![],
        )
        .expect("rule");
        let (title, body) = rule.render(&context(&pocsag()));
        assert_eq!(title, "Fire pager: RIC 1234");
        assert!(
            body.starts_with("FIRE at Main Street\nrtl0 · 153.3500 MHz · 2027-01-15T08:00:00Z"),
            "{body}"
        );
        // Defaults when the rule has no templates.
        let plain = rule_with(json!([]));
        let (title, body) = plain.render(&context(&pocsag()));
        assert_eq!(title, "pager");
        assert!(
            body.starts_with("pocsag.message: FIRE at Main Street\nrtl0"),
            "{body}"
        );
    }

    #[test]
    fn iso_time_is_correct() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_800_000_000_123), "2027-01-15T08:00:00Z");
    }

    fn limited(cooldown: i32, per_hour: Option<i32>, quiet: Option<QuietWindow>) -> CompiledRule {
        CompiledRule::new(
            7,
            "r",
            None,
            None,
            &json!([]),
            cooldown,
            per_hour,
            quiet,
            "",
            "",
            vec![1],
        )
        .expect("rule")
    }

    #[test]
    fn cooldown_suppresses_repeats_until_it_has_passed() {
        let rule = limited(60, None, None);
        let mut limiter = Limiter::default();
        assert_eq!(limiter.check(&rule, 1_000_000), Verdict::Fire);
        assert_eq!(limiter.check(&rule, 1_030_000), Verdict::Cooldown);
        assert_eq!(limiter.check(&rule, 1_059_999), Verdict::Cooldown);
        assert_eq!(limiter.check(&rule, 1_060_000), Verdict::Fire);
        // Suppressed events do not extend the cooldown.
        assert_eq!(limiter.check(&rule, 1_100_000), Verdict::Cooldown);
        assert_eq!(limiter.check(&rule, 1_120_000), Verdict::Fire);
    }

    #[test]
    fn the_hourly_cap_slides() {
        let rule = limited(0, Some(3), None);
        let mut limiter = Limiter::default();
        for t in [0, 10_000, 20_000] {
            assert_eq!(limiter.check(&rule, t), Verdict::Fire);
        }
        assert_eq!(limiter.check(&rule, 30_000), Verdict::RateLimited);
        assert_eq!(limiter.check(&rule, 3_599_999), Verdict::RateLimited);
        // The first one aged out of the hour.
        assert_eq!(limiter.check(&rule, 3_600_000), Verdict::Fire);
        assert_eq!(limiter.check(&rule, 3_600_001), Verdict::RateLimited);
    }

    #[test]
    fn limits_are_per_rule() {
        let a = limited(60, None, None);
        let mut b = limited(60, None, None);
        b.id = 8;
        let mut limiter = Limiter::default();
        assert_eq!(limiter.check(&a, 0), Verdict::Fire);
        assert_eq!(limiter.check(&b, 1), Verdict::Fire);
        assert_eq!(limiter.check(&a, 2), Verdict::Cooldown);
        limiter.retain(&[8]);
        assert_eq!(
            limiter.check(&a, 3),
            Verdict::Fire,
            "state of a deleted rule is gone"
        );
    }

    #[test]
    fn quiet_hours_wrap_midnight_and_follow_the_zone() {
        // 22:00-06:00 at UTC+1.
        let window = QuietWindow {
            start_min: 22 * 60,
            end_min: 6 * 60,
            offset_secs: 3600,
        };
        let at = |utc_h: i64, utc_m: i64| (utc_h * 3600 + utc_m * 60) * 1000;
        assert!(window.contains(at(21, 0)), "22:00 local");
        assert!(window.contains(at(0, 30)), "01:30 local");
        assert!(window.contains(at(4, 59)), "05:59 local");
        assert!(!window.contains(at(5, 0)), "06:00 local");
        assert!(!window.contains(at(12, 0)));
        assert!(!window.contains(at(20, 59)), "21:59 local");
        // Same-day window, negative offset.
        let day = QuietWindow {
            start_min: 9 * 60,
            end_min: 17 * 60,
            offset_secs: -5 * 3600,
        };
        assert!(day.contains(at(14, 0)), "09:00 local");
        assert!(!day.contains(at(22, 0)), "17:00 local");
        // A quiet rule does not fire and leaves no trace.
        let rule = limited(0, Some(1), Some(window));
        let mut limiter = Limiter::default();
        assert_eq!(limiter.check(&rule, at(0, 30)), Verdict::Quiet);
        assert_eq!(limiter.check(&rule, at(12, 0)), Verdict::Fire);
    }
}
