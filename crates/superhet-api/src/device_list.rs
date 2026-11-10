//! Device discovery: `GET /api/v1/devices` lists every device daemon the bus knows about, live
//! or last known. Protocol: `docs/api-devices.md`.
//!
//! A device is "known" while the JetStream KV bucket `STATE` holds `dev.<id>` (published state,
//! removed on graceful shutdown, left behind by a crash) or `restore.<id>` (kept across
//! restarts). Each one is asked for its state with a `ctl.state` request: an answer means
//! online; no answer falls back to the stored document and reports the device offline.

use std::collections::BTreeSet;
use std::time::Duration;

use async_nats::{Client, jetstream};
use axum::{Extension, Router, extract::State, http::StatusCode, response::Response, routing::get};
use futures_util::{StreamExt, future::join_all};
use serde_json::{Value, json};
use superhet_bus::subjects;

use crate::access::Principal;
use crate::device_access::{DevicePolicy, Need, Subject};
use crate::devices::{
    DevicesState, connected, ctl_request, failure, json as json_response, read_kv,
};

/// How long reading the key list of the state bucket may take.
const KEYS_TIMEOUT: Duration = Duration::from_secs(2);
/// Most devices listed (a sanity bound against a flooded bucket).
const MAX_DEVICES: usize = 64;

/// Routes of the device list, behind the authentication hook.
pub fn router(state: DevicesState) -> Router {
    Router::new()
        .route("/api/v1/devices", get(list_devices))
        .with_state(state)
}

/// Device ids named by the keys of the state bucket (`dev.<id>` and `restore.<id>`).
fn ids_of_keys(keys: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    keys.into_iter()
        .filter_map(|key| {
            let id = key
                .strip_prefix("dev.")
                .or_else(|| key.strip_prefix("restore."))?;
            subjects::state_key(id).ok().map(|_| id.to_owned())
        })
        .collect()
}

async fn known_ids(client: &Client) -> Option<BTreeSet<String>> {
    let lookup = async {
        let store = jetstream::new(client.clone())
            .get_key_value(subjects::STATE_BUCKET)
            .await
            .ok()?;
        let mut keys = store.keys().await.ok()?;
        let mut out = Vec::new();
        while let Some(key) = keys.next().await {
            out.push(key.ok()?);
        }
        Some(ids_of_keys(out))
    };
    tokio::time::timeout(KEYS_TIMEOUT, lookup)
        .await
        .ok()
        .flatten()
}

/// The list entry of one device from its state document. Only presentation fields leave the
/// API here: no VFOs, no lock holders.
fn entry(id: &str, doc: &Value, online: bool) -> Value {
    let mut item = json!({
        "id": id,
        "online": online,
        "driver": doc.get("driver").cloned().unwrap_or(Value::Null),
        "center_hz": doc.get("center_hz").cloned().unwrap_or(Value::Null),
        "sample_rate": doc.get("sample_rate").cloned().unwrap_or(Value::Null),
    });
    for key in ["label", "tuner"] {
        if let Some(v) = doc.get(key).filter(|v| v.is_string()) {
            item[key] = v.clone();
        }
    }
    item
}

async fn describe(client: &Client, id: String) -> Value {
    let live = match subjects::ctl(&id, "state") {
        Ok(subject) => ctl_request(client, subject, Vec::new()).await.ok(),
        Err(_) => None,
    };
    if let Some(mut reply) = live
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(|reply| reply["ok"] == true && reply["state"].is_object())
    {
        return entry(&id, &reply["state"].take(), true);
    }
    for key in [subjects::state_key(&id), subjects::restore_key(&id)]
        .into_iter()
        .flatten()
    {
        if let Some(doc) = read_kv(client, &key)
            .await
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(Value::is_object)
        {
            return entry(&id, &doc, false);
        }
    }
    entry(&id, &Value::Null, false)
}

/// The ids `subject` may see, each with whether it may also control the device (ADR-0023).
fn visible(
    ids: BTreeSet<String>,
    table: &std::collections::HashMap<String, DevicePolicy>,
    subject: &Subject,
) -> Vec<(String, bool)> {
    let open = DevicePolicy::default();
    ids.into_iter()
        .filter_map(|id| {
            let policy = table.get(&id).unwrap_or(&open);
            policy
                .allows(subject, Need::View)
                .then(|| (id, policy.allows(subject, Need::Control)))
        })
        .collect()
}

