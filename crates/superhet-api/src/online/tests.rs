//! The online lookups against a local stub of both services; the real ones are never contacted.

use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    Router,
    http::{HeaderMap, StatusCode, header},
    routing::get,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;

const BOTH: OnlineSettings = OnlineSettings {
    enabled: true,
    adsbdb: true,
    planespotters: true,
};

/// What the stub saw.
#[derive(Default)]
struct Seen {
    requests: AtomicUsize,
    agents: Mutex<Vec<String>>,
    paths: Mutex<Vec<String>>,
}

fn note(seen: &Seen, headers: &HeaderMap, path: &str) {
    seen.requests.fetch_add(1, Ordering::SeqCst);
    let agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    seen.agents
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(agent);
    seen.paths
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(path.to_owned());
}

const AIRCRAFT: &str = r#"{"response":{"aircraft":{"type":"A350 941","icao_type":"A359",
"manufacturer":"Airbus","mode_s":"3C6710","registration":"D-AIXP",
"registered_owner_country_iso_name":"DE","registered_owner_country_name":"Germany",
"registered_owner_operator_flag_code":"DLH","registered_owner":"Lufthansa",
"url_photo":"https://image.airport-data.com/aircraft/001653085.jpg",
"url_photo_thumbnail":"https://airport-data.com/images/aircraft/thumbnails/001/653/001653085.jpg"}}}"#;

const ROUTE: &str = r#"{"response":{"flightroute":{"callsign":"DLH400","callsign_icao":"DLH400",
"callsign_iata":"LH400","airline":{"name":"Lufthansa","icao":"DLH","iata":"LH",
"country":"Germany","country_iso":"DE","callsign":"LUFTHANSA"},
"origin":{"country_iso_name":"DE","country_name":"Germany","elevation":364,"iata_code":"FRA",
"icao_code":"EDDF","latitude":50.03333,"longitude":8.57056,"municipality":"Frankfurt am Main",
"name":"Frankfurt am Main Airport"},
"destination":{"country_iso_name":"US","country_name":"United States","elevation":13,
"iata_code":"JFK","icao_code":"KJFK","latitude":40.6398,"longitude":-73.7789,
"municipality":"New York","name":"John F Kennedy International Airport"}}}}"#;

const PHOTO: &str = r#"{"photos":[{"id":"1053982","thumbnail":{"src":"https://t.plnspttrs.net/05121/1053982_2810c547e6_t.jpg","size":{"width":200,"height":142}},
"thumbnail_large":{"src":"https://t.plnspttrs.net/05121/1053982_2810c547e6_280.jpg","size":{"width":395,"height":280}},
"link":"https://www.planespotters.net/photo/1053982/d-abcd-dc-aviation-bombardier-challenger-604-cl-600-2b16?utm_source=api",
"photographer":"Günther Feniuk"}]}"#;

