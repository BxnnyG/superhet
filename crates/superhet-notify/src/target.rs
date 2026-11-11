//! Target settings: validation of what an admin enters, the typed form the senders use, and the
//! view the API may show (the webhook URL is write-only because such URLs carry tokens).

use std::str::FromStr;
use std::time::Duration;

use lettre::Address;
use reqwest::Method;
use reqwest::header::HeaderName;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::ssrf;
use crate::template::Template;

/// Kinds of target.
pub const KINDS: [&str; 3] = ["webhook", "email", "push"];
/// Longest webhook URL.
const MAX_URL: usize = 2048;
/// Most extra headers on a webhook.
const MAX_HEADERS: usize = 8;
/// Most recipients of an e-mail target.
const MAX_RECIPIENTS: usize = 5;
/// Headers an admin may not set: the client or the protocol owns them.
const FORBIDDEN_HEADERS: [&str; 9] = [
    "host",
    "content-length",
    "content-type",
    "transfer-encoding",
    "connection",
    "upgrade",
    "te",
    "trailer",
    "user-agent",
];

/// One extra webhook header; the value is a template (`Bearer {{secret}}`, `{{title}}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderDoc {
    /// Header name.
    pub name: String,
    /// Header value template.
    pub value: String,
}

fn d_post() -> String {
    "POST".to_owned()
}
fn d_json() -> String {
    "application/json".to_owned()
}
fn d_timeout() -> u64 {
    10
}
fn d_webhook_retries() -> u32 {
    4
}
fn d_email_retries() -> u32 {
    3
}
fn d_ttl() -> u32 {
    3600
}
fn d_urgency() -> String {
    "normal".to_owned()
}

/// Webhook settings as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookDoc {
    /// Destination (write-only for the API).
    #[serde(default)]
    pub url: Option<String>,
    /// `POST` or `PUT`.
    #[serde(default = "d_post")]
    pub method: String,
    /// Content type of a custom body.
    #[serde(default = "d_json")]
    pub content_type: String,
    /// Custom body template; absent = the standard JSON document.
    #[serde(default)]
    pub body_template: Option<String>,
    /// Extra headers.
    #[serde(default)]
    pub headers: Vec<HeaderDoc>,
    /// Time limit of one attempt.
    #[serde(default = "d_timeout")]
    pub timeout_secs: u64,
    /// Retries after the first attempt.
    #[serde(default = "d_webhook_retries")]
    pub retries: u32,
    /// Allow private / loopback / link-local destinations (explicit admin choice).
    #[serde(default)]
    pub allow_private: bool,
    /// Sign the body with the target's secret (HMAC-SHA256 headers).
    #[serde(default)]
    pub sign: bool,
}

/// E-mail settings as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailDoc {
    /// Recipients.
    pub to: Vec<String>,
    /// Retries after the first attempt.
    #[serde(default = "d_email_retries")]
    pub retries: u32,
}

/// Web Push settings as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushDoc {
    /// Accounts that receive it; empty = every account with a subscription.
    #[serde(default)]
    pub user_ids: Vec<i64>,
    /// How long the push service may hold the message for an offline browser.
    #[serde(default = "d_ttl")]
    pub ttl_secs: u32,
    /// `very-low`, `low`, `normal` or `high`.
    #[serde(default = "d_urgency")]
    pub urgency: String,
    /// Retries after the first attempt.
    #[serde(default = "d_email_retries")]
    pub retries: u32,
    /// Allow push endpoints in private ranges (only for a self-hosted push relay).
    #[serde(default)]
    pub allow_private: bool,
}

/// A webhook ready to send.
#[derive(Debug, Clone)]
pub struct Webhook {
    /// Destination.
    pub url: Url,
    /// Method.
    pub method: Method,
    /// Content type of the body.
    pub content_type: String,
    /// Custom body.
    pub body: Option<Template>,
    /// Extra headers.
    pub headers: Vec<(HeaderName, Template)>,
    /// Time limit of one attempt.
    pub timeout: Duration,
    /// Retries after the first attempt.
    pub retries: u32,
    /// Private destinations allowed.
    pub allow_private: bool,
    /// Sign the body.
    pub sign: bool,
}

/// A target's settings by kind.
#[derive(Debug, Clone)]
pub enum Config {
    /// HTTP request.
    Webhook(Webhook),
    /// SMTP mail.
    Email(EmailDoc),
    /// Web Push.
    Push(PushDoc),
}

impl Config {
    /// Retries after the first attempt.
    #[must_use]
    pub fn retries(&self) -> u32 {
        match self {
            Self::Webhook(w) => w.retries,
            Self::Email(e) => e.retries,
            Self::Push(p) => p.retries,
        }
    }
}

