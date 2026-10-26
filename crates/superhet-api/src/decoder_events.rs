//! Decoder event history over REST: `GET`/`DELETE /api/v1/decoder-events`,
//! `GET /api/v1/decoder-events/export.csv`, and the live feed `GET /api/v1/ws/events?dev=`.
//! Contract: `docs/api-decoder-events.md`.
//!
//! The worker writes the events (`superhet-worker`); this module only reads and clears them and
//! forwards each newly stored event, announced on the bus, to WebSocket clients.

use std::sync::Arc;

use async_nats::{Subscriber, connection::State as Connection};
use axum::{
    Extension, Router,
    body::Body,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::get,
};
use futures_util::StreamExt;
use serde::Deserialize;
use superhet_bus::subjects;
use superhet_db::{
    PgPool,
    decoder_events::{self, Event, KINDS, Query as DbQuery},
};
use tokio::sync::Semaphore;

use crate::devices::{DevicesState, failure, guard_mutation, json};
use crate::ws::send;

/// Route path of the live feed.
pub const WS_PATH: &str = "/api/v1/ws/events";
/// Most events in one page.
const MAX_LIMIT: i64 = 1000;
/// Default page size.
const DEFAULT_LIMIT: i64 = 100;
/// Most rows in a CSV export.
const MAX_EXPORT: usize = 100_000;
/// Rows fetched per query while exporting.
const EXPORT_PAGE: i64 = 5000;
const MAX_SEARCH_CHARS: usize = 100;
const MAX_CLIENT_BYTES: usize = 1024;
const MAX_WS_CLIENTS: usize = 32;

/// Shared state of the decoder event routes.
#[derive(Clone)]
pub struct EventsState {
    devices: DevicesState,
    db: Option<PgPool>,
    slots: Arc<Semaphore>,
}

impl EventsState {
    /// Create the state from the device routes' state (bus, origin policy) and the database.
    #[must_use]
    pub fn new(devices: DevicesState, db: Option<PgPool>) -> Self {
        Self {
            devices,
            db,
            slots: Arc::new(Semaphore::new(MAX_WS_CLIENTS)),
        }
    }
}

/// Routes, behind the authentication hook.
pub fn router(state: EventsState) -> Router {
    Router::new()
        .route(
            "/api/v1/decoder-events",
            get(list_events).delete(clear_events),
        )
        .route("/api/v1/decoder-events/export.csv", get(export_csv))
        .route(WS_PATH, get(events_ws))
        .with_state(state)
}

fn pool(state: &EventsState) -> Result<&PgPool, Box<Response>> {
    state.db.as_ref().ok_or_else(|| {
        Box::new(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "no database",
        ))
    })
}

fn internal(err: &dyn std::fmt::Display) -> Box<Response> {
    tracing::error!(%err, "decoder event database error");
    Box::new(failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "database error",
    ))
}

fn invalid(message: &str) -> Box<Response> {
    Box::new(failure(StatusCode::BAD_REQUEST, "invalid", message))
}

/// Query string of the list and export endpoints (export ignores `limit` and `cursor`).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    /// Comma-separated kinds.
    kind: Option<String>,
    dev: Option<String>,
    vfo: Option<u32>,
    from: Option<i64>,
    to: Option<i64>,
    q: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

impl Filter {
    fn to_query(&self) -> Result<DbQuery, Box<Response>> {
        let mut kinds = Vec::new();
        for kind in self.kind.as_deref().unwrap_or("").split(',') {
            let kind = kind.trim();
            if kind.is_empty() {
                continue;
            }
            if !KINDS.contains(&kind) {
                return Err(invalid("unknown kind"));
            }
            if !kinds.iter().any(|k| k == kind) {
                kinds.push(kind.to_owned());
            }
        }
        let search = self
            .q
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if search
            .as_ref()
            .is_some_and(|s| s.chars().count() > MAX_SEARCH_CHARS)
        {
            return Err(invalid("q is too long"));
        }
        let device = self.dev.clone().filter(|d| !d.is_empty());
        if device.as_ref().is_some_and(|d| d.len() > 64) {
            return Err(invalid("invalid dev"));
        }
        Ok(DbQuery {
            kinds,
            device,
            vfo: match self.vfo {
                Some(v) => Some(i32::try_from(v).map_err(|_| invalid("invalid vfo"))?),
                None => None,
            },
            from_ms: self.from,
            to_ms: self.to,
            search,
            limit: DEFAULT_LIMIT,
            after: None,
        })
    }
}

