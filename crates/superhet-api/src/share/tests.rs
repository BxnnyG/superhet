//! Access tests of the share door: every way a token can be refused, and what an allowed one
//! still may not do.

use std::collections::{BTreeSet, HashSet};

use axum::{Router, body::Body, http::Request, http::StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::links::{Feature, Kind, Link, Tuning, test_link};
use super::*;
use crate::origin::OriginPolicy;

const TOKEN: &str = "tok-main";

fn app(link: Link) -> (Router, AuthState) {
    let auth = AuthState::for_tests_with_shares(links::ShareLinks::for_tests(vec![(TOKEN, link)]));
    let devices = DevicesState::new(None, OriginPolicy::default(), auth.clone());
    let state = ShareState::new(
        devices,
        Arc::new(Hub::new(None)),
        Arc::new(AudioHub::new(None)),
    );
    (router(state), auth)
}

async fn get(app: &Router, path: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("infallible");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn features(list: &[Feature]) -> BTreeSet<Feature> {
    list.iter().copied().collect()
}

fn path(tail: &str) -> String {
    format!("/api/v1/share/{TOKEN}{tail}")
}

#[tokio::test]
async fn an_unknown_token_is_a_404_and_runs_into_the_limiter() {
    let (app, _) = app(test_link(1));
    let (status, body) = get(&app, "/api/v1/share/nope").await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("not_found"))
    );
    // The same address guessing on: refused after a few misses, even for the right token.
    let mut last = StatusCode::OK;
    for _ in 0..25 {
        last = get(&app, "/api/v1/share/nope").await.0;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(get(&app, &path("")).await.0, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn expired_and_revoked_links_are_gone_on_every_route() {
    let mut expired = test_link(1);
    expired.expires_at_ms = Some(1);
    let (app_expired, _) = app(expired);
    let mut revoked = test_link(2);
    revoked.revoked_at_ms = Some(1);
    let (app_revoked, _) = app(revoked);
    for tail in ["", "/state", "/ws/spectrum", "/ws/audio", "/ws/events"] {
        let (status, body) = get(&app_expired, &path(tail)).await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::GONE, Some("expired")),
            "{tail}"
        );
        let (status, body) = get(&app_revoked, &path(tail)).await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::GONE, Some("revoked")),
            "{tail}"
        );
    }
}

#[tokio::test]
async fn a_route_needs_its_feature() {
    let cases: &[(&[Feature], &str, bool)] = &[
        (&[Feature::Waterfall], "/ws/spectrum", true),
        (&[Feature::Waterfall], "/ws/audio", false),
        (&[Feature::Waterfall], "/ws/events", false),
        (&[Feature::Audio], "/ws/spectrum", false),
        (&[Feature::Audio], "/ws/audio", true),
        (&[Feature::PluginAudio], "/ws/audio", true),
        (&[Feature::PluginAudio], "/ws/spectrum", false),
        (&[Feature::PluginAudio], "/state", true),
        (&[Feature::DecoderEvents], "/ws/events", true),
        (&[Feature::DecoderEvents], "/ws/audio", false),
        (&[Feature::DecoderEvents], "/ws/spectrum", false),
        (&[Feature::DecoderEvents], "/state", false),
        (&[Feature::Audio], "/ws/events", false),
    ];
    for (list, tail, allowed) in cases {
        let mut link = test_link(1);
        link.features = features(list);
        let (app, _) = app(link);
        let (status, body) = get(&app, &path(tail)).await;
        if *allowed {
            // Past the gate; a plain GET is then no WebSocket upgrade, or there is no bus.
            assert!(
                !matches!(
                    status,
                    StatusCode::FORBIDDEN | StatusCode::NOT_FOUND | StatusCode::GONE
                ),
                "{list:?} {tail} -> {status}"
            );
        } else {
            assert_eq!(
                (status, body["error"].as_str()),
                (StatusCode::FORBIDDEN, Some("feature_off")),
                "{list:?} {tail}"
            );
        }
    }
}

