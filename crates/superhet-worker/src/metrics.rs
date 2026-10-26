//! Prometheus metrics of the worker (`GET /metrics` on the internal port 8076).
//!
//! Counters are process globals; the event kinds are a small fixed set produced by the
//! recorder, so the label cardinality is bounded.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use superhet_bus::metrics::{Exposition, Kind};

/// Upper bound of distinct event kinds tracked (defence against an unexpected label explosion).
const MAX_KINDS: usize = 32;

static INSERTED: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());
static INSERT_ERRORS: AtomicU64 = AtomicU64::new(0);
static PRUNED: AtomicU64 = AtomicU64::new(0);

/// Counts a decoder event stored in the database.
pub fn event_inserted(kind: &str) {
    let mut map = INSERTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(count) = map.get_mut(kind) {
        *count += 1;
    } else if map.len() < MAX_KINDS {
        map.insert(kind.to_owned(), 1);
    }
}

/// Counts a decoder event that could not be stored.
pub fn insert_failed() {
    INSERT_ERRORS.fetch_add(1, Ordering::Relaxed);
}

/// Counts decoder events removed by the retention job.
pub fn events_pruned(count: u64) {
    PRUNED.fetch_add(count, Ordering::Relaxed);
}

/// The text exposition.
#[must_use]
pub fn render() -> String {
    let mut e = Exposition::new();
    e.family(
        "superhet_worker_decoder_events_inserted_total",
        "Decoder history events stored in PostgreSQL, by kind.",
        Kind::Counter,
    );
    let inserted = INSERTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    for (kind, count) in &inserted {
        e.sample_u64(
            "superhet_worker_decoder_events_inserted_total",
            &[("kind", kind)],
            *count,
        );
    }
    e.single_u64(
        "superhet_worker_decoder_event_insert_errors_total",
        "Decoder history events that could not be stored.",
        Kind::Counter,
        INSERT_ERRORS.load(Ordering::Relaxed),
    );
    e.single_u64(
        "superhet_worker_decoder_events_pruned_total",
        "Decoder history events removed by the retention job.",
        Kind::Counter,
        PRUNED.load(Ordering::Relaxed),
    );
    e.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_show_up_in_the_exposition() {
        event_inserted("metrics_test_kind");
        event_inserted("metrics_test_kind");
        insert_failed();
        events_pruned(3);
        let text = render();
        assert!(
            text.contains(
                "superhet_worker_decoder_events_inserted_total{kind=\"metrics_test_kind\"} 2"
            ),
            "{text}"
        );
        assert!(text.contains("# TYPE superhet_worker_decoder_event_insert_errors_total counter"));
        assert!(text.contains("superhet_worker_decoder_events_pruned_total 3"));
    }
}
