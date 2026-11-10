//! Notification rules, targets and the delivery log (`/api/v1/admin/notification-*`, admin only)
//! and the push subscription of one's own browser (`/api/v1/me/push*`, accounts). Contract:
//! `docs/api-notifications.md`; design: ADR-0026.
//!
//! Secrets never leave through here: a target's secret is write-only (`has_secret` tells whether
//! one is set), the webhook URL is replaced by its host, the VAPID private key is not exposed at
//! all, and none of them are written to the audit log.

use std::collections::HashMap;

use axum::{
    Extension,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};
use superhet_bus::audit::Outcome;
use superhet_db::{
    decoder_events,
    notifications::{
        self as db, Delivery, NewDelivery, Quiet, Rule, RuleInput, SecretChange, Target, Updated,
    },
};
use superhet_notify::{Message, push, rule, target};

use super::{AdminState, bad, conflict, not_found};
use crate::access::{Actor, Principal};
use crate::auth::{Reply, db_failed, flatten, guard_origin, json_reply, parse_json};
use crate::registry::now_ms;

/// Most targets and rules; the matcher evaluates every enabled rule for every event.
const MAX_TARGETS: usize = 50;
const MAX_RULES: usize = 200;
const NAME_MAX: usize = 80;
const SECRET_MAX: usize = 512;
/// Most targets one rule may send to.
const MAX_RULE_TARGETS: usize = 20;

fn name_ok(name: &str) -> Result<(), Response> {
    let n = name.trim().chars().count();
    if n == 0 || n > NAME_MAX || name.chars().any(char::is_control) {
        return Err(bad("name must be 1 to 80 printable characters"));
    }
    Ok(())
}

// ------------------------------------------------------------------------------------ views

fn target_json(t: &Target) -> Value {
    json!({
        "id": t.id,
        "name": t.name,
        "kind": t.kind,
        "config": target::public_view(&t.kind, &t.config),
        "has_secret": t.secret.is_some(),
        "created_at_ms": t.created_at_ms,
        "updated_at_ms": t.updated_at_ms,
    })
}

fn clock(minutes: i32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn parse_clock(text: &str) -> Option<i32> {
    let (h, m) = text.split_once(':')?;
    let (h, m): (i32, i32) = (h.parse().ok()?, m.parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m) && text.len() == 5).then_some(h * 60 + m)
}

fn rule_json(r: &Rule) -> Value {
    let d = &r.data;
    json!({
        "id": r.id,
        "name": d.name,
        "enabled": d.enabled,
        "source": d.source,
        "kind": d.kind,
        "conditions": d.conditions,
        "cooldown_secs": d.cooldown_secs,
        "max_per_hour": d.max_per_hour,
        "quiet": d.quiet.as_ref().map(|q| json!({
            "start": clock(q.start_min), "end": clock(q.end_min), "tz": q.tz,
        })),
        "title_template": d.title_template,
        "body_template": d.body_template,
        "target_ids": d.target_ids,
        "created_at_ms": r.created_at_ms,
        "updated_at_ms": r.updated_at_ms,
    })
}

fn delivery_json(d: &Delivery) -> Value {
    json!({
        "id": d.id,
        "rule_id": d.rule_id,
        "rule_name": d.rule_name,
        "target_id": d.target_id,
        "target_name": d.target_name,
        "event_id": d.event_id,
        "status": d.status,
        "attempts": d.attempts,
        "last_error": d.last_error,
        "title": d.message.get("title"),
        "body": d.message.get("body"),
        "created_at_ms": d.created_at_ms,
        "finished_at_ms": d.finished_at_ms,
    })
}

// ----------------------------------------------------------------------------------- targets

pub(super) async fn list_targets(State(state): State<AdminState>) -> Response {
    flatten(list_targets_inner(&state).await)
}

async fn list_targets_inner(state: &AdminState) -> Reply {
    let rows = db::list_targets(state.auth.db()?)
        .await
        .map_err(db_failed)?;
    let rows: Vec<Value> = rows.iter().map(target_json).collect();
    Ok(json_reply(StatusCode::OK, &json!({"targets": rows})))
}

/// What a create / update body says about the secret.
enum SecretInput {
    Absent,
    Clear,
    Set(String),
}

struct TargetBody {
    name: String,
    kind: Option<String>,
    config: Value,
    secret: SecretInput,
}

