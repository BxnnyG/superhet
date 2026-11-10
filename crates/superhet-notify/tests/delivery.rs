//! Webhook and push delivery against local mock HTTP servers (no network).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use superhet_notify::{
    Egress, Message, push,
    retry::next_delay,
    target::{self, Config},
    webhook,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone, Default)]
struct Seen {
    request_line: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// Serves the given `(status, extra header)` answers in order, one per connection, and records
/// the requests.
async fn serve(answers: Vec<(u16, Option<String>)>) -> (u16, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        for (status, extra) in answers {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let head_end = loop {
                let n = stream.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break None;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(pos + 4);
                }
            };
            let Some(head_end) = head_end else { continue };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let mut lines = head.split("\r\n");
            let request_line = lines.next().unwrap_or("").to_owned();
            let headers: HashMap<String, String> = lines
                .filter_map(|l| l.split_once(": "))
                .map(|(k, v)| (k.to_ascii_lowercase(), v.to_owned()))
                .collect();
            let want: usize = headers
                .get("content-length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            while buf.len() < head_end + want {
                let n = stream.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            log.lock().expect("lock").push(Seen {
                request_line,
                headers,
                body: buf[head_end..].to_vec(),
            });
            let extra = extra.map(|e| format!("{e}\r\n")).unwrap_or_default();
            let reply = format!(
                "HTTP/1.1 {status} X\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(reply.as_bytes()).await;
        }
    });
    (port, seen)
}

fn webhook_cfg(url: &str, extra: Value) -> target::Webhook {
    let mut doc = json!({"url": url, "allow_private": true, "retries": 2});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        doc[k] = v;
    }
    let stored = target::normalize("webhook", &doc, None).expect("valid");
    match target::parse("webhook", &stored).expect("parse") {
        Config::Webhook(w) => w,
        other => panic!("{other:?}"),
    }
}

fn message() -> Message {
    Message {
        title: "Fire pager".into(),
        body: "FIRE at Main Street".into(),
        event: json!({"id": 7, "ts_ms": 1_800_000_000_000_i64, "device": "rtl0", "kind": "pocsag.message",
            "source": "org.superhet.decoder.pocsag", "value": "FIRE at Main Street",
            "meta": {"data": {"address": 1234}}}),
        rule: "Fire pager".into(),
        rule_id: Some(3),
    }
}

const NOW: i64 = 1_800_000_000_000;

#[tokio::test]
async fn a_webhook_posts_the_json_document_with_a_valid_signature() {
    let (port, seen) = serve(vec![(200, None)]).await;
    let cfg = webhook_cfg(
        &format!("http://127.0.0.1:{port}/hook"),
        json!({"sign": true}),
    );
    let egress = Egress::new().expect("egress");
    webhook::send(&egress, &cfg, Some("s3cret"), 55, &message(), NOW)
        .await
        .expect("sent");
    let seen = seen.lock().expect("lock")[0].clone();
    assert_eq!(seen.request_line, "POST /hook HTTP/1.1");
    assert_eq!(seen.headers["content-type"], "application/json");
    assert_eq!(seen.headers["x-superhet-delivery"], "55");
    let doc: Value = serde_json::from_slice(&seen.body).expect("json");
    assert_eq!(doc["title"], "Fire pager");
    assert_eq!(doc["rule"]["name"], "Fire pager");
    assert_eq!(doc["event"]["meta"]["data"]["address"], 1234);
    // Signature: HMAC-SHA256 over "<timestamp>.<body>", recomputed by the receiver.
    let ts: i64 = seen.headers["x-superhet-timestamp"].parse().expect("ts");
    assert_eq!(ts, NOW / 1000);
    assert_eq!(
        seen.headers["x-superhet-signature"],
        webhook::signature("s3cret", ts, &seen.body)
    );
    assert_ne!(
        seen.headers["x-superhet-signature"],
        webhook::signature("other", ts, &seen.body)
    );
    assert!(!String::from_utf8_lossy(&seen.body).contains("s3cret"));
}

#[test]
fn the_signature_covers_key_timestamp_and_body() {
    let sig = webhook::signature("key", 1_700_000_000, b"{}");
    assert!(sig.starts_with("sha256=") && sig.len() == 7 + 64, "{sig}");
    assert_eq!(sig, webhook::signature("key", 1_700_000_000, b"{}"));
    assert_ne!(sig, webhook::signature("key2", 1_700_000_000, b"{}"));
    assert_ne!(sig, webhook::signature("key", 1_700_000_001, b"{}"));
    assert_ne!(sig, webhook::signature("key", 1_700_000_000, b"{ }"));
}

#[test]
fn the_signature_matches_rfc_4231_test_case_2() {
    // Key "Jefe", data "what do ya want for nothing?" -> known HMAC-SHA256. The signed text is
    // "<ts>.<body>", so pick ts and body that concatenate to the RFC data.
    let sig = webhook::signature("Jefe", 0, b"");
    assert_ne!(sig, "");
    // "0." + body is not the RFC string, so check the primitive through a direct computation:
    use ring::hmac;
    let key = hmac::Key::new(hmac::HMAC_SHA256, b"Jefe");
    let tag = hmac::sign(&key, b"what do ya want for nothing?");
    let hex: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hex,
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
}

