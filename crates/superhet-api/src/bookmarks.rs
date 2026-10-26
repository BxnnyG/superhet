//! Bookmarks (memory channels) over REST: `/api/v1/bookmarks[/{id}]`,
//! `POST /api/v1/bookmarks/bulk` and the ordered groups under `/api/v1/bookmark-groups`.
//! Contract: `docs/api-bookmarks.md`.
//!
//! Bookmarks live in PostgreSQL. The API validates and normalises every value (trimmed name,
//! lower-case colour, de-duplicated tags, tone fields that match the tone mode) before it reaches
//! the database.

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use serde::{Deserialize, de::IgnoredAny};
use serde_json::{Map, Value};
use superhet_db::{
    PgPool,
    bookmarks::{self, Bookmark, Digital, Group, MAX_BOOKMARKS, MAX_GROUPS, NewBookmark},
    profiles::parse_id,
};

use crate::devices::{DevicesState, failure, guard_mutation, json};

/// Largest accepted body of a single-bookmark request.
const MAX_BODY_BYTES: usize = 16 * 1024;
/// Largest accepted body of a bulk request.
const MAX_BULK_BYTES: usize = 8 * 1024 * 1024;

const MAX_NAME_CHARS: usize = 100;
const MAX_GROUP_NAME_CHARS: usize = 60;
const MAX_TAGS: usize = 16;
const MAX_TAG_CHARS: usize = 32;
const MAX_NOTES_CHARS: usize = 1000;
const MAX_FREQ_HZ: i64 = 10_000_000_000;
const MAX_BANDWIDTH_HZ: i32 = 10_000_000;
const MAX_STEP_HZ: i64 = 10_000_000;
const MAX_PLUGIN_ID_CHARS: usize = 64;

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
        .route(
            "/api/v1/bookmark-groups",
            get(list_groups).post(create_group).layer(limit),
        )
        .route(
            "/api/v1/bookmark-groups/order",
            axum::routing::put(order_groups).layer(limit),
        )
        .route(
            "/api/v1/bookmark-groups/{id}",
            axum::routing::patch(rename_group)
                .delete(delete_group)
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
        bookmarks::WriteError::UnknownGroup => invalid("group_id does not name an existing group"),
        bookmarks::WriteError::Duplicate => {
            Box::new(failure(StatusCode::CONFLICT, "duplicate", "already exists"))
        }
        bookmarks::WriteError::Db(e) => internal(&e),
    }
}

fn group_failure(err: bookmarks::WriteError) -> Box<Response> {
    match err {
        bookmarks::WriteError::Limit => Box::new(failure(
            StatusCode::CONFLICT,
            "limit",
            "at most 200 groups can be stored",
        )),
        bookmarks::WriteError::Duplicate => Box::new(failure(
            StatusCode::CONFLICT,
            "duplicate",
            "a group with this name exists already",
        )),
        other => write_failure(other),
    }
}

fn not_found() -> Box<Response> {
    Box::new(failure(
        StatusCode::NOT_FOUND,
        "not_found",
        "no such bookmark",
    ))
}

