use super::*;
use std::fs;
use superhet_db::users::Role;
use superhet_satpass::parse_elements;
use tower::ServiceExt;

const L1: &str = "1 59051U 24039A   26327.50722175 -.00000013  00000+0  13685-4 0  9998";
const L2: &str = "2 59051  98.7256 284.9730 0008115  71.3466 288.8592 14.22444296141910";
const FROM_MS: i64 = 1_795_435_200_000; // 2026-11-23T12:00:00Z

fn scratch(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("superhet-weather-api-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn state(dir: &std::path::Path, db: Option<PgPool>) -> WeatherState {
    WeatherState {
        devices: DevicesState::for_tests(),
        db,
        dir: dir.to_owned(),
        max_bytes: 0,
        url: String::new(),
    }
}

fn app(state: WeatherState, role: Role) -> Router {
    router(state).layer(Extension(Principal::test(role)))
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: &str,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .expect("request");
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, headers, bytes.to_vec())
}

async fn call_json(app: &Router, method: &str, path: &str, body: &str) -> (StatusCode, Value) {
    let (status, _, bytes) = call(app, method, path, body).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A capture the way the plugin writes it (without a device, so no policy lookup is needed).
fn capture(dir: &std::path::Path, id: &str, started_s: i64, satellite: &str) {
    let d = dir.join(id);
    fs::create_dir_all(&d).expect("mkdir");
    fs::write(d.join("rgb.png"), b"\x89PNG full image").expect("image");
    fs::write(d.join("thumb-rgb.png"), b"\x89PNG thumb").expect("thumb");
    let doc = json!({"v": 1, "id": id, "satellite": satellite, "pipeline": "lrpt",
        "started_ms": started_s * 1000, "ended_ms": (started_s + 600) * 1000, "duration_s": 600,
        "frequency_hz": 137_900_000, "device": null, "peak_db": 7.5, "max_elevation_deg": null,
        "keep": false,
        "images": [{"name": "rgb.png", "label": "rgb", "width": 10, "height": 10, "size": 14, "thumb": "thumb-rgb.png"}]});
    fs::write(d.join("pass.json"), doc.to_string()).expect("doc");
}

#[tokio::test]
async fn the_gallery_lists_shows_serves_keeps_and_deletes() {
    let dir = scratch("gallery");
    capture(&dir, "1700000100-0001", 1_700_000_100, "METEOR-M2-4");
    capture(&dir, "1700000900-0002", 1_700_000_900, "METEOR-M2-3");
    let app = app(state(&dir, None), Role::Operator);

    let (status, doc) = call_json(&app, "GET", "/api/v1/weather/captures", "").await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = doc["captures"]
        .as_array()
        .expect("list")
        .iter()
        .map(|c| c["id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, ["1700000900-0002", "1700000100-0001"], "newest first");
    assert_eq!(
        doc["captures"][0]["norad_id"], 57166,
        "satellite name matched to the catalogue"
    );
    assert_eq!(doc["captures"][0]["images"][0]["thumb"], "thumb-rgb.png");
    assert!(doc["next_before_ms"].is_null());

    // Paging.
    let (_, page) = call_json(&app, "GET", "/api/v1/weather/captures?limit=1", "").await;
    assert_eq!(page["captures"].as_array().expect("list").len(), 1);
    assert_eq!(page["next_before_ms"], 1_700_000_900_000_i64);
    let (_, rest) = call_json(
        &app,
        "GET",
        "/api/v1/weather/captures?limit=1&before_ms=1700000900000",
        "",
    )
    .await;
    assert_eq!(rest["captures"][0]["id"], "1700000100-0001");

    let (status, one) =
        call_json(&app, "GET", "/api/v1/weather/captures/1700000100-0001", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["satellite"], "METEOR-M2-4");

    // Images: full, thumbnail, download.
    let (status, headers, bytes) = call(
        &app,
        "GET",
        "/api/v1/weather/captures/1700000100-0001/images/rgb.png",
        "",
    )
    .await;
    assert_eq!(
        (status, bytes.as_slice()),
        (StatusCode::OK, b"\x89PNG full image".as_slice())
    );
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    let (_, _, bytes) = call(
        &app,
        "GET",
        "/api/v1/weather/captures/1700000100-0001/images/rgb.png?thumb=true",
        "",
    )
    .await;
    assert_eq!(bytes, b"\x89PNG thumb");
    let (_, headers, _) = call(
        &app,
        "GET",
        "/api/v1/weather/captures/1700000100-0001/images/rgb.png?download=true",
        "",
    )
    .await;
    assert!(
        headers["content-disposition"]
            .to_str()
            .expect("str")
            .contains("1700000100-0001-rgb.png")
    );

    // Only what the pass lists is served; no way out of the directory.
    for bad in [
        "pass.json",
        "other.png",
        "..%2Fpass.json",
        "%2e%2e%2f1700000900-0002%2Frgb.png",
        ".hidden.png",
    ] {
        let path = format!("/api/v1/weather/captures/1700000100-0001/images/{bad}");
        let (status, _, _) = call(&app, "GET", &path, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{bad}");
    }
    for bad_id in ["..", "1700000100-0001x", "nonsense", "1700000111-0000"] {
        let (status, _, _) = call(
            &app,
            "GET",
            &format!("/api/v1/weather/captures/{bad_id}"),
            "",
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{bad_id}");
    }

    // Keep, then delete.
    let (status, kept) = call_json(
        &app,
        "PATCH",
        "/api/v1/weather/captures/1700000100-0001",
        r#"{"keep":true}"#,
    )
    .await;
    assert_eq!(
        (status, kept["keep"].as_bool()),
        (StatusCode::OK, Some(true))
    );
    for bad in ["{}", r#"{"keep":"yes"}"#, r#"{"keep":true,"x":1}"#, "nope"] {
        let (status, _) = call_json(
            &app,
            "PATCH",
            "/api/v1/weather/captures/1700000100-0001",
            bad,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (status, _) = call_json(
        &app,
        "DELETE",
        "/api/v1/weather/captures/1700000100-0001",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!dir.join("1700000100-0001").exists());
    let (status, _) = call_json(
        &app,
        "DELETE",
        "/api/v1/weather/captures/1700000100-0001",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_foreign_origin_cannot_change_the_gallery() {
    let dir = scratch("origin");
    capture(&dir, "1700000100-0001", 1_700_000_100, "METEOR-M2-4");
    let app = app(state(&dir, None), Role::Operator);
    let request = Request::builder()
        .method("DELETE")
        .uri("/api/v1/weather/captures/1700000100-0001")
        .header("origin", "https://evil.example")
        .header("host", "radio.example")
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(dir.join("1700000100-0001").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn predictions_validate_their_input_and_need_a_database() {
    let dir = scratch("predict");
    let app = app(state(&dir, None), Role::Listener);
    for (body, what) in [
        ("nope", "not json"),
        (r#"{"hours":0}"#, "hours"),
        (r#"{"hours":500}"#, "hours"),
        (r#"{"min_elevation_deg":120}"#, "elevation"),
        (r#"{"x":1}"#, "unknown field"),
        (r#"{"lat":51.0}"#, "one coordinate"),
        (r#"{"lat":95.0,"lon":7.0}"#, "latitude"),
    ] {
        let (status, doc) = call_json(&app, "POST", "/api/v1/weather/passes", body).await;
        // The position is only judged once the database (and so the element sets) is there.
        if what == "one coordinate" || what == "latitude" {
            assert!(
                status == StatusCode::BAD_REQUEST || status == StatusCode::SERVICE_UNAVAILABLE,
                "{what}: {status}"
            );
        } else {
            assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {doc}");
        }
    }
    let (status, doc) = call_json(&app, "POST", "/api/v1/weather/passes", "{}").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{doc}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn upcoming_passes_come_sorted_with_downlinks_and_only_for_operational_satellites() {
    let sets = parse_elements(&format!("Meteor\n{L1}\n{L2}\n")).expect("elements");
    let observer = Observer::new(51.0, 7.0).expect("observer");
    let passes = upcoming(&sets, observer, FROM_MS, 24, 10.0);
    // Four passes of at least 10 degrees in the window (cross-checked in superhet-satpass).
    assert_eq!(passes.len(), 4 + 1, "{passes:#?}");
    let aos: Vec<i64> = passes.iter().filter_map(|p| p["aos_ms"].as_i64()).collect();
    assert!(aos.windows(2).all(|w| w[0] <= w[1]));
    let first = &passes[0];
    assert_eq!(first["satellite"], "METEOR-M2 4");
    assert_eq!(first["downlinks"][0]["hz"], 137_900_000);
    assert!(first["max_elevation_deg"].as_f64().expect("el") > 27.0);
    assert!(first["elements_age_h"].as_f64().expect("age").abs() < 12.0);
    // A set of a satellite that is switched off yields nothing (NOAA 19).
    let mut noaa = sets[0].clone();
    noaa.norad_id = 33591;
    assert!(upcoming(&[noaa], observer, FROM_MS, 24, 0.0).is_empty());
    assert!(upcoming(&[], observer, FROM_MS, 24, 0.0).is_empty());
}

#[test]
fn the_maximum_elevation_of_a_capture_comes_from_the_matching_pass() {
    let sets = parse_elements(&format!("Meteor\n{L1}\n{L2}\n")).expect("elements");
    let observer = Observer::new(51.0, 7.0).expect("observer");
    let dir = scratch("elevation");
    // The second pass of the reference data: 2026-11-23T13:38:19Z to 13:46:53Z, 56.4 degrees.
    capture(&dir, "1795442299-0001", 1_795_442_299, "METEOR-M2-4");
    let c = store::read(&dir, "1795442299-0001").expect("capture");
    assert_eq!(elevation_of(&c, &sets, observer), Some(Some(56.4)));
    // A recording at a time without a pass over this place.
    capture(&dir, "1795445000-0002", 1_795_445_000, "METEOR-M2-4");
    let c = store::read(&dir, "1795445000-0002").expect("capture");
    assert_eq!(elevation_of(&c, &sets, observer), Some(None));
    // No element set for the satellite, or a satellite we do not know: cannot say.
    capture(&dir, "1795442299-0003", 1_795_442_299, "METEOR-M2-3");
    let c = store::read(&dir, "1795442299-0003").expect("capture");
    assert_eq!(elevation_of(&c, &sets, observer), None);
    capture(&dir, "1795442299-0004", 1_795_442_299, "Unknown Meteor");
    let c = store::read(&dir, "1795442299-0004").expect("capture");
    assert_eq!(elevation_of(&c, &sets, observer), None);
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn config_reports_satellite_status_and_the_store() {
    let dir = scratch("config");
    capture(&dir, "1700000100-0001", 1_700_000_100, "METEOR-M2-4");
    let app = app(state(&dir, None), Role::Listener);
    let (status, doc) = call_json(&app, "GET", "/api/v1/weather/config", "").await;
    assert_eq!(status, StatusCode::OK);
    let sats = doc["satellites"].as_array().expect("satellites");
    let status_of = |name: &str| {
        sats.iter()
            .find(|s| s["name"] == name)
            .map(|s| s["status"].clone())
    };
    assert_eq!(status_of("METEOR-M2 4"), Some(json!("operational")));
    assert_eq!(status_of("NOAA 19"), Some(json!("decommissioned")));
    assert_eq!(doc["store"]["captures"], 1);
    assert_eq!(doc["store"]["available"], true);
    assert_eq!(doc["elements"]["enabled"], false);
    assert_eq!(doc["elements"]["download"]["name"], "CelesTrak");
    let _ = fs::remove_dir_all(&dir);
}

/// The two database tests share one settings key; they take turns.
static DB_TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The element-set routes need the settings table: they run when `DATABASE_URL` is set.
async fn pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = superhet_db::connect(&url).await.ok()?;
    superhet_db::migrate(&pool).await.ok()?;
    sqlx_clear(&pool).await;
    Some(pool)
}

async fn sqlx_clear(pool: &PgPool) {
    // The key belongs to this feature; start each run from nothing.
    let _ = superhet_db::settings::put(pool, elements::SETTINGS_KEY, &json!({}), "test").await;
}

/// A tiny HTTP server that answers every request with `body` and counts the requests, noting
/// the User-Agent of the last one.
async fn stub_server(body: String) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 8192];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            log.lock().expect("log").push(request);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
        }
    });
    (
        format!("http://127.0.0.1:{port}/gp.php?GROUP=weather&FORMAT=json"),
        seen,
    )
}

fn omm(name: &str, norad: u64) -> Value {
    let mut set =
        serde_json::to_value(&parse_elements(&format!("{L1}\n{L2}\n")).expect("parse")[0])
            .expect("json");
    set["OBJECT_NAME"] = json!(name);
    set["NORAD_CAT_ID"] = json!(norad);
    set
}

#[tokio::test]
async fn the_download_is_opt_in_polite_and_limited_to_the_catalogue() {
    let _turn = DB_TURN.lock().await;
    let Some(pool) = pool().await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let dir = scratch("elements");
    let body = json!([omm("METEOR-M2 4", 59051), omm("ISS (ZARYA)", 25544)]).to_string();
    let (url, seen) = stub_server(body).await;
    let mut st = state(&dir, Some(pool.clone()));
    st.url = url;
    let app = app(st, Role::Admin);

    // Nothing is fetched until the admin allows it.
    let (_, doc) = call_json(&app, "GET", "/api/v1/weather/config", "").await;
    assert_eq!(doc["elements"]["enabled"], false);
    assert!(seen.lock().expect("log").is_empty());
    let (status, doc) = call_json(&app, "POST", "/api/v1/admin/weather/elements/refresh", "").await;
    assert_eq!(
        (status, doc["code"].as_str()),
        (StatusCode::CONFLICT, Some("download_off"))
    );

    // Allowing it fetches once, with the plain User-Agent and no position anywhere.
    let (status, doc) = call_json(
        &app,
        "PUT",
        "/api/v1/admin/weather/elements",
        r#"{"enabled":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    assert_eq!(doc["elements"]["source"], "download");
    assert_eq!(
        doc["elements"]["satellites"]
            .as_array()
            .expect("sats")
            .len(),
        1,
        "the ISS is dropped"
    );
    {
        let requests = seen.lock().expect("log");
        assert_eq!(requests.len(), 1);
        let request = requests[0].to_lowercase();
        assert!(
            request.contains(&format!(
                "user-agent: superhet/{}\r\n",
                env!("CARGO_PKG_VERSION")
            )),
            "{request}"
        );
        assert!(
            request.starts_with("get /gp.php?group=weather&format=json "),
            "{request}"
        );
        for leak in ["lat", "lon", "cookie", "authorization", "x-forwarded"] {
            assert!(
                !request.contains(&format!("\r\n{leak}")),
                "{leak} in {request}"
            );
        }
    }
    // A second download inside two hours is refused.
    let (status, doc) = call_json(&app, "POST", "/api/v1/admin/weather/elements/refresh", "").await;
    assert_eq!(
        (status, doc["code"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("too_soon"))
    );
    assert_eq!(seen.lock().expect("log").len(), 1);

    // The passes now work, from a position in the request.
    let (status, doc) = call_json(
        &app,
        "POST",
        "/api/v1/weather/passes",
        r#"{"lat":51.0,"lon":7.0,"hours":24,"min_elevation_deg":0}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    assert_eq!(doc["position_from"], "request");
    assert!(
        doc["passes"]
            .as_array()
            .expect("passes")
            .iter()
            .all(|p| p["satellite"] == "METEOR-M2 4")
    );
    // No position at all is a 409 with a reason, not a guess.
    let (status, doc) = call_json(&app, "POST", "/api/v1/weather/passes", "{}").await;
    assert_eq!(
        (status, doc["code"].as_str()),
        (StatusCode::CONFLICT, Some("no_position"))
    );

    // Switching off keeps the sets but stops downloads.
    let (_, doc) = call_json(
        &app,
        "PUT",
        "/api/v1/admin/weather/elements",
        r#"{"enabled":false}"#,
    )
    .await;
    assert_eq!(doc["elements"]["enabled"], false);
    assert_eq!(
        doc["elements"]["satellites"]
            .as_array()
            .expect("sats")
            .len(),
        1
    );
    let stored = Doc::from_value(
        superhet_db::settings::get(&pool, elements::SETTINGS_KEY)
            .await
            .expect("get"),
    );
    assert!(!stored.due(i64::MAX / 2));

    // Pasting works without the download; junk is explained.
    let (status, doc) = call_json(
        &app,
        "POST",
        "/api/v1/admin/weather/elements/manual",
        &json!({"text": format!("{L1}\n{L2}\n")}).to_string(),
    )
    .await;
    assert_eq!(
        (status, doc["elements"]["source"].as_str()),
        (StatusCode::OK, Some("manual"))
    );
    let (status, doc) = call_json(
        &app,
        "POST",
        "/api/v1/admin/weather/elements/manual",
        r#"{"text":"hello"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{doc}");
    let (status, _) = call_json(
        &app,
        "POST",
        "/api/v1/admin/weather/elements/manual",
        r#"{"text":"x","y":1}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    sqlx_clear(&pool).await;
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_failed_download_is_reported_and_remembered() {
    let _turn = DB_TURN.lock().await;
    let Some(pool) = pool().await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let dir = scratch("elements-fail");
    let (url, _seen) = stub_server("this is not element data".into()).await;
    let mut st = state(&dir, Some(pool.clone()));
    st.url = url;
    let app = app(st, Role::Admin);
    let (status, doc) = call_json(
        &app,
        "PUT",
        "/api/v1/admin/weather/elements",
        r#"{"enabled":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(doc["elements"]["error"].as_str().is_some(), "{doc}");
    assert_eq!(doc["elements"]["source"], Value::Null);
    let (status, doc) = call_json(&app, "POST", "/api/v1/admin/weather/elements/refresh", "").await;
    assert_eq!(
        (status, doc["code"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("too_soon")),
        "a failure counts as an attempt"
    );
    let (status, doc) = call_json(
        &app,
        "POST",
        "/api/v1/weather/passes",
        r#"{"lat":51.0,"lon":7.0}"#,
    )
    .await;
    assert_eq!(
        (status, doc["code"].as_str()),
        (StatusCode::CONFLICT, Some("no_elements"))
    );
    sqlx_clear(&pool).await;
    let _ = fs::remove_dir_all(&dir);
}