fn webhook(doc: &WebhookDoc) -> Result<Webhook, String> {
    let raw = doc.url.as_deref().ok_or("a URL is required")?;
    if raw.len() > MAX_URL {
        return Err(format!("the URL is limited to {MAX_URL} characters"));
    }
    let url = Url::parse(raw).map_err(|_| "the URL is not valid".to_owned())?;
    ssrf::check_url(&url, doc.allow_private)?;
    let method = match doc.method.to_ascii_uppercase().as_str() {
        "POST" => Method::POST,
        "PUT" => Method::PUT,
        _ => return Err("method must be POST or PUT".to_owned()),
    };
    if doc.content_type.len() > 100
        || doc.content_type.is_empty()
        || !doc
            .content_type
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/+-.;= ".contains(c))
    {
        return Err("content type is not valid".to_owned());
    }
    let body = match &doc.body_template {
        None => None,
        Some(t) if t.is_empty() => None,
        Some(t) if t.chars().count() > 4096 => {
            return Err("the body template is limited to 4096 characters".to_owned());
        }
        Some(t) => Some(Template::parse(t).map_err(|e| format!("body template: {e}"))?),
    };
    if doc.headers.len() > MAX_HEADERS {
        return Err(format!("at most {MAX_HEADERS} extra headers"));
    }
    let mut headers = Vec::new();
    for h in &doc.headers {
        let name = HeaderName::from_str(&h.name.to_ascii_lowercase()).map_err(|_| {
            format!(
                "header name {:?} is not valid",
                h.name.chars().take(40).collect::<String>()
            )
        })?;
        if FORBIDDEN_HEADERS.contains(&name.as_str())
            || name.as_str().starts_with("x-superhet-")
            || name.as_str().starts_with("proxy-")
            || name.as_str().starts_with("sec-")
        {
            return Err(format!("header {} cannot be set", name.as_str()));
        }
        if h.value.chars().count() > 500 {
            return Err("header values are limited to 500 characters".to_owned());
        }
        let value =
            Template::parse(&h.value).map_err(|e| format!("header {}: {e}", name.as_str()))?;
        headers.push((name, value));
    }
    if !(1..=30).contains(&doc.timeout_secs) {
        return Err("timeout must be 1 to 30 seconds".to_owned());
    }
    if doc.retries > 8 {
        return Err("at most 8 retries".to_owned());
    }
    Ok(Webhook {
        url,
        method,
        content_type: doc.content_type.clone(),
        body,
        headers,
        timeout: Duration::from_secs(doc.timeout_secs),
        retries: doc.retries,
        allow_private: doc.allow_private,
        sign: doc.sign,
    })
}

fn email(doc: &EmailDoc) -> Result<(), String> {
    if doc.to.is_empty() || doc.to.len() > MAX_RECIPIENTS {
        return Err(format!("give 1 to {MAX_RECIPIENTS} recipients"));
    }
    for to in &doc.to {
        if to.len() > 254 || Address::from_str(to).is_err() {
            return Err(format!(
                "{:?} is not an e-mail address",
                to.chars().take(60).collect::<String>()
            ));
        }
    }
    if doc.retries > 8 {
        return Err("at most 8 retries".to_owned());
    }
    Ok(())
}

fn push(doc: &PushDoc) -> Result<(), String> {
    if doc.user_ids.len() > 200 || doc.user_ids.iter().any(|id| *id <= 0) {
        return Err("user ids are not valid".to_owned());
    }
    if !(60..=86_400).contains(&doc.ttl_secs) {
        return Err("ttl must be 60 to 86400 seconds".to_owned());
    }
    if !["very-low", "low", "normal", "high"].contains(&doc.urgency.as_str()) {
        return Err("urgency must be very-low, low, normal or high".to_owned());
    }
    if doc.retries > 8 {
        return Err("at most 8 retries".to_owned());
    }
    Ok(())
}

fn parse_doc<T: for<'de> Deserialize<'de>>(value: &Value) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|e| {
        let text = e.to_string();
        format!("settings: {}", text.lines().next().unwrap_or("invalid"))
    })
}