fn parse_target_body(headers: &HeaderMap, body: &Bytes) -> Result<TargetBody, Response> {
    let doc: Map<String, Value> = parse_json(headers, body)?;
    if let Some(extra) = doc
        .keys()
        .find(|k| !["name", "kind", "config", "secret"].contains(&k.as_str()))
    {
        return Err(bad(&format!("unknown field {extra:?}")));
    }
    let name = doc
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("name is required"))?
        .trim()
        .to_owned();
    name_ok(&name)?;
    let secret = match doc.get("secret") {
        None => SecretInput::Absent,
        Some(Value::Null) => SecretInput::Clear,
        Some(Value::String(s)) if s.is_empty() => SecretInput::Clear,
        Some(Value::String(s))
            if s.chars().count() <= SECRET_MAX && !s.chars().any(char::is_control) =>
        {
            SecretInput::Set(s.clone())
        }
        Some(_) => {
            return Err(bad(
                "secret must be text of at most 512 printable characters",
            ));
        }
    };
    Ok(TargetBody {
        name,
        kind: doc.get("kind").and_then(Value::as_str).map(str::to_owned),
        config: doc.get("config").cloned().unwrap_or_else(|| json!({})),
        secret,
    })
}

fn check_signing(kind: &str, config: &Value, has_secret: bool) -> Result<(), Response> {
    if kind == "webhook" && config.get("sign").and_then(Value::as_bool) == Some(true) && !has_secret
    {
        return Err(bad("signing needs a secret"));
    }
    Ok(())
}

pub(super) async fn create_target(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(create_target_inner(&state, &me, &headers, &body).await)
}

async fn create_target_inner(
    state: &AdminState,
    me: &Principal,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let pool = state.auth.db()?;
    let req = parse_target_body(headers, body)?;
    let kind = req.kind.as_deref().ok_or_else(|| bad("kind is required"))?;
    let config = target::normalize(kind, &req.config, None).map_err(|m| bad(&m))?;
    let secret = match &req.secret {
        SecretInput::Set(s) => Some(s.as_str()),
        _ => None,
    };
    if secret.is_some() && kind != "webhook" {
        return Err(bad("only webhooks take a secret"));
    }
    check_signing(kind, &config, secret.is_some())?;
    if db::list_targets(pool).await.map_err(db_failed)?.len() >= MAX_TARGETS {
        return Err(conflict("too_many", "too many targets"));
    }
    let created = db::insert_target(pool, &req.name, kind, &config, secret)
        .await
        .map_err(db_failed)?
        .ok_or_else(|| conflict("name_taken", "a target with that name exists"))?;
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "notification.target.create",
            Some(&created.name),
            Outcome::Ok,
            json!({"id": created.id, "kind": created.kind, "secret_set": created.secret.is_some()}),
        )
        .await;
    Ok(json_reply(
        StatusCode::CREATED,
        &json!({"target": target_json(&created)}),
    ))
}

pub(super) async fn update_target(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(update_target_inner(&state, &me, id, &headers, &body).await)
}

async fn update_target_inner(
    state: &AdminState,
    me: &Principal,
    id: i64,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let pool = state.auth.db()?;
    let req = parse_target_body(headers, body)?;
    let old = db::get_target(pool, id)
        .await
        .map_err(db_failed)?
        .ok_or_else(not_found)?;
    if req.kind.as_deref().is_some_and(|k| k != old.kind) {
        return Err(bad("the kind of a target cannot change"));
    }
    let config =
        target::normalize(&old.kind, &req.config, Some(&old.config)).map_err(|m| bad(&m))?;
    let change = match &req.secret {
        SecretInput::Absent => SecretChange::Keep,
        SecretInput::Clear => SecretChange::Clear,
        SecretInput::Set(s) => SecretChange::Set(s.clone()),
    };
    if old.kind != "webhook" && !matches!(change, SecretChange::Keep | SecretChange::Clear) {
        return Err(bad("only webhooks take a secret"));
    }
    let will_have_secret = match &change {
        SecretChange::Keep => old.secret.is_some(),
        SecretChange::Clear => false,
        SecretChange::Set(_) => true,
    };
    check_signing(&old.kind, &config, will_have_secret)?;
    let updated = match db::update_target(pool, id, &req.name, &config, &change)
        .await
        .map_err(db_failed)?
    {
        Updated::Ok(t) => t,
        Updated::Missing => return Err(not_found()),
        Updated::NameTaken => return Err(conflict("name_taken", "a target with that name exists")),
    };
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "notification.target.update",
            Some(&updated.name),
            Outcome::Ok,
            json!({"id": id, "secret_changed": !matches!(change, SecretChange::Keep)}),
        )
        .await;
    Ok(json_reply(
        StatusCode::OK,
        &json!({"target": target_json(&updated)}),
    ))
}

