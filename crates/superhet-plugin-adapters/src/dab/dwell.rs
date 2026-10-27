//! How long to stay on a block: decides, from successive `mux.json` snapshots, whether a block
//! carries no ensemble (leave early) or whether its ensemble is complete and stable (announce it,
//! move on during a scan).

use std::time::Duration;

use super::mux::{Codec, Mux};

/// Timing of the decision.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Without an ensemble id after this long the block counts as empty.
    pub no_signal: Duration,
    /// The label set must stay unchanged this long before the ensemble counts as complete.
    pub stable: Duration,
    /// Give up waiting for completeness after this long and take what there is.
    pub max: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            no_signal: Duration::from_secs(5),
            stable: Duration::from_millis(2500),
            max: Duration::from_secs(15),
        }
    }
}

/// What the observations say so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Not decided yet.
    Wait,
    /// No ensemble on this block.
    Empty,
    /// An ensemble with its services (as far as they arrived).
    Found,
}

/// Judges one block; create a fresh one after every retune.
#[derive(Debug)]
pub struct Dwell {
    timing: Timing,
    signature: String,
    since: Duration,
}

/// What must stay equal for the list to count as stable: ensemble id and label plus the labels
/// of the audio services.
fn signature(mux: &Mux) -> String {
    let mut text = format!("{}|{}", mux.ensemble.eid, mux.ensemble.label);
    for service in mux.services.iter().filter(|s| s.codec != Codec::Data) {
        text.push('|');
        text.push_str(&service.sid_hex);
        text.push('=');
        text.push_str(&service.label);
    }
    text
}

/// Every audio service has its label and the ensemble has its own.
fn complete(mux: &Mux) -> bool {
    let mut audio = mux
        .services
        .iter()
        .filter(|s| s.codec != Codec::Data)
        .peekable();
    !mux.ensemble.label.is_empty() && audio.peek().is_some() && audio.all(|s| !s.label.is_empty())
}

impl Dwell {
    /// A fresh judge.
    #[must_use]
    pub fn new(timing: Timing) -> Self {
        Self {
            timing,
            signature: String::new(),
            since: Duration::ZERO,
        }
    }

    /// Feed a snapshot taken `elapsed` after the retune.
    pub fn observe(&mut self, elapsed: Duration, mux: &Mux) -> Verdict {
        if !mux.ensemble.known() {
            return if elapsed >= self.timing.no_signal {
                Verdict::Empty
            } else {
                Verdict::Wait
            };
        }
        let now = signature(mux);
        if now != self.signature {
            self.signature = now;
            self.since = elapsed;
        }
        let steady = elapsed.saturating_sub(self.since) >= self.timing.stable;
        if (complete(mux) && steady) || elapsed >= self.timing.max {
            if mux.services.is_empty() {
                Verdict::Empty
            } else {
                Verdict::Found
            }
        } else {
            Verdict::Wait
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dab::mux::parse;

    fn locked() -> Mux {
        parse(include_bytes!("../../tests/dab/mux-locked.json")).expect("fixture")
    }

    fn empty() -> Mux {
        parse(include_bytes!("../../tests/dab/mux-nosignal.json")).expect("fixture")
    }

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    #[test]
    fn a_block_without_ensemble_is_empty_after_the_no_signal_time() {
        let mut dwell = Dwell::new(Timing::default());
        assert_eq!(dwell.observe(secs(1.0), &empty()), Verdict::Wait);
        assert_eq!(dwell.observe(secs(4.9), &empty()), Verdict::Wait);
        assert_eq!(dwell.observe(secs(5.0), &empty()), Verdict::Empty);
    }

    #[test]
    fn a_complete_ensemble_is_found_once_it_stays_unchanged() {
        let mut dwell = Dwell::new(Timing::default());
        let mux = locked();
        assert_eq!(dwell.observe(secs(2.0), &mux), Verdict::Wait);
        assert_eq!(dwell.observe(secs(4.0), &mux), Verdict::Wait);
        assert_eq!(dwell.observe(secs(4.6), &mux), Verdict::Found);
    }

    #[test]
    fn a_growing_service_list_restarts_the_stability_timer() {
        let mut dwell = Dwell::new(Timing::default());
        let mut mux = locked();
        let full = mux.services.clone();
        mux.services.truncate(1);
        assert_eq!(dwell.observe(secs(2.0), &mux), Verdict::Wait);
        assert_eq!(dwell.observe(secs(4.0), &mux), Verdict::Wait);
        mux.services = full;
        // New service labels arrived at 4.4 s: stable again only 2.5 s later.
        assert_eq!(dwell.observe(secs(4.4), &mux), Verdict::Wait);
        assert_eq!(dwell.observe(secs(6.0), &mux), Verdict::Wait);
        assert_eq!(dwell.observe(secs(7.0), &mux), Verdict::Found);
    }

    #[test]
    fn missing_labels_are_waited_for_but_not_forever() {
        let mut dwell = Dwell::new(Timing::default());
        let mut mux = locked();
        mux.services[1].label.clear();
        assert_eq!(dwell.observe(secs(8.0), &mux), Verdict::Wait);
        assert_eq!(dwell.observe(secs(14.0), &mux), Verdict::Wait);
        assert_eq!(dwell.observe(secs(15.0), &mux), Verdict::Found);
    }

    #[test]
    fn an_ensemble_id_without_services_is_empty_in_the_end() {
        let mut dwell = Dwell::new(Timing::default());
        let mut mux = locked();
        mux.services.clear();
        assert_eq!(dwell.observe(secs(10.0), &mux), Verdict::Wait);
        assert_eq!(dwell.observe(secs(15.0), &mux), Verdict::Empty);
    }
}
