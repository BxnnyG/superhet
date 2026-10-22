//! Bookmarks (memory channels) over REST: `/api/v1/bookmarks[/{id}]` and
//! `POST /api/v1/bookmarks/bulk`. Contract: `docs/api-bookmarks.md`.
//!
//! Bookmarks live in PostgreSQL. The API validates and normalises every value (trimmed name,
//! lower-case colour, de-duplicated tags) before it reaches the database.

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use serde::{Deserialize, Deserializer, de::IgnoredAny};
use superhet_db::{
    PgPool,
    bookmarks::{self, Bookmark, MAX_BOOKMARKS, NewBookmark},
    profiles::parse_id,
};

use crate::devices::{DevicesState, failure, guard_mutation, json};

/// Largest accepted body of a single-bookmark request.
const MAX_BODY_BYTES: usize = 16 * 1024;
/// Largest accepted body of a bulk request.
const MAX_BULK_BYTES: usize = 2 * 1024 * 1024;

const MAX_NAME_CHARS: usize = 100;
const MAX_TAGS: usize = 16;
const MAX_TAG_CHARS: usize = 32;
const MAX_NOTES_CHARS: usize = 1000;
const MAX_FREQ_HZ: i64 = 10_000_000_000;
const MAX_BANDWIDTH_HZ: i32 = 10_000_000;

/// Shared state of the bookmark routes.
#[derive(Clone)]
pub struct BookmarksState {
    devices: DevicesState,
    db: Option<PgPool>,
}

impl BookmarksState {
    /// Create the state from the device routes' state (origin policy) and the database.
    #[must_use]
    pub fn new(devices: DevicesState, db: Option<PgPool>) -> Self {
        Self { devices, db }
    }
}

/// Routes of the bookmark API, behind the authentication hook.
pub fn router(state: BookmarksState) -> Router {
    let limit = DefaultBodyLimit::max(MAX_BODY_BYTES);
    Router::new()
        .route(
            "/api/v1/bookmarks",
            get(list_bookmarks).post(create_bookmark).layer(limit),
        )
        .route(
            "/api/v1/bookmarks/bulk",
            post(bulk_create).layer(DefaultBodyLimit::max(MAX_BULK_BYTES)),
        )
        .route(
            "/api/v1/bookmarks/{id}",
            axum::routing::patch(update_bookmark)
                .delete(delete_bookmark)
                .layer(limit),
        )
        .with_state(state)
}

fn pool(state: &BookmarksState) -> Result<&PgPool, Box<Response>> {
    state.db.as_ref().ok_or_else(|| {
        Box::new(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "no database",
        ))
    })
}

fn internal(err: &dyn std::fmt::Display) -> Box<Response> {
    tracing::error!(%err, "bookmark database error");
    Box::new(failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "database error",
    ))
}

fn write_failure(err: bookmarks::WriteError) -> Box<Response> {
    match err {
        bookmarks::WriteError::Limit => Box::new(failure(
            StatusCode::CONFLICT,
            "limit",
            "at most 10000 bookmarks can be stored",
        )),
        bookmarks::WriteError::Db(e) => internal(&e),
    }
}

fn not_found() -> Box<Response> {
    Box::new(failure(
        StatusCode::NOT_FOUND,
        "not_found",
        "no such bookmark",
    ))
}

fn invalid(message: &str) -> Box<Response> {
    Box::new(failure(StatusCode::BAD_REQUEST, "invalid", message))
}

fn malformed(err: &serde_json::Error) -> Box<Response> {
    invalid(&format!("malformed request: {err}"))
}

fn bookmark_id(raw: &str) -> Result<String, Box<Response>> {
    parse_id(raw).ok_or_else(not_found)
}

fn bookmark_json(b: &Bookmark) -> serde_json::Value {
    serde_json::json!({
        "id": b.id,
        "name": b.name,
        "freq_hz": b.freq_hz,
        "mode": b.mode,
        "bandwidth_hz": b.bandwidth_hz,
        "tags": b.tags,
        "color": b.color,
        "notes": b.notes,
        "created_ms": b.created_ms,
        "updated_ms": b.updated_ms,
    })
}