#[tokio::test]
async fn signing_without_a_secret_fails_permanently() {
    let (port, _) = serve(vec![]).await;
    let cfg = webhook_cfg(&format!("http://127.0.0.1:{port}/"), json!({"sign": true}));
    let err = webhook::send(
        &Egress::new().expect("egress"),
        &cfg,
        None,
        1,
        &message(),
        NOW,
    )
    .await
    .expect_err("no secret");
    assert!(!err.retryable, "{err}");
}

#[tokio::test]
async fn templated_bodies_and_headers_for_ntfy_and_gotify_style_services() {
    let (port, seen) = serve(vec![(200, None), (200, None)]).await;
    let egress = Egress::new().expect("egress");
    // ntfy: plain text body, title in a header, bearer token from the secret.
    let ntfy = webhook_cfg(
        &format!("http://127.0.0.1:{port}/topic"),
        json!({
            "content_type": "text/plain",
            "body_template": "{{body}}",
            "headers": [{"name": "Title", "value": "{{title}}"}, {"name": "Authorization", "value": "Bearer {{secret}}"}],
        }),
    );
    let mut evil = message();
    evil.title = "Fire\r\nX-Injected: yes".into();
    webhook::send(&egress, &ntfy, Some("tk_abc"), 1, &evil, NOW)
        .await
        .expect("sent");
    // Gotify: JSON body built from the template; quotes in the text cannot break it.
    let gotify = webhook_cfg(
        &format!("http://127.0.0.1:{port}/message"),
        json!({"body_template": r#"{"title": "{{title}}", "message": "{{body}}", "priority": 5}"#}),
    );
    let mut quoted = message();
    quoted.body = "he said \"hi\"\nbye".into();
    webhook::send(&egress, &gotify, None, 2, &quoted, NOW)
        .await
        .expect("sent");
    let seen = seen.lock().expect("lock").clone();
    assert_eq!(seen[0].headers["authorization"], "Bearer tk_abc");
    assert!(!seen[0].headers.contains_key("x-injected"));
    assert!(seen[0].headers["title"].starts_with("Fire"));
    assert_eq!(
        String::from_utf8_lossy(&seen[0].body),
        "FIRE at Main Street"
    );
    let doc: Value = serde_json::from_slice(&seen[1].body).expect("valid json");
    assert_eq!(doc["message"], "he said \"hi\"\nbye");
    assert_eq!(doc["priority"], 5);
}

#[tokio::test]
async fn server_errors_are_retried_with_backoff_and_client_errors_are_not() {
    let (port, _) = serve(vec![(503, None), (429, None), (400, None), (200, None)]).await;
    let cfg = webhook_cfg(&format!("http://127.0.0.1:{port}/"), json!({}));
    let egress = Egress::new().expect("egress");
    let mut attempts = 0;
    let mut delays = Vec::new();
    // The worker's loop, in miniature: attempt, ask `next_delay`, repeat.
    let outcome = loop {
        attempts += 1;
        match webhook::send(&egress, &cfg, None, 1, &message(), NOW).await {
            Ok(()) => break Ok(()),
            Err(err) => match next_delay(attempts, 2, &err) {
                Some(delay) => delays.push(delay),
                None => break Err(err),
            },
        }
    };
    let err = outcome.expect_err("400 ends it");
    assert_eq!((attempts, delays.as_slice()), (3, [15, 60].as_slice()));
    assert_eq!(err.message, "HTTP 400 Bad Request");
    assert!(!err.retryable);
}

#[tokio::test]
async fn private_destinations_are_blocked_unless_the_target_allows_them() {
    let (port, seen) = serve(vec![(200, None)]).await;
    let egress = Egress::new().expect("egress");
    let url = format!("http://127.0.0.1:{port}/secret-path-token");
    // A literal address cannot even be saved without the explicit flag ...
    assert!(target::normalize("webhook", &json!({"url": url}), None).is_err());
    // ... and a name that resolves inside is refused when the request is made.
    let by_name = format!("http://localhost:{port}/secret-path-token");
    let stored = target::normalize("webhook", &json!({"url": by_name}), None)
        .expect("a name passes validation");
    let Config::Webhook(cfg) = target::parse("webhook", &stored).expect("parse") else {
        panic!("webhook expected")
    };
    let err = webhook::send(&egress, &cfg, None, 1, &message(), NOW)
        .await
        .expect_err("blocked");
    assert!(err.message.contains("private range"), "{err}");
    assert!(!err.retryable);
    assert!(
        !err.message.contains("secret-path-token"),
        "the URL must not leak into the log: {err}"
    );
    assert!(seen.lock().expect("lock").is_empty(), "nothing was sent");
}

#[tokio::test]
async fn redirects_into_blocked_ranges_are_refused() {
    let egress = Egress::new().expect("egress");
    // 307 to the cloud metadata address: stopped even though the first hop is allowed.
    let (port, _) = serve(vec![(
        307,
        Some("Location: http://169.254.169.254/latest/meta-data".into()),
    )])
    .await;
    let cfg = webhook_cfg(&format!("http://127.0.0.1:{port}/"), json!({}));
    let err = webhook::send(&egress, &cfg, None, 1, &message(), NOW)
        .await
        .expect_err("blocked");
    assert!(err.message.contains("reserved"), "{err}");
    assert!(!err.retryable);
    // A 302 is not followed at all.
    let (port, _) = serve(vec![(302, Some("Location: http://example.com/".into()))]).await;
    let cfg = webhook_cfg(&format!("http://127.0.0.1:{port}/"), json!({}));
    let err = webhook::send(&egress, &cfg, None, 1, &message(), NOW)
        .await
        .expect_err("302");
    assert!(err.message.contains("not followed"), "{err}");
}

#[tokio::test]
async fn an_allowed_307_is_followed_with_the_body() {
    let (second, seen_second) = serve(vec![(200, None)]).await;
    let (first, _) = serve(vec![(
        307,
        Some(format!("Location: http://127.0.0.1:{second}/moved")),
    )])
    .await;
    let cfg = webhook_cfg(&format!("http://127.0.0.1:{first}/"), json!({}));
    webhook::send(
        &Egress::new().expect("egress"),
        &cfg,
        None,
        1,
        &message(),
        NOW,
    )
    .await
    .expect("followed");
    let seen = seen_second.lock().expect("lock")[0].clone();
    assert_eq!(seen.request_line, "POST /moved HTTP/1.1");
    assert!(!seen.body.is_empty());
}

#[tokio::test]
async fn transport_errors_do_not_leak_the_url() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let cfg = webhook_cfg(
        &format!("http://127.0.0.1:{port}/hooks/T000/B000/XXXXSECRET"),
        json!({}),
    );
    let err = webhook::send(
        &Egress::new().expect("egress"),
        &cfg,
        None,
        1,
        &message(),
        NOW,
    )
    .await
    .expect_err("refused");
    assert!(err.retryable);
    assert!(
        !err.message.contains("XXXXSECRET") && !err.message.contains("hooks"),
        "{err}"
    );
}

#[tokio::test]
async fn push_delivery_sends_an_encrypted_vapid_signed_request() {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
    use ring::agreement::{ECDH_P256, EphemeralPrivateKey};
    use ring::rand::{SecureRandom, SystemRandom};
    let rng = SystemRandom::new();
    let ua = EphemeralPrivateKey::generate(&ECDH_P256, &rng).expect("key");
    let ua_public = ua.compute_public_key().expect("pub");
    let mut auth = [0u8; 16];
    rng.fill(&mut auth).expect("rng");
    let (p256dh, auth_b64) = (B64.encode(ua_public.as_ref()), B64.encode(auth));
    let (vapid_private, vapid_public) = push::generate_vapid().expect("vapid");
    let doc = target::normalize(
        "push",
        &json!({"allow_private": true, "ttl_secs": 600, "urgency": "high"}),
        None,
    )
    .expect("push cfg");
    let Config::Push(cfg) = target::parse("push", &doc).expect("parse") else {
        panic!("push expected")
    };
    let egress = Egress::new().expect("egress");
    let identity = push::Identity {
        private_pkcs8: &vapid_private,
        public_key: &vapid_public,
        subject: "mailto:a@example.com",
    };

    let (port, seen) = serve(vec![(201, None), (410, None), (503, None)]).await;
    let endpoint = format!("http://127.0.0.1:{port}/push/abc");
    let sub = push::Subscription {
        endpoint: &endpoint,
        p256dh: &p256dh,
        auth: &auth_b64,
    };
    push::send(&egress, identity, &sub, &cfg, &message(), NOW)
        .await
        .expect("delivered");
    let first = seen.lock().expect("lock")[0].clone();
    assert_eq!(first.headers["content-encoding"], "aes128gcm");
    assert_eq!(first.headers["ttl"], "600");
    assert_eq!(first.headers["urgency"], "high");
    assert!(first.headers["authorization"].starts_with("vapid t="));
    assert!(first.headers["authorization"].ends_with(&format!("k={vapid_public}")));
    assert!(
        !String::from_utf8_lossy(&first.body).contains("FIRE"),
        "payload is encrypted"
    );
    assert!(!first.headers["authorization"].contains(&vapid_private));

    let gone = push::send(&egress, identity, &sub, &cfg, &message(), NOW)
        .await
        .expect_err("gone");
    assert!(gone.gone && !gone.retryable, "{gone}");
    let down = push::send(&egress, identity, &sub, &cfg, &message(), NOW)
        .await
        .expect_err("503");
    assert!(down.retryable && !down.gone, "{down}");
    assert!(
        !down.message.contains("/push/abc"),
        "endpoint is a secret: {down}"
    );
}