/// Validate what an admin entered and return the document to store (defaults filled in). For a
/// webhook without a `url`, the one in `previous` is kept.
///
/// # Errors
/// A message for the admin.
pub fn normalize(kind: &str, input: &Value, previous: Option<&Value>) -> Result<Value, String> {
    match kind {
        "webhook" => {
            let mut doc: WebhookDoc = parse_doc(input)?;
            if doc.url.as_deref().is_none_or(str::is_empty) {
                doc.url = previous
                    .and_then(|p| p.get("url"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            webhook(&doc)?;
            serde_json::to_value(doc).map_err(|e| e.to_string())
        }
        "email" => {
            let doc: EmailDoc = parse_doc(input)?;
            email(&doc)?;
            serde_json::to_value(doc).map_err(|e| e.to_string())
        }
        "push" => {
            let doc: PushDoc = parse_doc(input)?;
            push(&doc)?;
            serde_json::to_value(doc).map_err(|e| e.to_string())
        }
        _ => Err("kind must be webhook, email or push".to_owned()),
    }
}

/// The typed settings from a stored document.
///
/// # Errors
/// A message if the stored document is not valid (it was validated on the way in).
pub fn parse(kind: &str, stored: &Value) -> Result<Config, String> {
    match kind {
        "webhook" => Ok(Config::Webhook(webhook(&parse_doc(stored)?)?)),
        "email" => {
            let doc: EmailDoc = parse_doc(stored)?;
            email(&doc)?;
            Ok(Config::Email(doc))
        }
        "push" => {
            let doc: PushDoc = parse_doc(stored)?;
            push(&doc)?;
            Ok(Config::Push(doc))
        }
        _ => Err("unknown target kind".to_owned()),
    }
}

/// The settings as the API shows them: the webhook URL is replaced by its host (`url_host`),
/// everything else is shown as stored.
#[must_use]
pub fn public_view(kind: &str, stored: &Value) -> Value {
    let mut view = stored.clone();
    if kind == "webhook"
        && let Some(map) = view.as_object_mut()
    {
        let host = map
            .remove("url")
            .and_then(|u| u.as_str().and_then(|u| Url::parse(u).ok()))
            .map(|u| match (u.host_str(), u.port()) {
                (Some(h), Some(p)) => format!("{}://{h}:{p}", u.scheme()),
                (Some(h), None) => format!("{}://{h}", u.scheme()),
                _ => String::new(),
            });
        map.insert("url_host".to_owned(), json!(host));
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhooks_get_defaults_and_keep_their_url_on_edit() {
        let stored = normalize(
            "webhook",
            &json!({"url": "https://hooks.example.com/abc?t=1"}),
            None,
        )
        .expect("ok");
        assert_eq!(stored["method"], "POST");
        assert_eq!(stored["retries"], 4);
        assert_eq!(stored["timeout_secs"], 10);
        let edited =
            normalize("webhook", &json!({"retries": 1}), Some(&stored)).expect("keeps url");
        assert_eq!(edited["url"], "https://hooks.example.com/abc?t=1");
        assert!(normalize("webhook", &json!({"retries": 1}), None).is_err());
    }

    #[test]
    fn the_url_is_not_shown_back() {
        let stored = normalize(
            "webhook",
            &json!({"url": "https://hooks.example.com:8443/secret/token?k=v"}),
            None,
        )
        .expect("ok");
        let view = public_view("webhook", &stored).to_string();
        assert!(
            !view.contains("secret") && !view.contains("token") && !view.contains("k=v"),
            "{view}"
        );
        assert!(view.contains("https://hooks.example.com:8443"));
    }

    #[test]
    fn unsafe_webhook_settings_are_rejected() {
        let bad = [
            json!({"url": "ftp://example.com/"}),
            json!({"url": "https://u:p@example.com/"}),
            json!({"url": "http://127.0.0.1:8080/"}),
            json!({"url": "http://169.254.169.254/"}),
            json!({"url": "https://example.com/", "method": "DELETE"}),
            json!({"url": "https://example.com/", "timeout_secs": 0}),
            json!({"url": "https://example.com/", "timeout_secs": 300}),
            json!({"url": "https://example.com/", "retries": 99}),
            json!({"url": "https://example.com/", "headers": [{"name": "Host", "value": "x"}]}),
            json!({"url": "https://example.com/", "headers": [{"name": "X-Superhet-Signature", "value": "x"}]}),
            json!({"url": "https://example.com/", "headers": [{"name": "bad name", "value": "x"}]}),
            json!({"url": "https://example.com/", "body_template": "{{oops"}),
            json!({"url": "https://example.com/", "unknown": 1}),
        ];
        for doc in bad {
            assert!(normalize("webhook", &doc, None).is_err(), "{doc}");
        }
        // Explicitly allowed private destinations pass; the metadata address never does.
        let ok = json!({"url": "http://192.168.1.5:8080/hook", "allow_private": true});
        assert!(normalize("webhook", &ok, None).is_ok());
        let meta = json!({"url": "http://169.254.169.254/", "allow_private": true});
        assert!(normalize("webhook", &meta, None).is_err());
    }

    #[test]
    fn email_and_push_settings_are_validated() {
        assert!(normalize("email", &json!({"to": ["ops@example.com"]}), None).is_ok());
        assert!(normalize("email", &json!({"to": []}), None).is_err());
        assert!(normalize("email", &json!({"to": ["not an address"]}), None).is_err());
        assert!(
            normalize(
                "email",
                &json!({"to": ["a@example.com\r\nBcc: x@example.com"]}),
                None
            )
            .is_err()
        );
        assert!(normalize("push", &json!({}), None).is_ok());
        assert!(
            normalize(
                "push",
                &json!({"user_ids": [1, 2], "urgency": "high"}),
                None
            )
            .is_ok()
        );
        assert!(normalize("push", &json!({"urgency": "urgent"}), None).is_err());
        assert!(normalize("push", &json!({"ttl_secs": 5}), None).is_err());
        assert!(normalize("sms", &json!({}), None).is_err());
    }

    #[test]
    fn stored_documents_parse_into_typed_settings() {
        let stored = normalize(
            "webhook",
            &json!({"url": "https://example.com/h", "retries": 2}),
            None,
        )
        .expect("ok");
        match parse("webhook", &stored).expect("parse") {
            Config::Webhook(w) => assert_eq!((w.retries, w.method), (2, Method::POST)),
            other => panic!("{other:?}"),
        }
    }
}