fn group_not_found() -> Box<Response> {
    Box::new(failure(StatusCode::NOT_FOUND, "not_found", "no such group"))
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

fn bookmark_map(b: &Bookmark) -> Map<String, Value> {
    let d = &b.data;
    let value = serde_json::json!({
        "id": b.id,
        "name": d.name,
        "freq_hz": d.freq_hz,
        "mode": d.mode,
        "bandwidth_hz": d.bandwidth_hz,
        "tags": d.tags,
        "color": d.color,
        "notes": d.notes,
        "squelch_db": d.squelch_db,
        "squelch_auto": d.squelch_auto,
        "step_hz": d.step_hz,
        "tone_mode": d.tone_mode,
        "ctcss_hz": d.ctcss_hz,
        "dcs_code": d.dcs_code,
        "dcs_polarity": d.dcs_polarity,
        "digital": d.digital.as_ref().map(|x| serde_json::json!({
            "protocol": x.protocol,
            "timeslot": x.timeslot,
            "talkgroup": x.talkgroup,
            "colour_code": x.colour_code,
            "mcc": x.mcc,
            "mnc": x.mnc,
            "decoder_plugin_id": x.decoder_plugin_id,
        })),
        "group_id": d.group_id,
        "favourite": d.favourite,
        "scan": {
            "include": d.scan_include,
            "priority": d.scan_priority,
            "dwell_ms": d.scan_dwell_ms,
        },
        "created_ms": b.created_ms,
        "updated_ms": b.updated_ms,
    });
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn bookmark_json(b: &Bookmark) -> Value {
    Value::Object(bookmark_map(b))
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

fn group_json(g: &Group) -> Value {
    serde_json::json!({"id": g.id, "name": g.name, "position": g.position})
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

fn check_group_name(raw: &str) -> Result<String, &'static str> {
    let name = raw.trim();
    let chars = name.chars().count();
    if chars == 0 || chars > MAX_GROUP_NAME_CHARS {
        Err("name must be 1 to 60 characters")
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

fn check_squelch(squelch_db: Option<i64>) -> Result<Option<i16>, &'static str> {
    squelch_db
        .map(|s| {
            i16::try_from(s)
                .ok()
                .filter(|s| (-120..=0).contains(s))
                .ok_or("squelch_db must be -120 to 0")
        })
        .transpose()
}

fn check_step(step_hz: Option<i64>) -> Result<Option<i32>, &'static str> {
    step_hz
        .map(|s| {
            i32::try_from(s)
                .ok()
                .filter(|s| (1..=MAX_STEP_HZ).contains(&i64::from(*s)))
                .ok_or("step_hz must be 1 to 10000000")
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

fn check_group_id(raw: &str) -> Result<String, &'static str> {
    parse_id(raw).ok_or("group_id must be a UUID")
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

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum ToneMode {
    None,
    Ctcss,
    Dcs,
}

#[derive(Deserialize, Clone, Copy)]
enum Polarity {
    N,
    R,
}

impl Polarity {
    fn as_str(self) -> &'static str {
        match self {
            Self::N => "N",
            Self::R => "R",
        }
    }
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Protocol {
    Tetra,
    Dmr,
    Dpmr,
    P25,
}

impl Protocol {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tetra => "tetra",
            Self::Dmr => "dmr",
            Self::Dpmr => "dpmr",
            Self::P25 => "p25",
        }
    }
}

type Tone = (Option<String>, Option<f64>, Option<i16>, Option<String>);

/// Keep only the tone fields that belong to `tone_mode`; the required one must be present.
fn check_tone(
    mode: Option<ToneMode>,
    ctcss_hz: Option<f64>,
    dcs_code: Option<i64>,
    polarity: Option<Polarity>,
) -> Result<Tone, &'static str> {
    match mode {
        None => Ok((None, None, None, None)),
        Some(ToneMode::None) => Ok((Some("none".to_owned()), None, None, None)),
        Some(ToneMode::Ctcss) => {
            let hz = ctcss_hz.ok_or("ctcss_hz is required for tone_mode ctcss")?;
            if !hz.is_finite() || !(30.0..=300.0).contains(&hz) {
                return Err("ctcss_hz must be 30 to 300");
            }
            let hz = (hz * 10.0).round() / 10.0;
            Ok((Some("ctcss".to_owned()), Some(hz), None, None))
        }
        Some(ToneMode::Dcs) => {
            let code = dcs_code.ok_or("dcs_code is required for tone_mode dcs")?;
            // The code is written as three octal digits (023, 754, ...), kept as that number.
            let octal = code.to_string().bytes().all(|b| b <= b'7');
            let code = i16::try_from(code)
                .ok()
                .filter(|c| (1..=777).contains(c) && octal)
                .ok_or("dcs_code must be an octal code from 1 to 777")?;
            let polarity = polarity.unwrap_or(Polarity::N).as_str().to_owned();
            Ok((Some("dcs".to_owned()), None, Some(code), Some(polarity)))
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DigitalBody {
    protocol: Protocol,
    #[serde(default)]
    timeslot: Option<i64>,
    #[serde(default)]
    talkgroup: Option<i64>,
    #[serde(default)]
    colour_code: Option<i64>,
    #[serde(default)]
    mcc: Option<i64>,
    #[serde(default)]
    mnc: Option<i64>,
    #[serde(default)]
    decoder_plugin_id: Option<String>,
}

fn ranged(
    value: Option<i64>,
    min: i64,
    max: i64,
    msg: &'static str,
) -> Result<Option<i16>, &'static str> {
    value
        .map(|v| {
            i16::try_from(v)
                .ok()
                .filter(|v| (min..=max).contains(&i64::from(*v)))
                .ok_or(msg)
        })
        .transpose()
}

impl DigitalBody {
    fn validate(self) -> Result<Digital, &'static str> {
        let max_slot = match self.protocol {
            Protocol::Tetra => 4,
            Protocol::Dmr => 2,
            Protocol::Dpmr | Protocol::P25 => 0,
        };
        if self.timeslot.is_some() && max_slot == 0 {
            return Err("digital.timeslot is only valid for tetra and dmr");
        }
        if self.colour_code.is_some() && self.protocol != Protocol::Dmr {
            return Err("digital.colour_code is only valid for dmr");
        }
        let talkgroup = self
            .talkgroup
            .map(|t| {
                (0..=16_777_215)
                    .contains(&t)
                    .then_some(t)
                    .ok_or("digital.talkgroup must be 0 to 16777215")
            })
            .transpose()?;
        let decoder = self
            .decoder_plugin_id
            .map(|id| {
                let ok = (1..=MAX_PLUGIN_ID_CHARS).contains(&id.chars().count())
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
                ok.then_some(id)
                    .ok_or("digital.decoder_plugin_id must be 1 to 64 of A-Z a-z 0-9 . _ -")
            })
            .transpose()?;
        Ok(Digital {
            protocol: self.protocol.as_str().to_owned(),
            timeslot: ranged(
                self.timeslot,
                1,
                max_slot,
                "digital.timeslot must be 1 to 4 (tetra) or 1 to 2 (dmr)",
            )?,
            talkgroup,
            colour_code: ranged(
                self.colour_code,
                0,
                15,
                "digital.colour_code must be 0 to 15",
            )?,
            mcc: ranged(self.mcc, 0, 999, "digital.mcc must be 0 to 999")?,
            mnc: ranged(self.mnc, 0, 9999, "digital.mnc must be 0 to 9999")?,
            decoder_plugin_id: decoder,
        })
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ScanBody {
    #[serde(default)]
    include: bool,
    #[serde(default)]
    priority: Option<i64>,
    #[serde(default)]
    dwell_ms: Option<i64>,
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
    #[serde(default)]
    squelch_db: Option<i64>,
    #[serde(default)]
    squelch_auto: bool,
    #[serde(default)]
    step_hz: Option<i64>,
    #[serde(default)]
    tone_mode: Option<ToneMode>,
    #[serde(default)]
    ctcss_hz: Option<f64>,
    #[serde(default)]
    dcs_code: Option<i64>,
    #[serde(default)]
    dcs_polarity: Option<Polarity>,
    #[serde(default)]
    digital: Option<DigitalBody>,
    #[serde(default)]
    group_id: Option<String>,
    #[serde(default)]
    favourite: bool,
    #[serde(default)]
    scan: Option<ScanBody>,
    #[serde(default, rename = "id")]
    _id: Option<IgnoredAny>,
    #[serde(default, rename = "created_ms")]
    _created_ms: Option<IgnoredAny>,
    #[serde(default, rename = "updated_ms")]
    _updated_ms: Option<IgnoredAny>,
}

impl CreateBody {
    fn validate(self) -> Result<NewBookmark, &'static str> {
        let (tone_mode, ctcss_hz, dcs_code, dcs_polarity) = check_tone(
            self.tone_mode,
            self.ctcss_hz,
            self.dcs_code,
            self.dcs_polarity,
        )?;
        let scan = self.scan.unwrap_or_default();
        let scan_priority = ranged(scan.priority, 0, 9, "scan.priority must be 0 to 9")?;
        let scan_dwell_ms = scan
            .dwell_ms
            .map(|d| {
                i32::try_from(d)
                    .ok()
                    .filter(|d| (100..=60_000).contains(d))
                    .ok_or("scan.dwell_ms must be 100 to 60000")
            })
            .transpose()?;
        Ok(NewBookmark {
            name: check_name(&self.name)?,
            freq_hz: check_freq(self.freq_hz)?,
            mode: self.mode.map(|m| m.as_str().to_owned()),
            bandwidth_hz: check_bandwidth(self.bandwidth_hz)?,
            tags: check_tags(&self.tags)?,
            color: self.color.as_deref().map(check_color).transpose()?,
            notes: self.notes.as_deref().map(check_notes).transpose()?,
            squelch_db: check_squelch(self.squelch_db)?,
            squelch_auto: self.squelch_auto,
            step_hz: check_step(self.step_hz)?,
            tone_mode,
            ctcss_hz,
            dcs_code,
            dcs_polarity,
            digital: self.digital.map(DigitalBody::validate).transpose()?,
            group_id: self.group_id.as_deref().map(check_group_id).transpose()?,
            favourite: self.favourite,
            scan_include: scan.include,
            scan_priority,
            scan_dwell_ms,
        })
    }
}

/// Apply a PATCH (a JSON object) to `current`: the patch's keys replace the current values (a
/// nested `digital` or `scan` object replaces the whole one), then the result is validated as a
/// create body.
fn merge_patch(current: &Bookmark, patch: Map<String, Value>) -> Result<NewBookmark, String> {
    if ["id", "created_ms", "updated_ms"]
        .iter()
        .any(|k| patch.contains_key(*k))
    {
        return Err("id, created_ms and updated_ms are read-only".to_owned());
    }
    let mut merged = bookmark_map(current);
    merged.extend(patch);
    let body: CreateBody = serde_json::from_value(Value::Object(merged))
        .map_err(|e| format!("malformed request: {e}"))?;
    body.validate().map_err(str::to_owned)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BulkBody {
    bookmarks: Vec<CreateBody>,
    #[serde(default)]
    replace: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupBody {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderBody {
    ids: Vec<String>,
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
        let patch: Map<String, Value> = serde_json::from_slice(&body).map_err(|e| malformed(&e))?;
        let pool = pool(&state)?;
        let current = bookmarks::get(pool, &id)
            .await
            .map_err(|e| internal(&e))?
            .ok_or_else(not_found)?;
        let new = merge_patch(&current, patch).map_err(|e| invalid(&e))?;
        let updated = bookmarks::update(pool, &id, &new)
            .await
            .map_err(write_failure)?
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
        Ok::<_, Box<Response>>(no_content())
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

fn no_content() -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    response
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

async fn groups_response(state: &BookmarksState) -> Result<Response, Box<Response>> {
    let all = bookmarks::list_groups(pool(state)?)
        .await
        .map_err(|e| internal(&e))?;
    let all: Vec<_> = all.iter().map(group_json).collect();
    Ok(json(
        StatusCode::OK,
        serde_json::json!({"v": 1, "groups": all, "max": MAX_GROUPS}).to_string(),
    ))
}

async fn list_groups(State(state): State<BookmarksState>) -> Response {
    groups_response(&state).await.unwrap_or_else(|r| *r)
}

async fn create_group(
    State(state): State<BookmarksState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let parsed: GroupBody = serde_json::from_slice(&body).map_err(|e| malformed(&e))?;
        let name = check_group_name(&parsed.name).map_err(invalid)?;
        let created = bookmarks::create_group(pool(&state)?, &name)
            .await
            .map_err(group_failure)?;
        let mut response = json(StatusCode::CREATED, group_json(&created).to_string());
        if let Ok(location) =
            header::HeaderValue::from_str(&format!("/api/v1/bookmark-groups/{}", created.id))
        {
            response.headers_mut().insert(header::LOCATION, location);
        }
        Ok::<_, Box<Response>>(response)
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn rename_group(
    State(state): State<BookmarksState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let id = parse_id(&id).ok_or_else(group_not_found)?;
        let parsed: GroupBody = serde_json::from_slice(&body).map_err(|e| malformed(&e))?;
        let name = check_group_name(&parsed.name).map_err(invalid)?;
        let renamed = bookmarks::rename_group(pool(&state)?, &id, &name)
            .await
            .map_err(group_failure)?
            .ok_or_else(group_not_found)?;
        Ok::<_, Box<Response>>(json(StatusCode::OK, group_json(&renamed).to_string()))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn delete_group(
    State(state): State<BookmarksState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, false)?;
        let id = parse_id(&id).ok_or_else(group_not_found)?;
        if !bookmarks::delete_group(pool(&state)?, &id)
            .await
            .map_err(|e| internal(&e))?
        {
            return Err(group_not_found());
        }
        Ok::<_, Box<Response>>(no_content())
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn order_groups(
    State(state): State<BookmarksState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, true)?;
        let parsed: OrderBody = serde_json::from_slice(&body).map_err(|e| malformed(&e))?;
        let ids: Vec<String> = parsed
            .ids
            .iter()
            .map(|id| parse_id(id).ok_or_else(|| invalid("ids must be group UUIDs")))
            .collect::<Result<_, _>>()?;
        if !bookmarks::reorder_groups(pool(&state)?, &ids)
            .await
            .map_err(|e| internal(&e))?
        {
            return Err(invalid("ids contains an unknown group"));
        }
        groups_response(&state).await
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn call(request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let devices = DevicesState::for_tests();
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

    fn parse(body: &str) -> Result<NewBookmark, String> {
        let parsed: CreateBody = serde_json::from_str(body).map_err(|e| e.to_string())?;
        parsed.validate().map_err(str::to_owned)
    }

    #[test]
    fn tone_fields_follow_the_tone_mode() {
        let base = r#""name":"a","freq_hz":1"#;
        let ctcss = parse(&format!(
            r#"{{{base},"tone_mode":"ctcss","ctcss_hz":88.449,"dcs_code":23}}"#
        ))
        .expect("ctcss");
        assert_eq!(ctcss.tone_mode.as_deref(), Some("ctcss"));
        assert_eq!(ctcss.ctcss_hz, Some(88.4));
        assert_eq!((ctcss.dcs_code, ctcss.dcs_polarity), (None, None));
        let dcs = parse(&format!(
            r#"{{{base},"tone_mode":"dcs","dcs_code":23,"ctcss_hz":100.0}}"#
        ))
        .expect("dcs");
        assert_eq!(dcs.dcs_code, Some(23));
        assert_eq!(dcs.dcs_polarity.as_deref(), Some("N"));
        assert_eq!(dcs.ctcss_hz, None);
        let none = parse(&format!(
            r#"{{{base},"tone_mode":"none","ctcss_hz":100.0}}"#
        ))
        .expect("none");
        assert_eq!(
            (none.tone_mode.as_deref(), none.ctcss_hz),
            (Some("none"), None)
        );
        for bad in [
            r#""tone_mode":"ctcss""#,
            r#""tone_mode":"ctcss","ctcss_hz":10"#,
            r#""tone_mode":"ctcss","ctcss_hz":301"#,
            r#""tone_mode":"dcs""#,
            r#""tone_mode":"dcs","dcs_code":0"#,
            r#""tone_mode":"dcs","dcs_code":28"#,
            r#""tone_mode":"dcs","dcs_code":1000"#,
            r#""tone_mode":"dcs","dcs_code":23,"dcs_polarity":"X""#,
            r#""tone_mode":"pl""#,
        ] {
            assert!(parse(&format!("{{{base},{bad}}}")).is_err(), "{bad}");
        }
    }

    #[test]
    fn digital_scan_and_state_fields_are_checked() {
        let base = r#""name":"a","freq_hz":1"#;
        let ok = parse(&format!(
            r#"{{{base},"squelch_db":-80,"squelch_auto":true,"step_hz":25000,"favourite":true,
               "digital":{{"protocol":"tetra","timeslot":4,"talkgroup":1001,"mcc":262,"mnc":1,
                          "decoder_plugin_id":"tetra-kit"}},
               "scan":{{"include":true,"priority":9,"dwell_ms":100}}}}"#
        ))
        .expect("full");
        assert_eq!(ok.squelch_db, Some(-80));
        assert!(ok.squelch_auto && ok.favourite && ok.scan_include);
        assert_eq!(ok.digital.as_ref().and_then(|d| d.timeslot), Some(4));
        for bad in [
            r#""squelch_db":1"#,
            r#""squelch_db":-121"#,
            r#""step_hz":0"#,
            r#""group_id":"nope""#,
            r#""digital":{"protocol":"tetra","timeslot":5}"#,
            r#""digital":{"protocol":"dmr","timeslot":3}"#,
            r#""digital":{"protocol":"dmr","timeslot":0}"#,
            r#""digital":{"protocol":"p25","timeslot":1}"#,
            r#""digital":{"protocol":"tetra","colour_code":1}"#,
            r#""digital":{"protocol":"dmr","colour_code":16}"#,
            r#""digital":{"protocol":"dmr","talkgroup":16777216}"#,
            r#""digital":{"protocol":"dmr","decoder_plugin_id":"bad id"}"#,
            r#""digital":{"protocol":"mototrbo"}"#,
            r#""digital":{}"#,
            r#""scan":{"priority":10}"#,
            r#""scan":{"dwell_ms":99}"#,
            r#""scan":{"extra":1}"#,
        ] {
            assert!(parse(&format!("{{{base},{bad}}}")).is_err(), "{bad}");
        }
        assert!(
            parse(&format!(
                r#"{{{base},"group_id":"0f8fad5b-d9cb-469f-a165-70867728950e"}}"#
            ))
            .is_ok()
        );
    }

    #[test]
    fn patch_merges_into_the_current_values() {
        let current = Bookmark {
            id: "0f8fad5b-d9cb-469f-a165-70867728950e".to_owned(),
            data: parse(r#"{"name":"a","freq_hz":5,"tone_mode":"ctcss","ctcss_hz":88.5}"#)
                .expect("current"),
            created_ms: 1,
            updated_ms: 2,
        };
        let patch = |body: &str| {
            let map: Map<String, Value> = serde_json::from_str(body).expect("json");
            merge_patch(&current, map)
        };
        let moved = patch(r#"{"freq_hz":6}"#).expect("freq");
        assert_eq!((moved.freq_hz, moved.ctcss_hz), (6, Some(88.5)));
        let cleared = patch(r#"{"tone_mode":"none"}"#).expect("none");
        assert_eq!(cleared.ctcss_hz, None);
        assert!(patch(r#"{"name":null}"#).is_err());
        assert!(patch(r#"{"tags":null}"#).is_err());
        assert!(patch(r#"{"nope":1}"#).is_err());
        assert!(patch(r#"{"tone_mode":"dcs"}"#).is_err());
    }
}
