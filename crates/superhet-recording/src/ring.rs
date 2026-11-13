//! Per-VFO ring buffer of the last seconds of encoded audio (instant replay).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::gate::Gate;

/// Shortest and longest configurable replay window (seconds).
pub const MIN_SECONDS: u32 = 5;
/// See [`MIN_SECONDS`].
pub const MAX_SECONDS: u32 = 600;
/// Default replay window.
pub const DEFAULT_SECONDS: u32 = 60;
/// Most rings the hub keeps; the quietest is dropped beyond that.
const MAX_RINGS: usize = 128;
/// A ring without a frame for this long is forgotten (the VFO is gone).
const STALE_NS: u64 = 60_000_000_000;
/// Memory cap per ring, whatever the window says.
const MAX_RING_BYTES: usize = 16 * 1024 * 1024;

/// One stored frame: the Opus packet as published, plus what the gate and the file need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayFrame {
    /// Capture time of the first sample, Unix nanoseconds.
    pub capture_unix_ns: u64,
    /// Channels of the packet (1 or 2).
    pub channels: u8,
    /// Squelch open (or no squelch).
    pub open: bool,
    /// The Opus packet.
    pub payload: Arc<[u8]>,
}

/// Clamp a configured window to the supported range.
#[must_use]
pub fn clamp_seconds(seconds: u32) -> u32 {
    seconds.clamp(MIN_SECONDS, MAX_SECONDS)
}

/// The last `seconds` of one VFO.
#[derive(Debug)]
pub struct Ring {
    frames: VecDeque<ReplayFrame>,
    window_ns: u64,
    bytes: usize,
}

impl Ring {
    /// A ring that keeps `seconds` of audio.
    #[must_use]
    pub fn new(seconds: u32) -> Self {
        Self {
            frames: VecDeque::new(),
            window_ns: u64::from(clamp_seconds(seconds)) * 1_000_000_000,
            bytes: 0,
        }
    }

    /// Store a frame and drop what fell out of the window.
    pub fn push(&mut self, frame: ReplayFrame) {
        self.bytes += frame.payload.len();
        let newest = frame.capture_unix_ns;
        self.frames.push_back(frame);
        while let Some(front) = self.frames.front() {
            let old = newest.saturating_sub(front.capture_unix_ns) > self.window_ns;
            if old || self.bytes > MAX_RING_BYTES {
                self.bytes -= front.payload.len();
                self.frames.pop_front();
            } else {
                break;
            }
        }
    }

    /// Number of stored frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the ring holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Bytes of audio stored.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    fn newest_ns(&self) -> u64 {
        self.frames.back().map_or(0, |f| f.capture_unix_ns)
    }

    /// The frames of the last `seconds` (at most the window). With `gated`, silence is cut with
    /// the usual pre-roll and hang time.
    #[must_use]
    pub fn last(&self, seconds: u32, gated: bool) -> Vec<ReplayFrame> {
        let span = u64::from(seconds)
            .saturating_mul(1_000_000_000)
            .min(self.window_ns);
        let from = self.newest_ns().saturating_sub(span);
        let recent = self.frames.iter().filter(|f| f.capture_unix_ns >= from);
        if !gated {
            return recent.cloned().collect();
        }
        let mut gate = Gate::new();
        let mut out = Vec::new();
        for frame in recent {
            let open = frame.open;
            out.extend(gate.push(frame.clone(), open).into_iter().map(|k| k.frame));
        }
        out
    }
}

/// The rings of all VFOs, keyed by device and VFO id.
#[derive(Debug)]
pub struct ReplayHub {
    seconds: u32,
    rings: HashMap<(String, u32), Ring>,
}

impl ReplayHub {
    /// A hub whose rings keep `seconds` of audio each.
    #[must_use]
    pub fn new(seconds: u32) -> Self {
        Self {
            seconds: clamp_seconds(seconds),
            rings: HashMap::new(),
        }
    }

    /// The configured window.
    #[must_use]
    pub fn seconds(&self) -> u32 {
        self.seconds
    }

    /// Store a frame of `(device, vfo)`.
    pub fn push(&mut self, device: &str, vfo: u32, frame: ReplayFrame) {
        let key = (device.to_owned(), vfo);
        if !self.rings.contains_key(&key) && self.rings.len() >= MAX_RINGS {
            self.expire(frame.capture_unix_ns);
            if self.rings.len() >= MAX_RINGS {
                return;
            }
        }
        self.rings
            .entry(key)
            .or_insert_with(|| Ring::new(self.seconds))
            .push(frame);
    }