#[tokio::test]
async fn the_link_caps_refuse_with_503_and_free_up_again() {
    let (app, auth) = app(test_link(1)); // max_listeners = 2
    let link = test_link(1);
    let held: Vec<_> = (0..2)
        .map(|n| {
            auth.share_live()
                .acquire(
                    &link,
                    Kind::Spectrum,
                    Some(std::net::IpAddr::from([192, 0, 2, n + 1])),
                )
                .expect("slot")
        })
        .collect();
    let (status, body) = get(&app, &path("/ws/spectrum")).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("link_full"))
    );
    // The audio pool is separate, and the info route takes no slot.
    assert_ne!(
        get(&app, &path("/ws/audio")).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(get(&app, &path("")).await.0, StatusCode::OK);
    drop(held);
    assert_ne!(
        get(&app, &path("/ws/spectrum")).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn a_request_that_never_reached_a_websocket_holds_no_slot() {
    let (app, auth) = app(test_link(1));
    for _ in 0..5 {
        let _ = get(&app, &path("/ws/spectrum")).await;
    }
    assert_eq!(auth.share_live().count(1, Kind::Spectrum), 0);
}

#[tokio::test]
async fn the_info_route_shows_the_station_and_the_pinned_device_only() {
    let mut link = test_link(1);
    link.label = "internal label".into();
    link.created_by = Some("admin-name".into());
    link.embed_origins = vec!["https://blog.example.org".into()];
    link.device = "dev7".into();
    link.title = "Hilltop".into();
    let (app, _) = app(link);
    // A client-supplied device changes nothing: the link names the device.
    let (status, body) = get(&app, &path("?dev=other")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["device"], "dev7");
    assert_eq!(body["title"], "Hilltop");
    assert_eq!(body["tuning"]["mode"], "follow");
    let text = body.to_string();
    for secret in ["internal label", "admin-name", "blog.example.org", "token"] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
}

#[test]
fn route_table_and_token_parsing() {
    assert_eq!(
        need_of("/api/v1/share/{token}/ws/audio"),
        Some(Need::Ws(Kind::Audio))
    );
    assert_eq!(need_of("/api/v1/ws/audio"), None);
    assert_eq!(token_of("/api/v1/share/abc/ws/audio"), Some("abc"));
    assert_eq!(token_of("/api/v1/share/abc"), Some("abc"));
    assert_eq!(token_of("/api/v1/share/"), None);
    assert_eq!(token_of("/api/v1/other/abc"), None);
}

#[test]
fn decide_checks_status_before_features() {
    let mut link = test_link(1);
    link.features = features(&[Feature::Waterfall]);
    assert_eq!(
        decide(&link, Need::Ws(Kind::Audio), 0),
        Err(Denial::FeatureOff)
    );
    assert_eq!(decide(&link, Need::Ws(Kind::Spectrum), 0), Ok(()));
    link.revoked_at_ms = Some(1);
    assert_eq!(
        decide(&link, Need::Ws(Kind::Spectrum), 0),
        Err(Denial::Revoked)
    );
    link.revoked_at_ms = None;
    link.expires_at_ms = Some(5);
    assert_eq!(decide(&link, Need::Info, 5), Err(Denial::Expired));
}

#[test]
fn a_device_hidden_from_guests_is_hidden_from_links() {
    use crate::device_access::DevicePolicy;
    let open = DevicePolicy::default();
    assert!(open.allows(&guest_subject(), DeviceNeed::View));
    for hidden in [
        DevicePolicy {
            guests_visible: false,
            ..DevicePolicy::default()
        },
        DevicePolicy {
            view_role: Role::Operator,
            ..DevicePolicy::default()
        },
        DevicePolicy {
            allowed_users: vec![1],
            ..DevicePolicy::default()
        },
    ] {
        assert!(!hidden.allows(&guest_subject(), DeviceNeed::View));
    }
}

fn state_doc() -> Value {
    json!({
        "center_hz": 100_000_000, "sample_rate": 2_400_000, "gain": {"mode": "auto"},
        "locks": [{"owner": "u:1"}],
        "vfos": [
            {"id": 1, "freq_hz": 100_100_000, "mode": "wfm", "bandwidth_hz": 180_000, "owner": "u:1"},
            {"id": 2, "freq_hz": 100_200_000, "mode": "nfm", "bandwidth_hz": 12_500, "owner": "g:abcdef"},
            {"id": 3, "freq_hz": 100_300_000, "mode": "am", "bandwidth_hz": 10_000, "owner": "u:2"},
            {"id": 4, "freq_hz": 100_400_000, "mode": "am", "bandwidth_hz": 10_000},
            {"id": 5, "freq_hz": 100_500_000, "mode": "nfm", "bandwidth_hz": 12_500, "owner": "s:9:1"}
        ]
    })
}

fn operators() -> HashSet<i64> {
    HashSet::from([1])
}

fn ids(list: &[u32]) -> BTreeSet<u32> {
    list.iter().copied().collect()
}

#[test]
fn a_following_link_plays_only_operator_and_shared_vfos() {
    let link = test_link(1);
    let doc = state_doc();
    let ok = |wanted: &[u32]| audio_refusal(&link, Some(&doc), &operators(), None, &ids(wanted));
    assert_eq!(ok(&[1]), Ok(()));
    assert_eq!(ok(&[1, 4]), Ok(()));
    for refused in [2, 3, 5, 99] {
        assert_eq!(ok(&[1, refused]), Err("forbidden"), "vfo {refused}");
    }
    // No state to check against: nothing is allowed.
    assert_eq!(
        audio_refusal(&link, None, &operators(), None, &ids(&[1])),
        Err("forbidden")
    );
    // Nobody is an operator: nothing owned by an account is allowed either.
    assert_eq!(
        audio_refusal(&link, Some(&doc), &HashSet::new(), None, &ids(&[1])),
        Err("forbidden")
    );
    assert_eq!(ok(&[]), Ok(()));
}

#[test]
fn audio_needs_the_audio_feature_and_plugin_audio_its_own() {
    let plugin = superhet_bus::audio::PLUGIN_VFO_BASE;
    let doc = state_doc();
    let mut link = test_link(1);
    link.features = features(&[Feature::Waterfall]);
    assert_eq!(
        audio_refusal(&link, Some(&doc), &operators(), None, &ids(&[1])),
        Err("forbidden")
    );
    assert_eq!(
        audio_refusal(&link, Some(&doc), &operators(), None, &ids(&[plugin])),
        Err("forbidden")
    );
    link.features = features(&[Feature::PluginAudio]);
    assert_eq!(
        audio_refusal(&link, Some(&doc), &operators(), None, &ids(&[plugin])),
        Ok(())
    );
    assert_eq!(
        audio_refusal(&link, Some(&doc), &operators(), None, &ids(&[1])),
        Err("forbidden"),
        "plugin audio does not unlock VFOs"
    );
    link.features = features(&[Feature::Audio]);
    assert_eq!(
        audio_refusal(&link, Some(&doc), &operators(), None, &ids(&[plugin])),
        Err("forbidden"),
        "audio does not unlock plugin audio"
    );
}

#[test]
fn a_fixed_link_plays_only_the_listeners_own_vfo() {
    let mut link = test_link(1);
    link.tuning = Tuning::Fixed {
        freq_hz: 100_000_000,
        mode: "wfm".into(),
        range: None,
    };
    let doc = state_doc();
    let run = |own: Option<u32>, wanted: &[u32]| {
        audio_refusal(&link, Some(&doc), &operators(), own, &ids(wanted))
    };
    assert_eq!(run(Some(5), &[5]), Ok(()));
    assert_eq!(
        run(Some(5), &[1]),
        Err("forbidden"),
        "the operator's VFO stays private"
    );
    assert_eq!(run(Some(5), &[4]), Err("forbidden"));
    assert_eq!(
        run(None, &[5]),
        Err("forbidden"),
        "no VFO of its own, no audio"
    );
}

#[test]
fn the_public_state_leaks_neither_owners_nor_other_listeners() {
    let link = test_link(1);
    let out = public_state(&link, &state_doc(), &operators());
    let text = out.to_string();
    for secret in ["owner", "locks", "gain", "g:abcdef", "u:1", "s:9:1"] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    let vfos: Vec<u64> = out["vfos"]
        .as_array()
        .expect("vfos")
        .iter()
        .filter_map(|v| v["id"].as_u64())
        .collect();
    assert_eq!(vfos, vec![1, 4]);
    assert_eq!(out["center_hz"], 100_000_000);

    let mut waterfall_only = test_link(1);
    waterfall_only.features = features(&[Feature::Waterfall]);
    assert_eq!(
        public_state(&waterfall_only, &state_doc(), &operators())["vfos"],
        json!([])
    );
    let mut fixed = test_link(1);
    fixed.tuning = Tuning::Fixed {
        freq_hz: 1,
        mode: "am".into(),
        range: None,
    };
    assert_eq!(
        public_state(&fixed, &state_doc(), &operators())["vfos"],
        json!([])
    );
}
