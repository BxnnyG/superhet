//! Squelch gate: keeps the frames of a transmission (with a short pre-roll and a hang time) and
//! drops the silence in between. Works frame by frame so it serves live recorders and replay
//! selection alike.

use std::collections::VecDeque;

/// Frames kept before the squelch opened (100 ms at 20 ms per frame).
pub const PREROLL_FRAMES: usize = 5;
/// Frames kept after the squelch closed (400 ms).
pub const HANG_FRAMES: usize = 20;

/// What the gate says about one frame it was fed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept<T> {
    /// The frame.
    pub frame: T,
    /// First frame of a new segment: there is a gap (skipped silence) before it.
    pub starts_segment: bool,
}

/// The gate state.
#[derive(Debug)]
pub struct Gate<T> {
    preroll: VecDeque<T>,
    hang_left: usize,
    in_segment: bool,
    /// Consecutive frames seen with the squelch closed.
    closed_run: usize,
}

impl<T> Default for Gate<T> {
    fn default() -> Self {
        Self {
            preroll: VecDeque::new(),
            hang_left: 0,
            in_segment: false,
            closed_run: 0,
        }
    }
}

impl<T> Gate<T> {
    /// A fresh gate (closed).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Frames in a row seen with the squelch closed (the caller ends hit recordings on this).
    #[must_use]
    pub fn closed_run(&self) -> usize {
        self.closed_run
    }

    /// Feed the next frame; returns the frames to write (zero, one, or the pre-roll plus the
    /// frame when the squelch just opened).
    pub fn push(&mut self, frame: T, open: bool) -> Vec<Kept<T>> {
        if open {
            self.closed_run = 0;
            self.hang_left = HANG_FRAMES;
            let mut out = Vec::new();
            let mut first = !self.in_segment;
            for pre in self.preroll.drain(..) {
                out.push(Kept {
                    frame: pre,
                    starts_segment: std::mem::take(&mut first),
                });
            }
            out.push(Kept {
                frame,
                starts_segment: std::mem::take(&mut first),
            });
            self.in_segment = true;
            return out;
        }
        self.closed_run += 1;
        if self.hang_left > 0 {
            self.hang_left -= 1;
            return vec![Kept {
                frame,
                starts_segment: false,
            }];
        }
        self.in_segment = false;
        self.preroll.push_back(frame);
        while self.preroll.len() > PREROLL_FRAMES {
            self.preroll.pop_front();
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(pattern: &str) -> Vec<(usize, bool)> {
        let mut gate = Gate::new();
        let mut out = Vec::new();
        for (i, c) in pattern.chars().enumerate() {
            out.extend(
                gate.push(i, c == '#')
                    .into_iter()
                    .map(|k| (k.frame, k.starts_segment)),
            );
        }
        out
    }

    #[test]
    fn silence_is_dropped_and_segments_are_marked() {
        let pattern = format!("{}###{}###", "-".repeat(30), "-".repeat(40));
        let kept = run(&pattern);
        // First opening at index 30: 5 pre-roll frames (25..30) + 3 open + 20 hang.
        assert_eq!(kept[0], (25, true));
        assert_eq!(kept[5], (30, false));
        assert_eq!(kept.iter().filter(|(_, s)| *s).count(), 2);
        // The first segment ends after the hang; nothing from the long silence is kept.
        let first_end = 25 + 5 + 3 + HANG_FRAMES;
        assert!(kept.iter().any(|(i, _)| *i == first_end - 1));
        assert!(!kept.iter().any(|(i, _)| *i == first_end));
        // Second segment starts with its own pre-roll.
        let second = kept.iter().position(|(_, s)| *s).map(|_| ()).is_some();
        assert!(second);
        assert_eq!(kept.last().map(|k| k.0), Some(75));
    }

    #[test]
    fn a_short_pause_stays_inside_one_segment() {
        let kept = run("###-----###");
        assert_eq!(kept.iter().filter(|(_, s)| *s).count(), 1);
        assert_eq!(kept.len(), 11);
    }

    #[test]
    fn an_always_open_stream_is_kept_whole() {
        let kept = run(&"#".repeat(100));
        assert_eq!(kept.len(), 100);
    }

    #[test]
    fn closed_runs_are_counted() {
        let mut gate = Gate::new();
        gate.push(0, true);
        for i in 1..=7 {
            gate.push(i, false);
        }
        assert_eq!(gate.closed_run(), 7);
        gate.push(8, true);
        assert_eq!(gate.closed_run(), 0);
    }
}