pub(super) async fn delete_target(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    flatten(delete_target_inner(&state, &me, id, &headers).await)
}

async fn delete_target_inner(
    state: &AdminState,
    me: &Principal,
    id: i64,
    headers: &HeaderMap,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let name = db::delete_target(state.auth.db()?, id)
        .await
        .map_err(db_failed)?
        .ok_or_else(not_found)?;
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "notification.target.delete",
            Some(&name),
            Outcome::Ok,
            json!({"id": id}),
        )
        .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Queue a test message; the worker sends it like any other (so the test exercises the real
/// path, including the SSRF guard) and the result shows up in the delivery log.
pub(super) async fn test_target(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    flatten(test_target_inner(&state, &me, id, &headers).await)
}

async fn test_target_inner(
    state: &AdminState,
    me: &Principal,
    id: i64,
    headers: &HeaderMap,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let pool = state.auth.db()?;
    let t = db::get_target(pool, id)
        .await
        .map_err(db_failed)?
        .ok_or_else(not_found)?;
    let message = Message::test(&t.name, now_ms(), None, None);
    let delivery_id = db::enqueue(
        pool,
        &NewDelivery {
            rule_id: None,
            rule_name: "(test)".to_owned(),
            target_id: t.id,
            target_name: t.name.clone(),
            event_id: None,
            message: serde_json::to_value(&message).unwrap_or(Value::Null),
        },
    )
    .await
    .map_err(db_failed)?;
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "notification.test",
            Some(&t.name),
            Outcome::Ok,
            json!({"target": t.id}),
        )
        .await;
    Ok(json_reply(
        StatusCode::ACCEPTED,
        &json!({"delivery_ids": delivery_id.into_iter().collect::<Vec<_>>()}),
    ))
}

// ------------------------------------------------------------------------------------- rules

pub(super) async fn list_rules(State(state): State<AdminState>) -> Response {
    flatten(list_rules_inner(&state).await)
}

