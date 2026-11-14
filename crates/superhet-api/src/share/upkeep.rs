//! Background upkeep of share links: end the connections of links that expired or vanished, and
//! delete VFOs a crashed API left behind (listeners of fixed links own one each).

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{Value, json};
use superhet_bus::subjects;

use super::links::Status;
use crate::auth::AuthState;
use crate::devices::{DevicesState, call_daemon, ctl_state};
use crate::registry::now_ms;

/// How often connections are checked against their link.
const INTERVAL: Duration = Duration::from_secs(5);
/// Every this many passes the devices are swept for orphaned VFOs.
const SWEEP_EVERY: u32 = 12;
/// Prefix of the VFO owners of share listeners.
const OWNER_PREFIX: &str = "s:";

/// End connections whose link is no longer active (expiry passes without any admin action).
async fn end_stale(auth: &AuthState) {
    for id in auth.share_live().links() {
        let usable = match auth.share_links().by_id(id).await {
            Ok(Some(link)) => link.status(now_ms()) == Status::Active,
            Ok(None) => false,
            // Unreadable table: keep what is open, the next pass tries again.
            Err(_) => true,
        };
        if !usable {
            super::kick_link(auth, id);
        }
    }
}

/// Delete VFOs owned by share listeners that no live connection holds.
async fn sweep_orphans(auth: &AuthState, devices: &DevicesState) {
    let Ok(links) = auth.share_links().all().await else {
        return;
    };
    let wanted: BTreeSet<&str> = links.iter().map(|l| l.device.as_str()).collect();
    for device in wanted {
        let Ok(doc) = ctl_state(devices, device).await else {
            continue;
        };
        let orphans: Vec<u64> = doc
            .get("vfos")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter(|v| {
                v.get("owner").and_then(Value::as_str).is_some_and(|o| {
                    o.starts_with(OWNER_PREFIX) && !auth.share_live().owner_is_live(o)
                })
            })
            .filter_map(|v| v.get("id").and_then(Value::as_u64))
            .collect();
        for id in orphans {
            let Ok(subject) = subjects::ctl_vfo(device, "delete") else {
                continue;
            };
            let payload = json!({"v": 1, "id": id}).to_string().into_bytes();
            match call_daemon(devices, subject, payload).await {
                Ok(_) => tracing::info!(device, id, "deleted an orphaned share listener VFO"),
                Err(_) => tracing::debug!(device, id, "could not delete an orphaned share VFO"),
            }
        }
    }
}

/// Run the upkeep forever in the background.
pub fn spawn(auth: AuthState, devices: DevicesState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(INTERVAL);
        let mut pass = 0u32;
        loop {
            ticker.tick().await;
            end_stale(&auth).await;
            pass = pass.wrapping_add(1);
            if pass.is_multiple_of(SWEEP_EVERY) {
                sweep_orphans(&auth, &devices).await;
            }
        }
    });
}