fn bookmark_response(status: StatusCode, b: &Bookmark) -> Response {
    let mut response = json(status, bookmark_json(b).to_string());
    if status == StatusCode::CREATED
        && let Ok(location) = header::HeaderValue::from_str(&format!("/api/v1/bookmarks/{}", b.id))
    {
        response.headers_mut().insert(header::LOCATION, location);
    }
    response
}

// ---------- validation ----------

fn check_name(raw: &str) -> Result<String, &'static str> {
    let name = raw.trim();
    let chars = name.chars().count();
    if chars == 0 || chars > MAX_NAME_CHARS {
        Err("name must be 1 to 100 characters")
    } else if name.chars().any(char::is_control) {
        Err("name must not contain control characters")
    } else {
        Ok(name.to_owned())
    }
}

fn check_freq(freq_hz: i64) -> Result<i64, &'static str> {
    if (1..=MAX_FREQ_HZ).contains(&freq_hz) {
        Ok(freq_hz)
    } else {
        Err("freq_hz must be 1 to 10000000000")
    }
}

fn check_bandwidth(bandwidth_hz: Option<i64>) -> Result<Option<i32>, &'static str> {
    bandwidth_hz
        .map(|b| {
            i32::try_from(b)
                .ok()
                .filter(|b| (1..=MAX_BANDWIDTH_HZ).contains(b))
                .ok_or("bandwidth_hz must be 1 to 10000000")
        })
        .transpose()
}

/// Trimmed tags of 1..=32 characters without control characters; at most 16 are accepted
/// (counted before de-duplication, which keeps the first occurrence).
fn check_tags(raw: &[String]) -> Result<Vec<String>, &'static str> {
    if raw.len() > MAX_TAGS {
        return Err("at most 16 tags");
    }
    let mut tags: Vec<String> = Vec::with_capacity(raw.len());
    for tag in raw {
        let tag = tag.trim();
        let chars = tag.chars().count();
        if chars == 0 || chars > MAX_TAG_CHARS {
            return Err("a tag must be 1 to 32 characters");
        }
        if tag.chars().any(char::is_control) {
            return Err("tags must not contain control characters");
        }
        if !tags.iter().any(|t| t == tag) {
            tags.push(tag.to_owned());
        }
    }
    Ok(tags)
}

/// `#rrggbb` (any case), returned lower-case.
fn check_color(raw: &str) -> Result<String, &'static str> {
    let ok =
        raw.len() == 7 && raw.starts_with('#') && raw[1..].bytes().all(|b| b.is_ascii_hexdigit());
    if ok {
        Ok(raw.to_ascii_lowercase())
    } else {
        Err("color must look like #rrggbb")
    }
}

fn check_notes(raw: &str) -> Result<String, &'static str> {
    if raw.chars().count() > MAX_NOTES_CHARS {
        Err("notes must be at most 1000 characters")
    } else if raw
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        Err("notes must not contain control characters")
    } else {
        Ok(raw.to_owned())
    }
}

/// Demodulation modes, spelled like the VFO API (`docs/api-devices.md`) plus the SSB/CW modes.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Wfm,
    Nfm,
    Am,
    Usb,
    Lsb,
    Cw,
}

impl Mode {
    fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "wfm" => Self::Wfm,
            "nfm" => Self::Nfm,
            "am" => Self::Am,
            "usb" => Self::Usb,
            "lsb" => Self::Lsb,
            "cw" => Self::Cw,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Wfm => "wfm",
            Self::Nfm => "nfm",
            Self::Am => "am",
            Self::Usb => "usb",
            Self::Lsb => "lsb",
            Self::Cw => "cw",
        }
    }
}

/// Create body. `id` and the timestamps are accepted and ignored so that an export (the list
/// response) can be fed back into `bulk` unchanged.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    name: String,
    freq_hz: i64,
    #[serde(default)]
    mode: Option<Mode>,
    #[serde(default)]
    bandwidth_hz: Option<i64>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default, rename = "id")]
    _id: Option<IgnoredAny>,
    #[serde(default, rename = "created_ms")]
    _created_ms: Option<IgnoredAny>,
    #[serde(default, rename = "updated_ms")]
    _updated_ms: Option<IgnoredAny>,
}

