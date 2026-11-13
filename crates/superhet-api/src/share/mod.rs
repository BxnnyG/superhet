//! Public receiver share links (ADR-0029, `docs/api-share.md`).
//!
//! An admin makes a link; whoever holds its token may open `/r/<token>` (full public page) or
//! `/embed/<token>` (compact player for an iframe) without an account. The token is the only
//! credential: it travels in the URL path of every request, so no cookie is needed.
//!
//! This is a separate door beside the session check, not a kind of session. Every route below
//! goes through [`gate`], which resolves the token and refuses (fail closed) unless the link is
//! active, allows the feature the route belongs to, the device policy still shows the device to
//! guests and the caps have room. Handlers only ever act on the device the link names; the
//! client chooses nothing about it. What a connection may hear is decided by [`audio_refusal`],
//! what it may retune to by [`links::Link::check_tune`].

// Handlers use `Result<_, Response>` so `?` short-circuits straight into the HTTP answer.
#![allow(clippy::result_large_err)]

mod audio;
pub(crate) mod links;
pub(crate) mod live;
mod upkeep;

use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::{
    Extension, Router,
    extract::{MatchedPath, Request, State, ws::WebSocketUpgrade},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::get,
};
use serde_json::{Value, json};
use superhet_bus::audit::Outcome;
use superhet_db::users::Role;

use crate::audio_hub::AudioHub;
use crate::audit::{ActorKind, Who};
use crate::auth::{AuthState, ClientInfo, error, json_reply, limiter};
use crate::device_access::{Need as DeviceNeed, Subject};
use crate::devices::{self, DevicesState};
use crate::hub::{Hub, HubError};
use crate::registry::now_ms;
use links::{Feature, Kind, Link, Status, Tuning};
use live::{Live, Permit, Refusal};

pub use upkeep::spawn as spawn_upkeep;

/// Environment variable: connections of all share links together (default 100).
pub const ENV_MAX_LISTENERS: &str = "SUPERHET_SHARE_MAX_LISTENERS";
const DEFAULT_GLOBAL_CAP: usize = 100;
/// Connections one client address may hold across all links.
const ADDR_CAP: usize = 8;
/// How long the list of operator accounts is reused.
const OPERATORS_TTL: Duration = Duration::from_secs(10);

/// The table of open connections, with the global cap from the environment.
#[must_use]
pub fn live_from_env() -> Arc<Live> {
    let cap = std::env::var(ENV_MAX_LISTENERS)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_GLOBAL_CAP);
    Live::new(cap, ADDR_CAP)
}

/// What a route needs from a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    /// The public description of the link.
    Info,
    /// The trimmed device state.
    State,
    /// A WebSocket of this kind.
    Ws(Kind),
}

const ROUTES: &[(&str, Need)] = &[
    ("/api/v1/share/{token}", Need::Info),
    ("/api/v1/share/{token}/state", Need::State),
    (
        "/api/v1/share/{token}/ws/spectrum",
        Need::Ws(Kind::Spectrum),
    ),
    ("/api/v1/share/{token}/ws/audio", Need::Ws(Kind::Audio)),
    ("/api/v1/share/{token}/ws/events", Need::Ws(Kind::Events)),
];

fn need_of(matched: &str) -> Option<Need> {
    ROUTES.iter().find(|(p, _)| *p == matched).map(|(_, n)| *n)
}

impl Need {
    /// Whether `link` allows what the route serves.
    fn permitted_by(self, link: &Link) -> bool {
        match self {
            Self::Info => true,
            Self::State => link.allows(Feature::Waterfall) || link.allows_any_audio(),
            Self::Ws(Kind::Spectrum) => link.allows(Feature::Waterfall),
            Self::Ws(Kind::Audio) => link.allows_any_audio(),
            Self::Ws(Kind::Events) => link.allows(Feature::DecoderEvents),
        }
    }
}

/// Why a token was turned away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Denial {
    Expired,
    Revoked,
    FeatureOff,
}

