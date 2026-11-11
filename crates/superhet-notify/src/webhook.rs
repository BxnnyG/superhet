//! Webhook delivery: a JSON (or templated) POST, optionally signed.
//!
//! Signing: when the target has `sign` on, the request carries
//! `X-Superhet-Timestamp: <unix seconds>` and `X-Superhet-Signature: sha256=<hex>`, the
//! HMAC-SHA256 of `"<timestamp>.<body>"` under the target's secret. Receivers recompute it and
//! reject old timestamps. `X-Superhet-Delivery` carries the delivery id (stable across retries,
//! usable for de-duplication on the receiving side).

use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use ring::hmac;
use serde_json::json;

use crate::egress::{Egress, Request, SendError, status_error};
use crate::message::Message;
use crate::target::Webhook;
use crate::template::Escape;

/// Longest request body we build.
const MAX_BODY_CHARS: usize = 16_000;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// `sha256=<hex>` over `"<timestamp>.<body>"`.
#[must_use]
pub fn signature(secret: &str, timestamp_secs: i64, body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let mut signed = format!("{timestamp_secs}.").into_bytes();
    signed.extend_from_slice(body);
    format!("sha256={}", hex(hmac::sign(&key, &signed).as_ref()))
}

/// The request body for a message.
#[must_use]
pub fn body(cfg: &Webhook, delivery_id: i64, message: &Message) -> (String, Vec<u8>) {
    match &cfg.body {
        Some(template) => {
            let escape = if cfg.content_type.to_ascii_lowercase().contains("json") {
                Escape::Json
            } else {
                Escape::Text
            };
            (
                cfg.content_type.clone(),
                template
                    .render(&message.context(), escape, MAX_BODY_CHARS)
                    .into_bytes(),
            )
        }
        None => {
            let doc = json!({
                "v": 1,
                "delivery": delivery_id,
                "rule": {"id": message.rule_id, "name": message.rule},
                "title": message.title,
                "body": message.body,
                "event": message.event,
            });
            ("application/json".to_owned(), doc.to_string().into_bytes())
        }
    }
}

/// The headers of the request.
///
/// # Errors
/// A permanent error if signing is on without a secret.
pub fn headers(
    cfg: &Webhook,
    secret: Option<&str>,
    delivery_id: i64,
    message: &Message,
    content_type: &str,
    payload: &[u8],
    now_secs: i64,
) -> Result<HeaderMap, SendError> {
    let mut map = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(content_type) {
        map.insert(CONTENT_TYPE, v);
    }
    map.insert(
        HeaderName::from_static("x-superhet-delivery"),
        HeaderValue::from(delivery_id),
    );
    let mut ctx = message.context();
    if let (Some(secret), Some(obj)) = (secret, ctx.as_object_mut()) {
        obj.insert("secret".to_owned(), json!(secret));
    }
    for (name, template) in &cfg.headers {
        let value = template.render(&ctx, Escape::Header, 600);
        if let Ok(v) = HeaderValue::from_str(value.trim()) {
            map.insert(name.clone(), v);
        }
    }
    if cfg.sign {
        let secret =
            secret.ok_or_else(|| SendError::permanent("signing is on but no secret is set"))?;
        map.insert(
            HeaderName::from_static("x-superhet-timestamp"),
            HeaderValue::from(now_secs),
        );
        if let Ok(v) = HeaderValue::from_str(&signature(secret, now_secs, payload)) {
            map.insert(HeaderName::from_static("x-superhet-signature"), v);
        }
    }
    Ok(map)
}

/// Send one webhook attempt.
///
/// # Errors
/// A [`SendError`] telling whether a retry can help.
pub async fn send(
    egress: &Egress,
    cfg: &Webhook,
    secret: Option<&str>,
    delivery_id: i64,
    message: &Message,
    now_ms: i64,
) -> Result<(), SendError> {
    let (content_type, payload) = body(cfg, delivery_id, message);
    let headers = headers(
        cfg,
        secret,
        delivery_id,
        message,
        &content_type,
        &payload,
        now_ms / 1000,
    )?;
    let status = egress
        .send(&Request {
            method: cfg.method.clone(),
            url: &cfg.url,
            headers,
            body: payload,
            timeout: cfg.timeout,
            allow_private: cfg.allow_private,
        })
        .await?;
    status_error(status).map_or(Ok(()), Err)
}