async fn list_rules_inner(state: &AdminState) -> Reply {
    let rows = db::list_rules(state.auth.db()?, false)
        .await
        .map_err(db_failed)?;
    let rows: Vec<Value> = rows.iter().map(rule_json).collect();
    Ok(json_reply(StatusCode::OK, &json!({"rules": rows})))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct QuietBody {
    start: String,
    end: String,
    tz: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleBody {
    name: String,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    conditions: Option<Value>,
    #[serde(default)]
    cooldown_secs: i32,
    #[serde(default)]
    max_per_hour: Option<i32>,
    #[serde(default)]
    quiet: Option<QuietBody>,
    #[serde(default)]
    title_template: String,
    #[serde(default)]
    body_template: String,
    #[serde(default)]
    target_ids: Vec<i64>,
}

fn yes() -> bool {
    true
}

/// Validate a rule body and turn it into the stored form.
async fn rule_input(state: &AdminState, req: RuleBody) -> Result<RuleInput, Response> {
    let pool = state.auth.db()?;
    name_ok(&req.name)?;
    let blank = |s: Option<String>| s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let (source, kind) = (blank(req.source), blank(req.kind));
    if let Some(s) = &source
        && (s.len() > 128 || s.chars().any(|c| c.is_control() || c.is_whitespace()))
    {
        return Err(bad("source must be a plugin id or core"));
    }
    if let Some(k) = &kind
        && !decoder_events::KINDS.contains(&k.as_str())
        && !decoder_events::is_plugin_kind(k)
    {
        return Err(bad("kind is not a valid event type"));
    }
    let conditions = req.conditions.unwrap_or_else(|| json!([]));
    if !(0..=604_800).contains(&req.cooldown_secs) {
        return Err(bad("cooldown must be 0 to 604800 seconds"));
    }
    if req
        .max_per_hour
        .is_some_and(|m| !(1..=100_000).contains(&m))
    {
        return Err(bad("max_per_hour must be 1 to 100000"));
    }
    if req.target_ids.len() > MAX_RULE_TARGETS {
        return Err(bad("too many targets on one rule"));
    }
    let known: Vec<i64> = db::list_targets(pool)
        .await
        .map_err(db_failed)?
        .iter()
        .map(|t| t.id)
        .collect();
    if req.target_ids.iter().any(|id| !known.contains(id)) {
        return Err(bad("a selected target does not exist"));
    }
    let quiet = match req.quiet {
        None => None,
        Some(q) => {
            let (start_min, end_min) = (
                parse_clock(&q.start).ok_or_else(|| bad("quiet start must be HH:MM"))?,
                parse_clock(&q.end).ok_or_else(|| bad("quiet end must be HH:MM"))?,
            );
            if start_min == end_min {
                return Err(bad("quiet hours must not start and end at the same time"));
            }
            let zones = db::zone_offsets(pool, std::slice::from_ref(&q.tz))
                .await
                .map_err(db_failed)?;
            if !zones.contains_key(&q.tz) {
                return Err(bad(
                    "unknown time zone (use an IANA name like Europe/Berlin)",
                ));
            }
            Some(Quiet {
                start_min,
                end_min,
                tz: q.tz,
            })
        }
    };
    // Compile once to reject bad conditions and templates.
    rule::CompiledRule::new(
        0,
        &req.name,
        source.clone(),
        kind.clone(),
        &conditions,
        req.cooldown_secs,
        req.max_per_hour,
        None,
        &req.title_template,
        &req.body_template,
        Vec::new(),
    )
    .map_err(|m| bad(&m))?;
    let normalized = serde_json::from_value::<Vec<rule::ConditionDoc>>(conditions.clone())
        .ok()
        .and_then(|docs| serde_json::to_value(docs).ok())
        .unwrap_or(conditions);
    Ok(RuleInput {
        name: req.name.trim().to_owned(),
        enabled: req.enabled,
        source,
        kind,
        conditions: normalized,
        cooldown_secs: req.cooldown_secs,
        max_per_hour: req.max_per_hour,
        quiet,
        title_template: req.title_template,
        body_template: req.body_template,
        target_ids: req.target_ids,
    })
}

pub(super) async fn create_rule(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(create_rule_inner(&state, &me, &headers, &body).await)
}

async fn create_rule_inner(
    state: &AdminState,
    me: &Principal,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let pool = state.auth.db()?;
    let input = rule_input(state, parse_json(headers, body)?).await?;
    if db::list_rules(pool, false).await.map_err(db_failed)?.len() >= MAX_RULES {
        return Err(conflict("too_many", "too many rules"));
    }
    let created = db::insert_rule(pool, &input)
        .await
        .map_err(db_failed)?
        .ok_or_else(|| conflict("name_taken", "a rule with that name exists"))?;
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "notification.rule.create",
            Some(&created.data.name),
            Outcome::Ok,
            json!({"id": created.id, "enabled": created.data.enabled, "targets": created.data.target_ids}),
        )
        .await;
    Ok(json_reply(
        StatusCode::CREATED,
        &json!({"rule": rule_json(&created)}),
    ))
}

pub(super) async fn update_rule(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(update_rule_inner(&state, &me, id, &headers, &body).await)
}

async fn update_rule_inner(
    state: &AdminState,
    me: &Principal,
    id: i64,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let pool = state.auth.db()?;
    let input = rule_input(state, parse_json(headers, body)?).await?;
    let updated = match db::update_rule(pool, id, &input).await.map_err(db_failed)? {
        Updated::Ok(r) => r,
        Updated::Missing => return Err(not_found()),
        Updated::NameTaken => return Err(conflict("name_taken", "a rule with that name exists")),
    };
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "notification.rule.update",
            Some(&updated.data.name),
            Outcome::Ok,
            json!({"id": id, "enabled": updated.data.enabled, "targets": updated.data.target_ids}),
        )
        .await;
    Ok(json_reply(
        StatusCode::OK,
        &json!({"rule": rule_json(&updated)}),
    ))
}

pub(super) async fn delete_rule(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    flatten(delete_rule_inner(&state, &me, id, &headers).await)
}