impl CreateBody {
    fn validate(self) -> Result<NewBookmark, &'static str> {
        Ok(NewBookmark {
            name: check_name(&self.name)?,
            freq_hz: check_freq(self.freq_hz)?,
            mode: self.mode.map(|m| m.as_str().to_owned()),
            bandwidth_hz: check_bandwidth(self.bandwidth_hz)?,
            tags: check_tags(&self.tags)?,
            color: self.color.as_deref().map(check_color).transpose()?,
            notes: self.notes.as_deref().map(check_notes).transpose()?,
        })
    }
}

/// Distinguishes an absent field (`None`) from an explicit `null` (`Some(None)`).
fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// An optional field that may be absent but not `null`.
fn not_null<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchBody {
    #[serde(default, deserialize_with = "not_null")]
    name: Option<String>,
    #[serde(default, deserialize_with = "not_null")]
    freq_hz: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    mode: Option<Option<Mode>>,
    #[serde(default, deserialize_with = "present")]
    bandwidth_hz: Option<Option<i64>>,
    #[serde(default, deserialize_with = "not_null")]
    tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    color: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    notes: Option<Option<String>>,
}

impl PatchBody {
    /// Apply the patch to `current` and validate the result.
    fn apply(self, current: &Bookmark) -> Result<NewBookmark, &'static str> {
        let merged = CreateBody {
            name: self.name.unwrap_or_else(|| current.name.clone()),
            freq_hz: self.freq_hz.unwrap_or(current.freq_hz),
            mode: match self.mode {
                Some(mode) => mode,
                None => current.mode.as_deref().and_then(Mode::parse),
            },
            bandwidth_hz: self
                .bandwidth_hz
                .unwrap_or_else(|| current.bandwidth_hz.map(i64::from)),
            tags: self.tags.unwrap_or_else(|| current.tags.clone()),
            color: self.color.unwrap_or_else(|| current.color.clone()),
            notes: self.notes.unwrap_or_else(|| current.notes.clone()),
            _id: None,
            _created_ms: None,
            _updated_ms: None,
        };
        merged.validate()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BulkBody {
    bookmarks: Vec<CreateBody>,
    #[serde(default)]
    replace: bool,
}

// ---------- handlers ----------

async fn list_bookmarks(State(state): State<BookmarksState>) -> Response {
    let result = async {
        let all = bookmarks::list(pool(&state)?)
            .await
            .map_err(|e| internal(&e))?;
        let all: Vec<_> = all.iter().map(bookmark_json).collect();
        Ok::<_, Box<Response>>(json(
            StatusCode::OK,
            serde_json::json!({"v": 1, "bookmarks": all}).to_string(),
        ))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn create_bookmark(
    State(state): State<BookmarksState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let parsed: CreateBody = serde_json::from_slice(&body).map_err(|e| malformed(&e))?;
        let new = parsed.validate().map_err(invalid)?;
        let created = bookmarks::create(pool(&state)?, &new)
            .await
            .map_err(write_failure)?;
        Ok::<_, Box<Response>>(bookmark_response(StatusCode::CREATED, &created))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn update_bookmark(
    State(state): State<BookmarksState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let id = bookmark_id(&id)?;
        let patch: PatchBody = serde_json::from_slice(&body).map_err(|e| malformed(&e))?;
        let pool = pool(&state)?;
        let current = bookmarks::get(pool, &id)
            .await
            .map_err(|e| internal(&e))?
            .ok_or_else(not_found)?;
        let new = patch.apply(&current).map_err(invalid)?;
        let updated = bookmarks::update(pool, &id, &new)
            .await
            .map_err(|e| internal(&e))?
            .ok_or_else(not_found)?;
        Ok::<_, Box<Response>>(bookmark_response(StatusCode::OK, &updated))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn delete_bookmark(
    State(state): State<BookmarksState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, false)?;
        let id = bookmark_id(&id)?;
        if !bookmarks::delete(pool(&state)?, &id)
            .await
            .map_err(|e| internal(&e))?
        {
            return Err(not_found());
        }
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NO_CONTENT;
        Ok::<_, Box<Response>>(response)
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn bulk_create(
    State(state): State<BookmarksState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let parsed: BulkBody = serde_json::from_slice(&body).map_err(|e| malformed(&e))?;
        let replace = parsed.replace;
        let mut items = Vec::with_capacity(parsed.bookmarks.len());
        for (index, item) in parsed.bookmarks.into_iter().enumerate() {
            items.push(
                item.validate()
                    .map_err(|e| invalid(&format!("bookmarks[{index}]: {e}")))?,
            );
        }
        if i64::try_from(items.len()).map_or(true, |n| n > MAX_BOOKMARKS) {
            return Err(write_failure(bookmarks::WriteError::Limit));
        }
        let created = bookmarks::create_many(pool(&state)?, &items, replace)
            .await
            .map_err(write_failure)?;
        Ok::<_, Box<Response>>(json(
            StatusCode::OK,
            serde_json::json!({"v": 1, "created": created}).to_string(),
        ))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::OriginPolicy;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn call(request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let devices = DevicesState::new(None, OriginPolicy::default());
        let app = router(BookmarksState::new(devices, None));
        let response = app.oneshot(request).await.expect("infallible");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    fn req(method: &str, path: &str, origin: Option<&str>, body: &str) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "sdr.local:8073");
        if !body.is_empty() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        builder.body(Body::from(body.to_owned())).expect("request")
    }

    #[tokio::test]
    async fn mutations_check_origin_and_body_before_the_database() {
        let ok = r#"{"name":"a","freq_hz":100000000}"#;
        let (status, _) = call(req(
            "POST",
            "/api/v1/bookmarks",
            Some("https://evil.example"),
            ok,
        ))
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        for bad in [
            "{}",
            r#"{"name":"","freq_hz":1}"#,
            r#"{"name":"a","freq_hz":0}"#,
            r#"{"name":"a","freq_hz":10000000001}"#,
            r#"{"name":"a","freq_hz":1,"mode":"fm"}"#,
            r#"{"name":"a","freq_hz":1,"x":1}"#,
            r#"{"name":"a","freq_hz":1,"color":"red"}"#,
            r#"{"name":"a","freq_hz":1,"bandwidth_hz":0}"#,
        ] {
            let (status, body) = call(req("POST", "/api/v1/bookmarks", None, bad)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
            assert_eq!(body["code"], "invalid", "{bad}");
        }
        let (status, body) = call(req("POST", "/api/v1/bookmarks", None, ok)).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::SERVICE_UNAVAILABLE, Some("unavailable"))
        );
        let (status, body) = call(req(
            "POST",
            "/api/v1/bookmarks/bulk",
            None,
            r#"{"bookmarks":[{"name":"a","freq_hz":0}]}"#,
        ))
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some("invalid"))
        );
    }

    #[tokio::test]
    async fn bad_ids_are_not_found() {
        for method in ["PATCH", "DELETE"] {
            let body = if method == "PATCH" { "{}" } else { "" };
            let (status, body) = call(req(method, "/api/v1/bookmarks/nope", None, body)).await;
            assert_eq!(
                (status, body["code"].as_str()),
                (StatusCode::NOT_FOUND, Some("not_found"))
            );
        }
    }

    #[test]
    fn values_are_normalised() {
        assert_eq!(check_name("  Tower ").as_deref(), Ok("Tower"));
        assert!(check_name(&"x".repeat(101)).is_err());
        assert_eq!(check_color("#AbCdEf").as_deref(), Ok("#abcdef"));
        for bad in ["abcdef", "#abcde", "#abcdeg", "#abcdef0", "#ääää"] {
            assert!(check_color(bad).is_err(), "{bad}");
        }
        assert_eq!(
            check_tags(&[" a ".into(), "b".into(), "a".into()]),
            Ok(vec!["a".to_owned(), "b".to_owned()])
        );
        assert!(check_tags(&vec!["x".to_owned(); 17]).is_err());
        assert!(check_tags(&["".to_owned()]).is_err());
        assert!(check_tags(&["y".repeat(33)]).is_err());
        assert!(check_notes(&"n".repeat(1001)).is_err());
        assert!(check_notes("line\nbreak").is_ok());
        assert!(check_bandwidth(Some(10_000_001)).is_err());
        assert_eq!(check_bandwidth(None), Ok(None));
    }
}