/// `"<ts_ms>.<id>"` of the last row of a page.
fn cursor_of(event: &Event) -> String {
    format!("{}.{}", event.data.ts_ms, event.id)
}

fn parse_cursor(raw: &str) -> Option<(i64, i64)> {
    let (ts, id) = raw.split_once('.')?;
    Some((ts.parse().ok()?, id.parse().ok()?))
}

async fn list_events(State(state): State<EventsState>, Query(params): Query<Filter>) -> Response {
    let result = async {
        let mut query = params.to_query()?;
        let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(invalid("limit must be 1..=1000"));
        }
        query.limit = limit + 1;
        if let Some(raw) = &params.cursor {
            query.after = Some(parse_cursor(raw).ok_or_else(|| invalid("invalid cursor"))?);
        }
        let mut events = decoder_events::list(pool(&state)?, &query)
            .await
            .map_err(|e| internal(&e))?;
        let more = i64::try_from(events.len()).is_ok_and(|n| n > limit);
        events.truncate(usize::try_from(limit).unwrap_or(0));
        let next = if more {
            events.last().map(cursor_of)
        } else {
            None
        };
        let items: Vec<_> = events.iter().map(Event::to_json).collect();
        Ok::<_, Box<Response>>(json(
            StatusCode::OK,
            serde_json::json!({"v": 1, "events": items, "next_cursor": next}).to_string(),
        ))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

async fn clear_events(State(state): State<EventsState>, headers: HeaderMap) -> Response {
    let result = async {
        guard_mutation(&state.devices, &headers, false)?;
        let deleted = decoder_events::clear(pool(&state)?)
            .await
            .map_err(|e| internal(&e))?;
        Ok::<_, Box<Response>>(json(
            StatusCode::OK,
            serde_json::json!({"v": 1, "deleted": deleted}).to_string(),
        ))
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` for Unix milliseconds (proleptic Gregorian, UTC).
pub(crate) fn iso(ts_ms: i64) -> String {
    let ms = ts_ms.rem_euclid(1000);
    let secs = ts_ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// One CSV field. Text that a spreadsheet would run as a formula gets a leading quote (RadioText
/// comes from the air, anybody can send it).
pub(crate) fn csv_field(raw: &str) -> String {
    let mut text = raw.replace(['\r', '\n'], " ");
    if text.starts_with(['=', '+', '-', '@', '\t']) {
        text.insert(0, '\'');
    }
    if text.contains([',', '"']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

fn csv_row(e: &Event) -> String {
    let d = &e.data;
    let fields = [
        iso(d.ts_ms),
        d.ts_ms.to_string(),
        d.device.clone(),
        d.vfo.to_string(),
        d.freq_hz.map_or_else(String::new, |f| f.to_string()),
        d.mode.clone().unwrap_or_default(),
        d.kind.clone(),
        d.value.clone(),
        d.meta.to_string(),
    ];
    let mut line = fields
        .iter()
        .map(|f| csv_field(f))
        .collect::<Vec<_>>()
        .join(",");
    line.push_str("\r\n");
    line
}

const CSV_HEADER: &str = "time,ts_ms,device,vfo,freq_hz,mode,kind,value,meta\r\n";

async fn export_csv(State(state): State<EventsState>, Query(filter): Query<Filter>) -> Response {
    let result = async {
        let mut query = filter.to_query()?;
        let pool = pool(&state)?;
        let mut body = String::from(CSV_HEADER);
        let mut rows = 0_usize;
        query.limit = EXPORT_PAGE;
        while rows < MAX_EXPORT {
            let page = decoder_events::list(pool, &query)
                .await
                .map_err(|e| internal(&e))?;
            let Some(last) = page.last() else { break };
            query.after = Some((last.data.ts_ms, last.id));
            for event in &page {
                body.push_str(&csv_row(event));
            }
            rows += page.len();
            if i64::try_from(page.len()).is_ok_and(|n| n < EXPORT_PAGE) {
                break;
            }
        }
        let mut response = Response::new(Body::from(body));
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("text/csv; charset=utf-8"),
        );
        headers.insert(
            header::CONTENT_DISPOSITION,
            header::HeaderValue::from_static(
                "attachment; filename=\"superhet-decoder-events.csv\"",
            ),
        );
        headers.insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
        Ok::<_, Box<Response>>(response)
    }
    .await;
    result.unwrap_or_else(|r| *r)
}

#[derive(Debug, Deserialize)]
struct WsQuery {
    dev: String,
}

async fn events_ws(
    State(state): State<EventsState>,
    Extension(principal): Extension<crate::access::Principal>,
    Query(query): Query<WsQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    if !state.devices.origins.allows(&headers) {
        tracing::warn!("events ws: origin rejected");
        return Err(StatusCode::FORBIDDEN);
    }
    let subject = subjects::decoder_events(&query.dev).map_err(|_| StatusCode::BAD_REQUEST)?;
    let client = match &state.devices.nats {
        Some(client) if client.connection_state() == Connection::Connected => client.clone(),
        _ => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };
    let permit = Arc::clone(&state.slots)
        .try_acquire_owned()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let updates = client.subscribe(subject).await.map_err(|err| {
        tracing::warn!(%err, "events ws: subscribe failed");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    let conn = state
        .devices
        .auth
        .registry()
        .connect(
            &principal.session_key,
            &principal.who(),
            "events",
            &query.dev,
        )
        .await;
    Ok(upgrade
        .max_message_size(MAX_CLIENT_BYTES)
        .max_frame_size(MAX_CLIENT_BYTES)
        .on_upgrade(move |socket| async move {
            let _client = crate::metrics::track_client("events");
            serve_client(socket, updates, conn).await;
            drop(permit);
        }))
}

/// `{"v":1,"type":"decoder_event","event":<event>}` for a JSON object.
fn envelope(doc: &str) -> Option<String> {
    let is_object = serde_json::from_str::<serde_json::Value>(doc)
        .ok()
        .is_some_and(|v| v.is_object());
    is_object.then(|| format!(r#"{{"v":1,"type":"decoder_event","event":{doc}}}"#))
}

async fn serve_client(
    mut socket: WebSocket,
    mut updates: Subscriber,
    mut kill: crate::registry::ConnGuard,
) {
    loop {
        tokio::select! {
            () = kill.killed() => break,
            incoming = socket.recv() => match incoming {
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                // One-way feed; anything the client sends is ignored.
                Some(Ok(_)) => {}
            },
            update = updates.next() => {
                let Some(update) = update else { break };
                let Some(frame) = std::str::from_utf8(&update.payload).ok().and_then(envelope) else {
                    continue;
                };
                if !send(&mut socket, Message::text(frame)).await {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use superhet_db::decoder_events::NewEvent;

    fn event(value: &str) -> Event {
        Event {
            id: 9,
            data: NewEvent {
                ts_ms: 1_700_000_000_123,
                device: "rtl0".into(),
                vfo: 1,
                freq_hz: Some(433_500_000),
                mode: Some("nfm".into()),
                kind: "rds_rt".into(),
                value: value.into(),
                meta: serde_json::json!({"a": 1}),
            },
        }
    }

    #[test]
    fn iso_formats_utc_milliseconds() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso(-1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn csv_quotes_and_defuses_formulas() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("=1+1"), "'=1+1");
        assert_eq!(csv_field("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(csv_field("-5"), "'-5");
        assert_eq!(csv_field("two\nlines"), "two lines");
        let row = csv_row(&event("=cmd|' /C calc'!A0"));
        assert!(row.starts_with(
            "2023-11-14T22:13:20.123Z,1700000000123,rtl0,1,433500000,nfm,rds_rt,'=cmd"
        ));
        assert!(row.ends_with("\r\n"));
    }

    #[test]
    fn cursors_round_trip() {
        let e = event("x");
        assert_eq!(parse_cursor(&cursor_of(&e)), Some((1_700_000_000_123, 9)));
        for bad in ["", "1", "a.b", "1.2.3", ".5"] {
            assert_eq!(parse_cursor(bad), None, "{bad}");
        }
    }

    #[test]
    fn filters_are_validated() {
        let ok = Filter {
            kind: Some("dtmf, ctcss,,dtmf".into()),
            vfo: Some(2),
            q: Some("  hi ".into()),
            ..Filter::default()
        }
        .to_query()
        .expect("valid");
        assert_eq!(ok.kinds, ["dtmf", "ctcss"]);
        assert_eq!(ok.vfo, Some(2));
        assert_eq!(ok.search.as_deref(), Some("hi"));
        let bad = Filter {
            kind: Some("nope".into()),
            ..Filter::default()
        };
        assert!(bad.to_query().is_err());
        let long = Filter {
            q: Some("x".repeat(101)),
            ..Filter::default()
        };
        assert!(long.to_query().is_err());
    }

    #[test]
    fn wraps_only_json_objects() {
        assert_eq!(
            envelope(r#"{"id":1}"#).as_deref(),
            Some(r#"{"v":1,"type":"decoder_event","event":{"id":1}}"#)
        );
        assert_eq!(envelope("[]"), None);
        assert_eq!(envelope("nope"), None);
    }
}