async fn delete_rule_inner(
    state: &AdminState,
    me: &Principal,
    id: i64,
    headers: &HeaderMap,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let name = db::delete_rule(state.auth.db()?, id)
        .await
        .map_err(db_failed)?
        .ok_or_else(not_found)?;
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "notification.rule.delete",
            Some(&name),
            Outcome::Ok,
            json!({"id": id}),
        )
        .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Queue the rule's own message (its templates over a sample event) to each of its targets.
pub(super) async fn test_rule(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    flatten(test_rule_inner(&state, &me, id, &headers).await)
}

async fn test_rule_inner(
    state: &AdminState,
    me: &Principal,
    id: i64,
    headers: &HeaderMap,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let pool = state.auth.db()?;
    let stored = db::get_rule(pool, id)
        .await
        .map_err(db_failed)?
        .ok_or_else(not_found)?;
    if stored.data.target_ids.is_empty() {
        return Err(conflict("no_targets", "the rule has no targets"));
    }
    let compiled = rule::CompiledRule::new(
        stored.id,
        &stored.data.name,
        stored.data.source.clone(),
        stored.data.kind.clone(),
        &json!([]),
        0,
        None,
        None,
        &stored.data.title_template,
        &stored.data.body_template,
        Vec::new(),
    )
    .map_err(|m| bad(&m))?;
    let mut message = Message::test(
        &stored.data.name,
        now_ms(),
        stored.data.kind.as_deref(),
        stored.data.source.as_deref(),
    );
    let (title, body) = compiled.render(&rule::context(&message.event));
    message.title = format!("[test] {title}");
    message.body = body;
    let targets: HashMap<i64, String> = db::list_targets(pool)
        .await
        .map_err(db_failed)?
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();
    let mut ids = Vec::new();
    for target_id in &stored.data.target_ids {
        let Some(name) = targets.get(target_id) else {
            continue;
        };
        let queued = db::enqueue(
            pool,
            &NewDelivery {
                rule_id: Some(stored.id),
                rule_name: format!("{} (test)", stored.data.name),
                target_id: *target_id,
                target_name: name.clone(),
                event_id: None,
                message: serde_json::to_value(&message).unwrap_or(Value::Null),
            },
        )
        .await
        .map_err(db_failed)?;
        ids.extend(queued);
    }
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "notification.test",
            Some(&stored.data.name),
            Outcome::Ok,
            json!({"rule": stored.id}),
        )
        .await;
    Ok(json_reply(
        StatusCode::ACCEPTED,
        &json!({"delivery_ids": ids}),
    ))
}

// ---------------------------------------------------------------------------- delivery log

pub(super) async fn list_deliveries(
    State(state): State<AdminState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    flatten(list_deliveries_inner(&state, &query).await)
}

async fn list_deliveries_inner(state: &AdminState, query: &HashMap<String, String>) -> Reply {
    let pool = state.auth.db()?;
    let limit = match query.get("limit") {
        None => 50,
        Some(v) => v
            .parse::<i64>()
            .ok()
            .filter(|n| (1..=200).contains(n))
            .ok_or_else(|| bad("limit must be 1 to 200"))?,
    };
    let before = match query.get("before") {
        None => None,
        Some(v) => Some(
            v.parse::<i64>()
                .map_err(|_| bad("before must be a delivery id"))?,
        ),
    };
    let status = match query.get("status").map(String::as_str) {
        None | Some("") => None,
        Some(s @ ("pending" | "sent" | "failed")) => Some(s),
        Some(_) => return Err(bad("status must be pending, sent or failed")),
    };
    let rule_id = match query.get("rule") {
        None => None,
        Some(v) => Some(
            v.parse::<i64>()
                .map_err(|_| bad("rule must be a rule id"))?,
        ),
    };
    let rows = db::list_deliveries(pool, limit + 1, before, status, rule_id)
        .await
        .map_err(db_failed)?;
    let more = rows.len() > usize::try_from(limit).unwrap_or(0);
    let rows: Vec<Value> = rows
        .iter()
        .take(usize::try_from(limit).unwrap_or(0))
        .map(delivery_json)
        .collect();
    let next_before = if more {
        rows.last().and_then(|r| r["id"].as_i64())
    } else {
        None
    };
    Ok(json_reply(
        StatusCode::OK,
        &json!({"deliveries": rows, "next_before": next_before}),
    ))
}

// ---------------------------------------------------------------------- own push subscription