/// A stub of adsbdb (`/adsbdb/..`) and the photo service (`/photos/..`) on a free local port.
async fn stub(seen: Arc<Seen>) -> (String, tokio::task::JoinHandle<()>) {
    let json_ok = |body: &'static str| {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            body,
        )
    };
    let s1 = seen.clone();
    let s2 = seen.clone();
    let s3 = seen.clone();
    let s4 = seen;
    let app = Router::new()
        .route(
            "/adsbdb/aircraft/{hex}",
            get(
                move |axum::extract::Path(hex): axum::extract::Path<String>, h: HeaderMap| async move {
                    note(&s1, &h, &format!("aircraft/{hex}"));
                    match hex.as_str() {
                        "3C6710" => json_ok(AIRCRAFT).into_response(),
                        "AAAAAA" => StatusCode::TOO_MANY_REQUESTS.into_response(),
                        "BBBBBB" => (
                            StatusCode::FOUND,
                            [(header::LOCATION, "http://127.0.0.1:1/elsewhere")],
                        )
                            .into_response(),
                        "CCCCCC" => json_ok(Box::leak("x".repeat(300_000).into_boxed_str()))
                            .into_response(),
                        _ => StatusCode::NOT_FOUND.into_response(),
                    }
                },
            ),
        )
        .route(
            "/adsbdb/callsign/{cs}",
            get(
                move |axum::extract::Path(cs): axum::extract::Path<String>, h: HeaderMap| async move {
                    note(&s2, &h, &format!("callsign/{cs}"));
                    if cs == "DLH400" {
                        json_ok(ROUTE).into_response()
                    } else {
                        StatusCode::NOT_FOUND.into_response()
                    }
                },
            ),
        )
        .route(
            "/photos/hex/{hex}",
            get(
                move |axum::extract::Path(hex): axum::extract::Path<String>, h: HeaderMap| async move {
                    note(&s3, &h, &format!("photo/{hex}"));
                    match hex.as_str() {
                        "3C6710" => json_ok(PHOTO).into_response(),
                        "DDDDDD" => json_ok(
                            r#"{"photos":[{"thumbnail_large":{"src":"https://evil.example/x.jpg"},
                            "link":"https://www.planespotters.net/photo/1","photographer":"X"}]}"#,
                        )
                        .into_response(),
                        "EEEEEE" => json_ok(r#"{"photos":[]}"#).into_response(),
                        _ => StatusCode::NOT_FOUND.into_response(),
                    }
                },
            ),
        )
        .route("/count", get(move || async move { s4.requests.load(Ordering::SeqCst).to_string() }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://127.0.0.1:{port}"), server)
}

use axum::response::IntoResponse;

async fn service(seen: &Arc<Seen>) -> (Service, tokio::task::JoinHandle<()>) {
    let (base, server) = stub(seen.clone()).await;
    (
        Service::with_bases(&format!("{base}/adsbdb"), &format!("{base}/photos")),
        server,
    )
}

fn requests(seen: &Seen) -> usize {
    seen.requests.load(Ordering::SeqCst)
}

#[test]
fn the_override_locks_the_sources() {
    let stored = OnlineSettings {
        enabled: true,
        adsbdb: true,
        planespotters: false,
    };
    assert_eq!(effective_with(stored, None), (stored, false));
    assert_eq!(effective_with(stored, Some("  ")), (stored, false));
    assert_eq!(
        effective_with(stored, Some("off")),
        (OnlineSettings::default(), true)
    );
    assert_eq!(effective_with(stored, Some("all")), (BOTH, true));
    let photo_only = effective_with(OnlineSettings::default(), Some("Planespotters")).0;
    assert!(photo_only.planespotters_on() && !photo_only.adsbdb_on());
    // Words nobody knows switch nothing on.
    assert_eq!(
        effective_with(BOTH, Some("everything")).0,
        OnlineSettings::default()
    );
    assert_eq!(BOTH.image_origins().len(), 2);
    assert!(photo_only.image_origins()[0].starts_with("https://"));
    assert!(stored.image_origins().is_empty());
    assert!(OnlineSettings::default().image_origins().is_empty());
}

#[tokio::test]
async fn nothing_is_asked_while_the_sources_are_off() {
    let seen = Arc::new(Seen::default());
    let (svc, server) = service(&seen).await;
    let off = OnlineSettings {
        enabled: false,
        adsbdb: true,
        planespotters: true,
    };
    let body = svc.lookup(&off, "3c6710", Some("DLH400")).await;
    assert_eq!(body["enabled"], false);
    assert_eq!(body["sources"]["adsbdb"], "off");
    assert_eq!(body["sources"]["planespotters"], "off");
    assert!(body["aircraft"].is_null() && body["route"].is_null() && body["photo"].is_null());
    assert_eq!(requests(&seen), 0);

    // One source on: only that one is asked.
    let photo_only = OnlineSettings {
        enabled: true,
        adsbdb: false,
        planespotters: true,
    };
    let body = svc.lookup(&photo_only, "3c6710", Some("DLH400")).await;
    assert_eq!(body["sources"]["adsbdb"], "off");
    assert_eq!(body["sources"]["planespotters"], "ok");
    assert_eq!(requests(&seen), 1);
    server.abort();
}

#[tokio::test]
async fn a_lookup_returns_aircraft_route_and_photo_and_is_cached() {
    let seen = Arc::new(Seen::default());
    let (svc, server) = service(&seen).await;
    let body = svc.lookup(&BOTH, "3c6710", Some("DLH400")).await;
    assert_eq!(body["sources"]["adsbdb"], "ok");
    assert_eq!(body["sources"]["planespotters"], "ok");
    assert_eq!(body["aircraft"]["registration"], "D-AIXP");
    assert_eq!(body["aircraft"]["type"], "A350 941");
    assert_eq!(body["aircraft"]["owner"], "Lufthansa");
    assert_eq!(body["aircraft"]["owner_country_iso"], "DE");
    // adsbdb's own photo URLs (a third service) are not passed on.
    assert!(!body.to_string().contains("airport-data"));
    assert_eq!(body["route"]["origin"]["iata"], "FRA");
    assert_eq!(body["route"]["destination"]["city"], "New York");
    assert_eq!(body["route"]["destination"]["lat"], 40.6398);
    assert_eq!(body["route"]["airline"]["name"], "Lufthansa");
    assert_eq!(
        body["photo"]["src"],
        "https://t.plnspttrs.net/05121/1053982_2810c547e6_280.jpg"
    );
    // The photo page link is passed on unchanged, the credit comes along.
    assert!(
        body["photo"]["link"]
            .as_str()
            .is_some_and(|l| l.ends_with("?utm_source=api"))
    );
    assert_eq!(body["photo"]["photographer"], "G\u{fc}nther Feniuk");
    assert_eq!(
        (
            body["photo"]["width"].as_u64(),
            body["photo"]["height"].as_u64()
        ),
        (Some(395), Some(280))
    );
    assert_eq!(requests(&seen), 3);

    // The second lookup is answered from memory.
    let again = svc.lookup(&BOTH, "3c6710", Some("DLH400")).await;
    assert_eq!(again, body);
    assert_eq!(requests(&seen), 3);

    // Only the address and the callsign are in the requests, with the honest agent.
    let paths = seen
        .paths
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for p in &paths {
        assert!(
            ["aircraft/3C6710", "callsign/DLH400", "photo/3C6710"].contains(&p.as_str()),
            "{p}"
        );
    }
    for agent in seen
        .agents
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
    {
        assert!(agent.starts_with("Superhet/"), "{agent}");
        assert!(
            agent.contains("(+https://github.com/BxnnyG/superhet)"),
            "{agent}"
        );
    }
    server.abort();
}

#[tokio::test]
async fn unknown_things_are_not_found_and_remembered() {
    let seen = Arc::new(Seen::default());
    let (svc, server) = service(&seen).await;
    let body = svc.lookup(&BOTH, "123456", Some("ABC123")).await;
    assert_eq!(body["sources"]["adsbdb"], "not_found");
    assert_eq!(body["sources"]["planespotters"], "not_found");
    assert!(body["aircraft"].is_null());
    let before = requests(&seen);
    let _ = svc.lookup(&BOTH, "123456", Some("ABC123")).await;
    assert_eq!(requests(&seen), before, "a miss is not asked again at once");

    // No photos in the answer: nothing found.
    let body = svc.lookup(&BOTH, "eeeeee", None).await;
    assert_eq!(body["sources"]["planespotters"], "not_found");
    // Without a callsign there is no route to ask for.
    assert!(body["route"].is_null());
    server.abort();
}

#[tokio::test]
async fn a_photo_from_an_unknown_host_is_dropped() {
    let seen = Arc::new(Seen::default());
    let (svc, server) = service(&seen).await;
    let body = svc.lookup(&BOTH, "dddddd", None).await;
    assert!(body["photo"].is_null());
    assert_eq!(body["sources"]["planespotters"], "not_found");
    server.abort();
}

#[tokio::test]
async fn a_refusal_pauses_the_service_and_redirects_are_not_followed() {
    let seen = Arc::new(Seen::default());
    let (svc, server) = service(&seen).await;
    let body = svc.lookup(&BOTH, "aaaaaa", None).await;
    assert_eq!(body["sources"]["adsbdb"], "rate_limited");
    let before = requests(&seen);
    // The service is left alone for a while: no request at all.
    let body = svc.lookup(&BOTH, "3c6710", Some("DLH400")).await;
    assert_eq!(body["sources"]["adsbdb"], "rate_limited");
    assert_eq!(
        requests(&seen) - before,
        1,
        "only the photo service answered"
    );
    server.abort();

    let seen = Arc::new(Seen::default());
    let (svc, server) = service(&seen).await;
    let body = svc.lookup(&BOTH, "bbbbbb", None).await;
    assert_eq!(body["sources"]["adsbdb"], "unavailable");
    server.abort();

    let seen = Arc::new(Seen::default());
    let (svc, server) = service(&seen).await;
    let body = svc.lookup(&BOTH, "cccccc", None).await;
    assert_eq!(body["sources"]["adsbdb"], "unavailable", "oversized answer");
    server.abort();
}

#[tokio::test]
async fn an_unreachable_service_is_reported_not_hidden() {
    // Nothing listens on port 1.
    let svc = Service::with_bases("http://127.0.0.1:1/a", "http://127.0.0.1:1/p");
    let body = svc.lookup(&BOTH, "3c6710", Some("DLH400")).await;
    assert_eq!(body["sources"]["adsbdb"], "unavailable");
    assert_eq!(body["sources"]["planespotters"], "unavailable");
}

#[test]
fn the_request_budget_runs_out() {
    let gate = Gate::new();
    for _ in 0..PER_MINUTE {
        assert!(gate.admit());
    }
    assert!(!gate.admit());
    gate.state().window_start = Instant::now() - Duration::from_secs(61);
    assert!(gate.admit());
}

#[test]
fn urls_are_checked_before_they_are_passed_on() {
    assert!(url_on("https://t.plnspttrs.net/a.jpg", &PHOTO_HOSTS));
    assert!(url_on("https://cdn.planespotters.net/a.jpg", &PHOTO_HOSTS));
    for bad in [
        "http://t.plnspttrs.net/a.jpg",
        "https://t.plnspttrs.net.evil.example/a.jpg",
        "https://evil.example/t.plnspttrs.net/a.jpg",
        "https://user@t.plnspttrs.net/a.jpg",
        "https://t.plnspttrs.net:8443/a.jpg",
        "//t.plnspttrs.net/a.jpg",
        "javascript:alert(1)",
        "",
    ] {
        assert!(!url_on(bad, &PHOTO_HOSTS), "{bad}");
    }
    assert!(valid_callsign("DLH400") && valid_callsign("N1"));
    for bad in ["", "A", "TOOLONGCS", "DL H1", "../x", "DLH4/0"] {
        assert!(!valid_callsign(bad), "{bad}");
    }
}

async fn call(app: Router, uri: &str) -> (StatusCode, Value) {
    let res = app
        .oneshot(
            axum::http::Request::get(uri)
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = res.status();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn the_route_validates_input_and_answers_off_by_default() {
    let seen = Arc::new(Seen::default());
    let (svc, server) = service(&seen).await;
    let app = router(OnlineState::with_service(
        DevicesState::for_tests(),
        Arc::new(svc),
    ));
    let (status, _) = call(app.clone(), "/api/v1/adsb/online?icao=xyz").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(app.clone(), "/api/v1/adsb/online?icao=3c6710&callsign=a/b").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // No database in this state: every switch is off, so nothing is sent anywhere.
    let (status, body) = call(app, "/api/v1/adsb/online?icao=3c6710&callsign=DLH400").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], false);
    assert_eq!(body["locked"], false);
    assert_eq!(requests(&seen), 0);
    server.abort();
}
