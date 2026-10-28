//! Prometheus metrics of the plugin host (`GET /metrics` on the internal port 8075).
//!
//! Event counters are process globals bumped by the supervisor and the input path; the gauges
//! (instances by state, claims, event age) are derived from the instance snapshots at scrape
//! time, so they never go stale. Instance labels are bounded by the number of running
//! instances, which is small on a single-user receiver.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use superhet_bus::metrics::{Exposition, Kind};
use superhet_plugin_protocol::ctl::{InstanceInfo, InstanceState};

/// Most instances listed with their own label set (defence against a label explosion).
const MAX_LABELLED: usize = 64;

static CRASHES: AtomicU64 = AtomicU64::new(0);
static RESTARTS: AtomicU64 = AtomicU64::new(0);
static INPUT_CHUNKS: AtomicU64 = AtomicU64::new(0);
static INPUT_BYTES: AtomicU64 = AtomicU64::new(0);
static INPUT_DROPPED: AtomicU64 = AtomicU64::new(0);

/// Counts a plugin process that ended unexpectedly.
pub(crate) fn crashed() {
    CRASHES.fetch_add(1, Ordering::Relaxed);
}

/// Counts an automatic start after a crash.
pub(crate) fn restarted() {
    RESTARTS.fetch_add(1, Ordering::Relaxed);
}

/// Counts one chunk of `bytes` sample bytes delivered to a plugin (tap frames).
pub(crate) fn input_chunk(bytes: usize) {
    INPUT_CHUNKS.fetch_add(1, Ordering::Relaxed);
    INPUT_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
}

/// Counts a chunk dropped because the plugin read its input too slowly.
pub(crate) fn input_dropped() {
    INPUT_DROPPED.fetch_add(1, Ordering::Relaxed);
}

const STATES: [(InstanceState, &str); 5] = [
    (InstanceState::Starting, "starting"),
    (InstanceState::Running, "running"),
    (InstanceState::Backoff, "backoff"),
    (InstanceState::Error, "error"),
    (InstanceState::Stopped, "stopped"),
];

/// The text exposition for `instances` at `now_ms` (Unix milliseconds).
#[must_use]
pub fn render(instances: &[InstanceInfo], now_ms: u64) -> String {
    let mut e = Exposition::new();
    e.family(
        "superhet_pluginhost_instances",
        "Plugin instances by lifecycle state.",
        Kind::Gauge,
    );
    for (state, name) in STATES {
        let count = instances.iter().filter(|i| i.state == state).count();
        e.sample_u64(
            "superhet_pluginhost_instances",
            &[("state", name)],
            count as u64,
        );
    }

    e.family(
        "superhet_pluginhost_claim_held",
        "1 while an exclusive plugin instance holds the device (running and its input is served).",
        Kind::Gauge,
    );
    for instance in instances.iter().filter(|i| i.exclusive).take(MAX_LABELLED) {
        let held = instance.state == InstanceState::Running && instance.input_error.is_none();
        e.sample_u64(
            "superhet_pluginhost_claim_held",
            &[
                ("instance", &instance.id),
                ("device", instance.device.as_deref().unwrap_or("")),
            ],
            u64::from(held),
        );
    }

    e.family(
        "superhet_pluginhost_last_event_age_seconds",
        "Seconds since the instance's last valid event (instances that never sent one are absent).",
        Kind::Gauge,
    );
    for instance in instances.iter().take(MAX_LABELLED) {
        if let Some(at) = instance.last_event_unix_ms {
            e.sample_f64(
                "superhet_pluginhost_last_event_age_seconds",
                &[("instance", &instance.id), ("plugin", &instance.plugin_id)],
                now_ms.saturating_sub(at) as f64 / 1000.0,
            );
        }
    }

    e.single_u64(
        "superhet_pluginhost_crashes_total",
        "Plugin processes that ended unexpectedly.",
        Kind::Counter,
        CRASHES.load(Ordering::Relaxed),
    );
    e.single_u64(
        "superhet_pluginhost_restarts_total",
        "Automatic starts after a crash.",
        Kind::Counter,
        RESTARTS.load(Ordering::Relaxed),
    );
    e.single_u64(
        "superhet_pluginhost_tap_frames_total",
        "Sample chunks delivered to plugins from the device daemons.",
        Kind::Counter,
        INPUT_CHUNKS.load(Ordering::Relaxed),
    );
    e.single_u64(
        "superhet_pluginhost_tap_bytes_total",
        "Sample bytes delivered to plugins from the device daemons.",
        Kind::Counter,
        INPUT_BYTES.load(Ordering::Relaxed),
    );
    e.single_u64(
        "superhet_pluginhost_tap_dropped_frames_total",
        "Sample chunks dropped because a plugin read its input too slowly.",
        Kind::Counter,
        INPUT_DROPPED.load(Ordering::Relaxed),
    );
    e.finish()
}

/// [`render`] at the current time.
#[must_use]
pub fn render_now(instances: &[InstanceInfo]) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    render(instances, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(id: &str, state: InstanceState, exclusive: bool) -> InstanceInfo {
        InstanceInfo {
            id: id.to_owned(),
            plugin_id: "dab".to_owned(),
            device: Some("rtl0".to_owned()),
            vfo: None,
            freq_hz: None,
            config: serde_json::Value::Null,
            exclusive,
            state,
            pid: None,
            restarts: 0,
            crashes_in_window: 0,
            retry_in_ms: None,
            last_error: None,
            events_ok: 0,
            events_invalid: 0,
            input_dropped: 0,
            input_error: None,
            audio_vfo: None,
            updated_unix_ms: 0,
            input_bytes: 0,
            input_level_db: None,
            last_input_unix_ms: None,
            last_event_unix_ms: None,
            created_unix_ms: 0,
            reconfigures: 0,
        }
    }

    #[test]
    fn gauges_come_from_the_instances_and_counters_from_events() {
        crashed();
        restarted();
        input_chunk(100);
        input_dropped();
        let mut held = instance("i-dab", InstanceState::Running, true);
        held.last_event_unix_ms = Some(7_000);
        let mut starved = instance("i-adsb", InstanceState::Running, true);
        starved.input_error = Some("device is claimed".to_owned());
        let list = [
            held,
            starved,
            instance("i-pocsag", InstanceState::Backoff, false),
        ];
        let text = render(&list, 10_500);
        for line in [
            "superhet_pluginhost_instances{state=\"running\"} 2",
            "superhet_pluginhost_instances{state=\"backoff\"} 1",
            "superhet_pluginhost_instances{state=\"error\"} 0",
            "superhet_pluginhost_claim_held{instance=\"i-dab\",device=\"rtl0\"} 1",
            "superhet_pluginhost_claim_held{instance=\"i-adsb\",device=\"rtl0\"} 0",
            "superhet_pluginhost_last_event_age_seconds{instance=\"i-dab\",plugin=\"dab\"} 3.5",
        ] {
            assert!(text.contains(line), "missing {line}\n{text}");
        }
        assert!(!text.contains("i-pocsag\",device"), "not exclusive");
        assert!(text.contains("# TYPE superhet_pluginhost_crashes_total counter"));
        assert!(text.contains("# TYPE superhet_pluginhost_tap_bytes_total counter"));
        assert!(!text.contains("superhet_pluginhost_crashes_total 0\n"));
    }
}