fn own_id(me: &Principal) -> Result<i64, Response> {
    match me.actor {
        Actor::User(id) => Ok(id),
        Actor::Guest(_) => Err(crate::auth::error(StatusCode::FORBIDDEN, "forbidden", None)),
    }
}

fn subscription_json(s: &db::Subscription) -> Value {
    json!({"id": s.id, "label": s.label, "created_at_ms": s.created_at_ms})
}

/// The stored VAPID key pair, created on first use.
async fn vapid(pool: &superhet_db::PgPool) -> Result<db::Vapid, Response> {
    if let Some(found) = db::get_vapid(pool).await.map_err(db_failed)? {
        return Ok(found);
    }
    let (private_pkcs8, public_key) = push::generate_vapid().map_err(|m| {
        tracing::warn!(error = %m, "generating the VAPID key failed");
        crate::auth::error(StatusCode::INTERNAL_SERVER_ERROR, "internal", None)
    })?;
    db::ensure_vapid(
        pool,
        &db::Vapid {
            private_pkcs8,
            public_key,
        },
    )
    .await
    .map_err(db_failed)
}

pub(super) async fn my_push(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
) -> Response {
    flatten(my_push_inner(&state, &me).await)
}

async fn my_push_inner(state: &AdminState, me: &Principal) -> Reply {
    let pool = state.auth.db()?;
    let uid = own_id(me)?;
    let key = vapid(pool).await?;
    let subs = db::list_subscriptions_of(pool, uid)
        .await
        .map_err(db_failed)?;
    let subs: Vec<Value> = subs.iter().map(subscription_json).collect();
    Ok(json_reply(
        StatusCode::OK,
        &json!({"public_key": key.public_key, "subscriptions": subs}),
    ))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Keys {
    p256dh: String,
    auth: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SubscribeBody {
    endpoint: String,
    keys: Keys,
    #[serde(default)]
    label: Option<String>,
}

pub(super) async fn subscribe(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flatten(subscribe_inner(&state, &me, &headers, &body).await)
}

async fn subscribe_inner(
    state: &AdminState,
    me: &Principal,
    headers: &HeaderMap,
    body: &Bytes,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let pool = state.auth.db()?;
    let uid = own_id(me)?;
    let req: SubscribeBody = parse_json(headers, body)?;
    push::validate_subscription(&req.endpoint, &req.keys.p256dh, &req.keys.auth)
        .map_err(|m| bad(&m))?;
    let label = req
        .label
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty());
    if label
        .as_ref()
        .is_some_and(|l| l.chars().count() > NAME_MAX || l.chars().any(char::is_control))
    {
        return Err(bad("label must be at most 80 printable characters"));
    }
    match db::upsert_subscription(
        pool,
        uid,
        &req.endpoint,
        &req.keys.p256dh,
        &req.keys.auth,
        label.as_deref(),
    )
    .await
    .map_err(db_failed)?
    {
        db::Subscribed::Ok(sub) => {
            state
                .auth
                .audit()
                .log(
                    &me.who(),
                    "push.subscribe",
                    Some(&me.name),
                    Outcome::Ok,
                    json!({"id": sub.id}),
                )
                .await;
            Ok(json_reply(
                StatusCode::CREATED,
                &json!({"subscription": subscription_json(&sub)}),
            ))
        }
        db::Subscribed::TooMany => Err(conflict("too_many", "too many browsers; remove one first")),
        db::Subscribed::OwnedByOther => Err(conflict(
            "endpoint_in_use",
            "that browser is subscribed by another account",
        )),
    }
}

pub(super) async fn unsubscribe(
    State(state): State<AdminState>,
    Extension(me): Extension<Principal>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    flatten(unsubscribe_inner(&state, &me, id, &headers).await)
}

async fn unsubscribe_inner(
    state: &AdminState,
    me: &Principal,
    id: i64,
    headers: &HeaderMap,
) -> Reply {
    guard_origin(&state.auth, headers)?;
    let uid = own_id(me)?;
    if !db::delete_subscription(state.auth.db()?, uid, id)
        .await
        .map_err(db_failed)?
    {
        return Err(not_found());
    }
    state
        .auth
        .audit()
        .log(
            &me.who(),
            "push.unsubscribe",
            Some(&me.name),
            Outcome::Ok,
            json!({"id": id}),
        )
        .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}