async fn list_devices(
    State(state): State<DevicesState>,
    Extension(principal): Extension<Principal>,
) -> Response {
    let client = match connected(state.nats.as_ref()) {
        Ok(client) => client,
        Err(e) => return e.into_response(),
    };
    let Some(ids) = known_ids(client).await else {
        return failure(
            StatusCode::BAD_GATEWAY,
            "bus",
            "device registry unavailable",
        );
    };
    let Ok(table) = state.auth.device_policies().table().await else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "device policy unavailable",
        );
    };
    let ids = visible(ids, &table, &Subject::from(&principal));
    let devices = join_all(
        ids.into_iter()
            .take(MAX_DEVICES)
            .map(|(id, control)| async move {
                let mut item = describe(client, id).await;
                item["access"] = json!({"view": true, "control": control});
                item
            }),
    )
    .await;
    json_response(
        StatusCode::OK,
        json!({"v": 1, "devices": devices}).to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[test]
    fn ids_come_from_both_key_kinds_once() {
        let ids = ids_of_keys(
            [
                "dev.dev0",
                "restore.dev0",
                "restore.rtl1",
                "other.x",
                "dev.a.b",
                "dev.",
            ]
            .map(str::to_owned),
        );
        assert_eq!(ids.into_iter().collect::<Vec<_>>(), ["dev0", "rtl1"]);
    }

    #[test]
    fn entry_keeps_presentation_fields_only() {
        let doc = json!({
            "device": "dev1", "label": "Roof", "driver": "rtlsdr", "tuner": "R820T",
            "center_hz": 100_000_000, "sample_rate": 2_048_000,
            "vfos": [{"id": 1}], "locks": [{"holder": "u:1"}]
        });
        let item = entry("dev1", &doc, true);
        assert_eq!(item["label"], "Roof");
        assert_eq!(item["online"], true);
        assert!(item.get("vfos").is_none() && item.get("locks").is_none());
        let bare = entry("dev2", &Value::Null, false);
        assert_eq!(bare["online"], false);
        assert!(bare.get("label").is_none());
        assert!(bare["driver"].is_null());
    }

    #[test]
    fn the_list_holds_only_devices_the_caller_may_see() {
        use crate::access::Role;
        let ids = || ["dev0", "private", "ops", "ro"].map(str::to_owned).into();
        let table = std::collections::HashMap::from([
            (
                "private".to_owned(),
                DevicePolicy {
                    allowed_users: vec![7],
                    ..DevicePolicy::default()
                },
            ),
            (
                "ops".to_owned(),
                DevicePolicy {
                    view_role: Role::Operator,
                    ..DevicePolicy::default()
                },
            ),
            (
                "ro".to_owned(),
                DevicePolicy {
                    control_role: Role::Operator,
                    ..DevicePolicy::default()
                },
            ),
        ]);
        let subject = |guest, user_id, role| Subject {
            guest,
            user_id,
            role,
        };
        let listener = visible(ids(), &table, &subject(false, Some(1), Role::Listener));
        assert_eq!(
            listener,
            [("dev0".to_owned(), true), ("ro".to_owned(), false)]
        );
        let listed = visible(ids(), &table, &subject(false, Some(7), Role::Listener));
        assert_eq!(
            listed.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            ["dev0", "private", "ro"]
        );
        let guest = visible(ids(), &table, &subject(true, None, Role::Listener));
        assert_eq!(guest.len(), 2);
        let operator = visible(ids(), &table, &subject(false, Some(2), Role::Operator));
        assert_eq!(
            operator
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["dev0", "ops", "ro"]
        );
        let admin = visible(ids(), &table, &subject(false, Some(3), Role::Admin));
        assert_eq!(admin.len(), 4);
        assert!(admin.iter().all(|(_, control)| *control));
    }

    #[tokio::test]
    async fn without_a_bus_the_list_is_unavailable() {
        let app = crate::access::as_test_principal(router(DevicesState::for_tests()));
        let response = app
            .oneshot(
                Request::get("/api/v1/devices")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