impl Denial {
    const fn code(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::Revoked => "revoked",
            Self::FeatureOff => "feature_off",
        }
    }

    const fn status(self) -> StatusCode {
        match self {
            Self::Expired | Self::Revoked => StatusCode::GONE,
            Self::FeatureOff => StatusCode::FORBIDDEN,
        }
    }
}

/// The link-level decision for a route: usable, and the route's feature allowed.
fn decide(link: &Link, need: Need, now_ms: i64) -> Result<(), Denial> {
    match link.status(now_ms) {
        Status::Active => {}
        Status::Expired => return Err(Denial::Expired),
        Status::Revoked => return Err(Denial::Revoked),
    }
    if need.permitted_by(link) {
        Ok(())
    } else {
        Err(Denial::FeatureOff)
    }
}

/// The token in `/api/v1/share/<token>[/...]`.
fn token_of(path: &str) -> Option<&str> {
    let token = path.strip_prefix("/api/v1/share/")?.split('/').next()?;
    (!token.is_empty()).then_some(token)
}

/// The VFOs of `doc` an operator is responsible for: ownerless (shared) ones and those owned by
/// an account with the operator or admin role. Guests' and share listeners' VFOs never are.
fn operator_vfos<'a>(doc: &'a Value, operators: &HashSet<i64>) -> Vec<&'a Value> {
    doc.get("vfos")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter(|v| match v.get("owner").and_then(Value::as_str) {
            None => true,
            Some(owner) => owner
                .strip_prefix("u:")
                .and_then(|id| id.parse::<i64>().ok())
                .is_some_and(|id| operators.contains(&id)),
        })
        .collect()
}

/// Whether a connection of `link` may hear every VFO in `wanted`. `doc` is the device state (only
/// needed for links that follow the operator), `own` the VFO this connection created on a fixed
/// link. Anything not explicitly allowed is refused, unknown ids included.
fn audio_refusal(
    link: &Link,
    doc: Option<&Value>,
    operators: &HashSet<i64>,
    own: Option<u32>,
    wanted: &BTreeSet<u32>,
) -> Result<(), &'static str> {
    for id in wanted {
        if superhet_bus::audio::is_plugin_vfo(*id) {
            if !link.allows(Feature::PluginAudio) {
                return Err("forbidden");
            }
            continue;
        }
        if !link.allows(Feature::Audio) {
            return Err("forbidden");
        }
        let allowed = match &link.tuning {
            Tuning::Fixed { .. } => own == Some(*id),
            Tuning::Follow => doc.is_some_and(|doc| {
                operator_vfos(doc, operators)
                    .iter()
                    .any(|v| v.get("id").and_then(Value::as_u64) == Some(u64::from(*id)))
            }),
        };
        if !allowed {
            return Err("forbidden");
        }
    }
    Ok(())
}

