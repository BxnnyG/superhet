//! Background upkeep of guest sessions: expire idle guests, end all of them when guest mode is
//! switched off, and delete the VFOs of guests that left (an anonymous visitor has no identity
//! to come back to, so their channels must not keep burning CPU).

use std::time::Duration;

use serde_json::{Value, json};
use superhet_bus::{audit::Outcome, subjects};

use crate::audit::{ActorKind, Who};
use crate::auth::AuthState;
use crate::devices::{DevicesState, call_daemon, ctl_state};
use crate::registry::GuestSession;

/// How often the upkeep runs.
const INTERVAL: Duration = Duration::from_secs(2);

fn guest_who(guest: &GuestSession) -> Who {
    Who {
        kind: ActorKind::Guest,
        id: Some(guest.id.clone()),
        name: Some("guest".to_owned()),
        role: Some(superhet_db::users::Role::Listener),
        ip: guest.ip,
        peer: guest.peer,
        via: Vec::new(),
        user_agent: guest.user_agent.clone(),
    }
}

/// Delete every VFO of the given owners on every device the clients talked to.
pub async fn delete_vfos_of(devices: &DevicesState, owners: &[String]) {
    for device in devices.auth.registry().devices() {
        let Ok(doc) = ctl_state(devices, &device).await else {
            continue;
        };
        let ids: Vec<u64> = doc
            .get("vfos")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter(|v| {
                v.get("owner")
                    .and_then(Value::as_str)
                    .is_some_and(|o| owners.iter().any(|x| x == o))
            })
            .filter_map(|v| v.get("id").and_then(Value::as_u64))
            .collect();
        for id in ids {
            let Ok(subject) = subjects::ctl_vfo(&device, "delete") else {
                continue;
            };
            let payload = json!({"v": 1, "id": id}).to_string().into_bytes();
            if call_daemon(devices, subject, payload).await.is_err() {
                tracing::debug!(device, id, "could not delete the VFO of a departed guest");
            }
        }
    }
}

/// One upkeep pass.
pub async fn run_once(auth: &AuthState, devices: &DevicesState) {
    let policy = auth.settings().guest_policy().await;
    let (ended, reason) = if policy.enabled {
        (
            auth.registry()
                .guests_expire(Duration::from_secs(policy.idle_timeout_secs)),
            "idle",
        )
    } else {
        (auth.registry().guests_clear(), "disabled")
    };
    for guest in &ended {
        auth.audit()
            .log(
                &guest_who(guest),
                "guest.session_end",
                None,
                Outcome::Ok,
                json!({"reason": reason}),
            )
            .await;
    }
    let owners = auth.registry().take_cleanups();
    if !owners.is_empty() {
        delete_vfos_of(devices, &owners).await;
    }
}

/// Run [`run_once`] forever in the background.
pub fn spawn(auth: AuthState, devices: DevicesState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(INTERVAL);
        loop {
            ticker.tick().await;
            run_once(&auth, &devices).await;
        }
    });
}