    /// Drop rings that have not received a frame for a minute (relative to `now_ns`).
    pub fn expire(&mut self, now_ns: u64) {
        self.rings
            .retain(|_, ring| now_ns.saturating_sub(ring.newest_ns()) <= STALE_NS);
    }

    /// The last `seconds` of a VFO, or `None` when nothing is buffered.
    #[must_use]
    pub fn last(
        &self,
        device: &str,
        vfo: u32,
        seconds: u32,
        gated: bool,
    ) -> Option<Vec<ReplayFrame>> {
        let ring = self.rings.get(&(device.to_owned(), vfo))?;
        let frames = ring.last(seconds, gated);
        (!frames.is_empty()).then_some(frames)
    }

    /// Number of rings and bytes held (for the status endpoint).
    #[must_use]
    pub fn usage(&self) -> (usize, usize) {
        (self.rings.len(), self.rings.values().map(Ring::bytes).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(i: u64, open: bool) -> ReplayFrame {
        ReplayFrame {
            capture_unix_ns: i * 20_000_000,
            channels: 1,
            open,
            payload: Arc::from(vec![i as u8; 10]),
        }
    }

    #[test]
    fn old_frames_fall_out_of_the_window() {
        let mut ring = Ring::new(MIN_SECONDS);
        for i in 0..1000 {
            ring.push(frame(i, true));
        }
        // 5 s window = 250 frames (+1 for the inclusive edge).
        assert!((250..=251).contains(&ring.len()), "{}", ring.len());
        assert_eq!(ring.bytes(), ring.len() * 10);
        let all = ring.last(600, false);
        assert_eq!(all.len(), ring.len());
        assert_eq!(all.last().map(|f| f.payload[0]), Some(999u64 as u8));
    }

    #[test]
    fn last_n_seconds_selects_the_newest() {
        let mut ring = Ring::new(60);
        for i in 0..1000 {
            ring.push(frame(i, true));
        }
        let ten = ring.last(10, false);
        assert!((500..=501).contains(&ten.len()), "{}", ten.len());
        assert_eq!(
            ten.last().map(|f| f.capture_unix_ns),
            Some(999 * 20_000_000)
        );
    }

    #[test]
    fn gated_replay_cuts_silence() {
        let mut ring = Ring::new(60);
        for i in 0..500 {
            // One transmission of 1 s in the middle.
            ring.push(frame(i, (200..250).contains(&i)));
        }
        let gated = ring.last(60, true);
        assert!(gated.len() < 100, "{}", gated.len());
        assert!(gated.iter().any(|f| f.open));
        assert!(ring.last(60, false).len() == 500);
    }

    #[test]
    fn memory_is_capped_whatever_the_window() {
        let mut ring = Ring::new(MAX_SECONDS);
        for i in 0..100u64 {
            let mut f = frame(i, true);
            f.payload = Arc::from(vec![0u8; 1024 * 1024]);
            ring.push(f);
        }
        assert!(ring.bytes() <= MAX_RING_BYTES);
        assert!(!ring.is_empty());
    }

    #[test]
    fn the_hub_keeps_vfos_apart_and_forgets_stale_ones() {
        let mut hub = ReplayHub::new(60);
        hub.push("dev0", 1, frame(1, true));
        hub.push("dev0", 2, frame(2, true));
        hub.push("dev1", 1, frame(3, true));
        assert_eq!(hub.last("dev0", 1, 10, false).map(|f| f.len()), Some(1));
        assert!(hub.last("dev0", 9, 10, false).is_none());
        assert_eq!(hub.usage().0, 3);
        // Keep one fresh ring, let the others go stale.
        hub.push("dev0", 2, frame(10_000, true));
        hub.expire(10_000 * 20_000_000);
        assert_eq!(hub.usage().0, 1);
    }

    #[test]
    fn the_window_is_clamped() {
        assert_eq!(clamp_seconds(0), MIN_SECONDS);
        assert_eq!(clamp_seconds(100_000), MAX_SECONDS);
        assert_eq!(ReplayHub::new(60).seconds(), 60);
    }
}