/// The device state as a share page may see it: the band, and (on links that follow the
/// operator) the operator's VFOs. No owners, locks, gains or other listeners.
fn public_state(link: &Link, doc: &Value, operators: &HashSet<i64>) -> Value {
    let vfos: Vec<Value> = if link.tuning == Tuning::Follow && link.allows_any_audio() {
        operator_vfos(doc, operators)
            .iter()
            .map(|v| {
                json!({
                    "id": v.get("id"),
                    "freq_hz": v.get("freq_hz"),
                    "mode": v.get("mode"),
                    "bandwidth_hz": v.get("bandwidth_hz"),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    json!({
        "v": 1,
        "center_hz": doc.get("center_hz"),
        "sample_rate": doc.get("sample_rate"),
        "vfos": vfos,
    })
}

/// How device policies see a share listener: as a guest (the least privileged caller).
fn guest_subject() -> Subject {
    Subject {
        guest: true,
        user_id: None,
        role: Role::Listener,
    }
}

/// What a request resolved to, handed to the handlers as a request extension.
#[derive(Clone)]
struct Grant {
    link: Arc<Link>,
    client: ClientInfo,
    /// Present for WebSocket routes; the slot is freed when the last clone is dropped.
    permit: Option<Arc<Permit>>,
}

struct Inner {
    auth: AuthState,
    devices: DevicesState,
    hub: Arc<Hub>,
    audio_hub: Arc<AudioHub>,
    /// Every request, per client address.
    requests: limiter::Limiter,
    /// Tokens that matched no link, per client address.
    bad_tokens: limiter::Limiter,
    operators: Mutex<Option<(Instant, Arc<HashSet<i64>>)>>,
}

/// Shared state of the share routes (cheap to clone).
#[derive(Clone)]
pub struct ShareState(Arc<Inner>);

impl ShareState {
    /// Create the state from the device routes' state and the two stream hubs.
    #[must_use]
    pub fn new(devices: DevicesState, hub: Arc<Hub>, audio_hub: Arc<AudioHub>) -> Self {
        let window = Duration::from_secs(60);
        Self(Arc::new(Inner {
            auth: devices.auth.clone(),
            devices,
            hub,
            audio_hub,
            requests: limiter::Limiter::new(limiter::Limits {
                window,
                per_ip: 240,
                global: 6000,
                per_name: 0,
                max_tracked_ips: 10_000,
            }),
            bad_tokens: limiter::Limiter::new(limiter::Limits {
                window: Duration::from_secs(15 * 60),
                per_ip: 20,
                global: 500,
                per_name: 0,
                max_tracked_ips: 10_000,
            }),
            operators: Mutex::new(None),
        }))
    }

    fn auth(&self) -> &AuthState {
        &self.0.auth
    }

    fn live(&self) -> &Arc<Live> {
        self.0.auth.share_live()
    }

    /// The accounts whose VFOs a following link may play (operators and admins). Unreadable
    /// accounts mean nobody, so a following link then plays nothing.
    async fn operators(&self) -> Arc<HashSet<i64>> {
        {
            let cache = self
                .0
                .operators
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some((at, set)) = cache.as_ref()
                && at.elapsed() < OPERATORS_TTL
            {
                return Arc::clone(set);
            }
        }
        let set: HashSet<i64> = match self.0.auth.db() {
            Ok(db) => superhet_db::users::list(db)
                .await
                .map(|users| {
                    users
                        .iter()
                        .filter(|u| !u.disabled && u.role >= Role::Operator)
                        .map(|u| u.id)
                        .collect()
                })
                .unwrap_or_default(),
            Err(_) => HashSet::new(),
        };
        let set = Arc::new(set);
        *self
            .0
            .operators
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((Instant::now(), Arc::clone(&set)));
        set
    }

    /// The audit actor of a share connection: a guest named after the link.
    fn who(client: &ClientInfo, link: Option<&Link>, id: Option<&str>) -> Who {
        Who {
            kind: ActorKind::Guest,
            id: id.map(str::to_owned),
            name: Some(link.map_or_else(|| "share".to_owned(), |l| format!("share:{}", l.label))),
            role: Some(Role::Listener),
            ip: client.ip,
            peer: client.peer,
            via: client.chain.clone(),
            user_agent: client.user_agent.clone(),
        }
    }

    fn deny(&self, client: &ClientInfo, link: Option<&Link>, route: &str, code: &str) {
        self.0.auth.audit().log_coalesced(
            &Self::who(
                client,
                link,
                link.map(|l| format!("share-{}", l.id)).as_deref(),
            ),
            "share.denied",
            Some(route),
            Outcome::Denied,
            json!({"reason": code, "link": link.map(|l| l.id)}),
        );
    }
}

/// Routes of the public share API (not behind the session check; [`gate`] guards each one).
pub fn router(state: ShareState) -> Router {
    Router::new()
        .route("/api/v1/share/{token}", get(info))
        .route("/api/v1/share/{token}/state", get(state_doc))
        .route("/api/v1/share/{token}/ws/spectrum", get(spectrum_ws))
        .route("/api/v1/share/{token}/ws/audio", get(audio::audio_ws))
        .route("/api/v1/share/{token}/ws/events", get(events_ws))
        .route_layer(middleware::from_fn_with_state(state.clone(), gate))
        .with_state(state)
}

fn too_many(wait: Duration) -> Response {
    let mut response = error(StatusCode::TOO_MANY_REQUESTS, "rate_limited", None);
    if let Ok(secs) = HeaderValue::from_str(&wait.as_secs().max(1).to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, secs);
    }
    response
}

/// Resolve the token of a request and refuse unless everything about the link allows it.
async fn gate(State(state): State<ShareState>, mut request: Request, next: Next) -> Response {
    let Some((route, need)) = request
        .extensions()
        .get::<MatchedPath>()
        .and_then(|p| need_of(p.as_str()).map(|n| (p.as_str().to_owned(), n)))
    else {
        // A route without an entry in `ROUTES` is never served.
        return error(StatusCode::NOT_FOUND, "not_found", None);
    };
    let client = state.auth().client_info(&request);
    let now = Instant::now();
    if let Err(wait) = state.0.requests.check(client.ip, now) {
        return too_many(wait);
    }
    state.0.requests.record_failure(client.ip, now);
    if let Err(wait) = state.0.bad_tokens.check(client.ip, now) {
        return too_many(wait);
    }
    let Some(token) = token_of(request.uri().path()) else {
        return error(StatusCode::NOT_FOUND, "not_found", None);
    };
    let found = state
        .auth()
        .share_links()
        .find(&crate::auth::token::hash_token(token))
        .await;
    let link = match found {
        Ok(Some(link)) => link,
        Ok(None) => {
            state.0.bad_tokens.record_failure(client.ip, now);
            state.deny(&client, None, &route, "unknown_token");
            return error(StatusCode::NOT_FOUND, "not_found", None);
        }
        Err(_) => return crate::auth::unavailable(),
    };
    if let Err(denial) = decide(&link, need, now_ms()) {
        state.deny(&client, Some(&link), &route, denial.code());
        return error(denial.status(), denial.code(), None);
    }
    // A device an admin hid from guests stays hidden: a link never widens a device policy.
    let policy = match state.auth().device_policies().policy(&link.device).await {
        Ok(policy) => policy,
        Err(_) => return crate::auth::unavailable(),
    };
    if !policy.allows(&guest_subject(), DeviceNeed::View) {
        state.deny(&client, Some(&link), &route, "device_restricted");
        return error(StatusCode::FORBIDDEN, "device_restricted", None);
    }
    let permit = match need {
        Need::Ws(kind) => match state.live().acquire(&link, kind, client.ip) {
            Ok(permit) => Some(Arc::new(permit)),
            Err(refusal) => {
                state.deny(&client, Some(&link), &route, refusal.code());
                return match refusal {
                    Refusal::TooManyFromAddress => too_many(Duration::from_secs(10)),
                    Refusal::LinkFull | Refusal::ServerFull => {
                        error(StatusCode::SERVICE_UNAVAILABLE, refusal.code(), None)
                    }
                };
            }
        },
        _ => None,
    };
    request.extensions_mut().insert(Grant {
        link,
        client,
        permit,
    });
    next.run(request).await
}

/// `GET /api/v1/share/{token}`: what the page shows about the link.
async fn info(State(state): State<ShareState>, Extension(grant): Extension<Grant>) -> Response {
    let link = &grant.link;
    let tuning = match &link.tuning {
        Tuning::Follow => json!({"mode": "follow"}),
        Tuning::Fixed {
            freq_hz,
            mode,
            range,
        } => json!({
            "mode": "fixed",
            "freq_hz": freq_hz,
            "demod": mode,
            "range": range.map(|(lo, hi)| json!({"min_hz": lo, "max_hz": hi})),
        }),
    };
    json_reply(
        StatusCode::OK,
        &json!({
            "v": 1,
            "title": link.title,
            "description": link.description,
            "location": link.location,
            "device": link.device,
            "features": link.features.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
            "tuning": tuning,
            "listeners": state.live().count(link.id, link.primary_kind()),
            "max_listeners": link.max_listeners,
            "expires_at_ms": link.expires_at_ms,
        }),
    )
}

/// `GET /api/v1/share/{token}/state`: the trimmed device state (polled by the page).
async fn state_doc(
    State(state): State<ShareState>,
    Extension(grant): Extension<Grant>,
) -> Response {
    let link = &grant.link;
    let stored = match devices::load_state(&state.0.devices, &link.device).await {
        Ok(stored) => stored,
        Err(response) => return *response,
    };
    let Ok(doc) = serde_json::from_slice::<Value>(&stored) else {
        return devices::failure(StatusCode::BAD_GATEWAY, "bus", "malformed device state");
    };
    let operators = state.operators().await;
    let mut out = public_state(link, &doc, &operators);
    if link.allows(Feature::PluginAudio) {
        out["plugin_streams"] =
            Value::Array(crate::plugins::audio_streams_of(&state.0.devices, &link.device).await);
    }
    out["listeners"] = json!(state.live().count(link.id, link.primary_kind()));
    json_reply(StatusCode::OK, &out)
}

/// `GET /api/v1/share/{token}/ws/spectrum`.
async fn spectrum_ws(
    State(state): State<ShareState>,
    Extension(grant): Extension<Grant>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    if !state.0.devices.origins.allows(&headers) {
        tracing::warn!("share spectrum ws: origin rejected");
        return Err(StatusCode::FORBIDDEN);
    }
    let permit = grant
        .permit
        .clone()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let device = grant.link.device.clone();
    let streams = state.0.hub.subscribe(&device).await.map_err(|err| {
        tracing::warn!(%err, "share spectrum ws: subscribe failed");
        match err {
            HubError::InvalidDevice => StatusCode::BAD_REQUEST,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        }
    })?;
    let who = ShareState::who(&grant.client, Some(&grant.link), Some(permit.key()));
    let conn = state
        .auth()
        .registry()
        .connect(permit.key(), &who, "spectrum", &device)
        .await;
    let hub = Arc::clone(&state.0.hub);
    Ok(upgrade
        .max_message_size(crate::ws::MAX_CONTROL_BYTES)
        .max_frame_size(crate::ws::MAX_CONTROL_BYTES)
        .on_upgrade(move |socket| async move {
            let _client = crate::metrics::track_client("spectrum");
            crate::ws::serve_client(socket, streams, &hub, &device, conn).await;
            hub.release(&device).await;
            drop(permit);
        }))
}

/// `GET /api/v1/share/{token}/ws/events`.
async fn events_ws(
    State(state): State<ShareState>,
    Extension(grant): Extension<Grant>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    if !state.0.devices.origins.allows(&headers) {
        tracing::warn!("share events ws: origin rejected");
        return Err(StatusCode::FORBIDDEN);
    }
    let permit = grant
        .permit
        .clone()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let device = grant.link.device.clone();
    let subject =
        superhet_bus::subjects::decoder_events(&device).map_err(|_| StatusCode::BAD_REQUEST)?;
    let client = match &state.0.devices.nats {
        Some(c) if c.connection_state() == async_nats::connection::State::Connected => c.clone(),
        _ => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };
    let updates = client.subscribe(subject).await.map_err(|err| {
        tracing::warn!(%err, "share events ws: subscribe failed");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    let who = ShareState::who(&grant.client, Some(&grant.link), Some(permit.key()));
    let conn = state
        .auth()
        .registry()
        .connect(permit.key(), &who, "events", &device)
        .await;
    Ok(upgrade
        .max_message_size(crate::decoder_events::MAX_CLIENT_BYTES)
        .max_frame_size(crate::decoder_events::MAX_CLIENT_BYTES)
        .on_upgrade(move |socket| async move {
            let _client = crate::metrics::track_client("events");
            crate::decoder_events::serve_client(socket, updates, conn).await;
            drop(permit);
        }))
}

/// End every open connection of a link (after it was revoked, changed or deleted); the browser
/// reconnects and is judged by the new state of the link.
pub fn kick_link(auth: &AuthState, link: i64) -> usize {
    let keys = auth.share_live().keys_of(link);
    for key in &keys {
        auth.registry().kick(key);
    }
    keys.len()
}

#[cfg(test)]
mod tests;
