//! Cutting a continuous audio stream into time slots aligned to UTC.
//!
//! FT8 transmits in 15 s slots, FT4 in 7.5 s and WSPR in 2 min, all starting at fixed UTC
//! multiples of the slot length (the Unix epoch is a multiple of a minute, so `time % slot == 0`
//! is the boundary). The decoders need the audio of exactly one slot. The samples reach the
//! adapter without timestamps, so the wall clock at which a chunk was read stands for the time
//! of its last sample; a gap in the stream (dropped chunks) is filled with silence so the slot
//! stays aligned, and a stream that runs ahead of the clock is trimmed.

/// Silence is inserted when a chunk starts more than this after the data before it.
const GAP_MS: u64 = 250;
/// The stream may run this far ahead of the clock before samples are dropped.
const AHEAD_MS: u64 = 400;

/// One slot of audio: `samples` start at UTC `start_ms` (milliseconds since the Unix epoch).
#[derive(Debug, PartialEq, Eq)]
pub struct Slot {
    pub start_ms: u64,
    pub samples: Vec<i16>,
}

/// Collects samples and hands out complete slots.
pub struct Slicer {
    rate: u64,
    slot_ms: u64,
    /// A slot whose first samples are missing up to this long is still returned (zero-filled).
    tolerance_ms: u64,
    buf: Vec<i16>,
    /// UTC time of `buf[0]`.
    buf_start_ms: u64,
    /// Start of the slot being collected; `None` before the first chunk.
    next_slot_ms: Option<u64>,
}

impl Slicer {
    /// `rate` samples per second, slots of `slot_ms` milliseconds, a slot may start up to
    /// `tolerance_ms` late and is still decoded.
    #[must_use]
    pub fn new(rate: u32, slot_ms: u64, tolerance_ms: u64) -> Self {
        Self {
            rate: u64::from(rate),
            slot_ms,
            tolerance_ms,
            buf: Vec::new(),
            buf_start_ms: 0,
            next_slot_ms: None,
        }
    }

    fn ms_of(&self, samples: usize) -> u64 {
        samples as u64 * 1000 / self.rate
    }

    fn samples_of(&self, ms: u64) -> usize {
        usize::try_from(ms * self.rate / 1000).unwrap_or(usize::MAX)
    }

    fn timeline_end_ms(&self) -> u64 {
        self.buf_start_ms + self.ms_of(self.buf.len())
    }

    /// The first slot that can still be collected from data starting at `start_ms`.
    fn first_slot(&self, start_ms: u64) -> u64 {
        let floor = start_ms / self.slot_ms * self.slot_ms;
        if start_ms <= floor + self.tolerance_ms {
            floor
        } else {
            floor + self.slot_ms
        }
    }

    /// Add `samples` that were read at UTC `now_ms` (the time of their last sample) and return
    /// the slots that are complete now.
    pub fn push(&mut self, samples: &[i16], now_ms: u64) -> Vec<Slot> {
        let start_ms = now_ms.saturating_sub(self.ms_of(samples.len()));
        let mut samples = samples;
        if self.next_slot_ms.is_none() {
            self.buf.clear();
            self.buf_start_ms = start_ms;
            self.next_slot_ms = Some(self.first_slot(start_ms));
        } else {
            let end = self.timeline_end_ms();
            if start_ms > end + GAP_MS {
                let gap = start_ms - end;
                if gap >= self.slot_ms {
                    // A long outage (or the clock was set): start over.
                    self.next_slot_ms = None;
                    return self.push(samples, now_ms);
                }
                let pad = self.samples_of(gap);
                self.buf.resize(self.buf.len() + pad, 0);
            } else if end > now_ms + self.slot_ms {
                // The clock was set back: start over.
                self.next_slot_ms = None;
                return self.push(samples, now_ms);
            } else if end > now_ms + AHEAD_MS {
                let skip = self.samples_of(end - now_ms).min(samples.len());
                samples = &samples[skip..];
            }
        }
        self.buf.extend_from_slice(samples);
        self.drain_slots()
    }

    fn drain_slots(&mut self) -> Vec<Slot> {
        let mut out = Vec::new();
        let Some(mut next) = self.next_slot_ms else {
            return out;
        };
        let slot_len = self.samples_of(self.slot_ms);
        // The clock stepped back: the slot being waited for lies far in the future.
        if next > self.timeline_end_ms() + self.slot_ms {
            self.buf.clear();
            self.next_slot_ms = None;
            return out;
        }
        // Samples before the slot are of no use.
        if next > self.buf_start_ms {
            let n = self
                .samples_of(next - self.buf_start_ms)
                .min(self.buf.len());
            self.buf.drain(..n);
            self.buf_start_ms += self.ms_of(n);
        }
        while self.timeline_end_ms() >= next + self.slot_ms {
            let mut samples = Vec::with_capacity(slot_len);
            if self.buf_start_ms > next {
                // The stream began a little after the slot did (within the tolerance).
                samples.resize(self.samples_of(self.buf_start_ms - next), 0);
            }
            let take = slot_len - samples.len();
            samples.extend_from_slice(&self.buf[..take]);
            out.push(Slot {
                start_ms: next,
                samples,
            });
            self.buf.drain(..take);
            next += self.slot_ms;
            self.buf_start_ms = next;
        }
        self.next_slot_ms = Some(next);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 12_000;
    const T0: u64 = 1_800_000_000_000; // a multiple of 15 s, 7.5 s and 120 s

    /// A stream whose sample `n` equals `n % 30011` from `start_ms`, cut into chunks of `chunk_ms`
    /// that are "read" at the time of their last sample. Returns the slots and the last index.
    fn feed(
        slicer: &mut Slicer,
        start_ms: u64,
        total_ms: u64,
        chunk_ms: u64,
        first_index: usize,
    ) -> Vec<Slot> {
        let mut slots = Vec::new();
        let per_chunk = (chunk_ms * u64::from(RATE) / 1000) as usize;
        let chunks = (total_ms / chunk_ms) as usize;
        for c in 0..chunks {
            let base = first_index + c * per_chunk;
            let samples: Vec<i16> = (0..per_chunk)
                .map(|i| ((base + i) % 30_011) as i16)
                .collect();
            let now = start_ms + (c as u64 + 1) * chunk_ms;
            slots.extend(slicer.push(&samples, now));
        }
        slots
    }

    #[test]
    fn aligned_stream_gives_back_to_back_slots() {
        let mut s = Slicer::new(RATE, 15_000, 1_000);
        let slots = feed(&mut s, T0, 46_000, 20, 0);
        assert_eq!(slots.len(), 3);
        for (k, slot) in slots.iter().enumerate() {
            assert_eq!(slot.start_ms, T0 + k as u64 * 15_000);
            assert_eq!(slot.samples.len(), 180_000);
            assert_eq!(slot.samples[0], ((k * 180_000) % 30_011) as i16);
            assert_eq!(
                slot.samples[179_999],
                ((k * 180_000 + 179_999) % 30_011) as i16
            );
        }
    }

    #[test]
    fn a_slot_that_began_too_early_is_skipped() {
        // The stream starts 5 s into a slot: that slot is incomplete, the next boundary is used.
        let mut s = Slicer::new(RATE, 15_000, 1_000);
        let slots = feed(&mut s, T0 + 5_000, 39_000, 20, 0);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].start_ms, T0 + 15_000);
        // Sample index of that boundary: 10 s after the start of the stream.
        assert_eq!(slots[0].samples[0], ((10 * 12_000) % 30_011) as i16);
    }

    #[test]
    fn a_slot_that_began_slightly_late_is_zero_filled() {
        let mut s = Slicer::new(RATE, 15_000, 1_000);
        let slots = feed(&mut s, T0 + 500, 15_000, 20, 1);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].start_ms, T0);
        assert_eq!(slots[0].samples.len(), 180_000);
        assert!(slots[0].samples[..6_000].iter().all(|&v| v == 0));
        assert_eq!(slots[0].samples[6_000], 1);
    }

    #[test]
    fn a_short_gap_is_filled_with_silence_and_keeps_alignment() {
        let mut s = Slicer::new(RATE, 15_000, 1_000);
        // 5 s of stream, 2 s lost, the rest. The slot must still end where the clock says.
        let mut slots = feed(&mut s, T0, 5_000, 20, 0);
        slots.extend(feed(&mut s, T0 + 7_000, 9_000, 20, 0));
        assert_eq!(slots.len(), 1);
        let slot = &slots[0];
        assert_eq!(slot.samples.len(), 180_000);
        assert!(
            slot.samples[60_000..84_000].iter().all(|&v| v == 0),
            "silence"
        );
        // After the gap the stream restarts at its own index 0.
        assert_eq!(slot.samples[84_000], 0);
        assert_eq!(slot.samples[84_001], 1);
    }

    #[test]
    fn a_long_outage_starts_over_at_the_next_boundary() {
        let mut s = Slicer::new(RATE, 15_000, 1_000);
        let mut slots = feed(&mut s, T0, 6_000, 20, 0);
        // 74 s of nothing, then the stream is back in the middle of a slot.
        slots.extend(feed(&mut s, T0 + 80_000, 26_000, 20, 0));
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].start_ms, T0 + 90_000);
    }

    #[test]
    fn a_stream_that_runs_ahead_of_the_clock_is_trimmed() {
        let mut s = Slicer::new(RATE, 15_000, 1_000);
        assert!(s.push(&vec![1i16; 12_000], T0 + 1_000).is_empty());
        // 2 s of samples in one burst at the 2 s mark: the stream is now 1 s ahead.
        assert!(s.push(&vec![2i16; 24_000], T0 + 2_000).is_empty());
        assert_eq!(s.timeline_end_ms(), T0 + 3_000);
        // On-time samples are dropped until the clock has caught up.
        assert!(s.push(&vec![3i16; 240], T0 + 2_020).is_empty());
        assert_eq!(s.timeline_end_ms(), T0 + 3_000);
        assert!(s.push(&vec![3i16; 240], T0 + 3_400).is_empty());
        assert!(s.timeline_end_ms() > T0 + 3_000);
    }

    #[test]
    fn ft4_slots_are_7_5_seconds() {
        let mut s = Slicer::new(RATE, 7_500, 500);
        let slots = feed(&mut s, T0, 23_000, 20, 0);
        assert_eq!(slots.len(), 3);
        assert_eq!(slots[1].start_ms, T0 + 7_500);
        assert_eq!(slots[1].samples.len(), 90_000);
    }

    #[test]
    fn a_clock_step_back_does_not_pile_up_memory() {
        let mut s = Slicer::new(RATE, 15_000, 1_000);
        let mut slots = feed(&mut s, T0 + 600_000, 20_000, 20, 0);
        // The clock is set back an hour; the stream continues.
        slots.extend(feed(&mut s, T0, 40_000, 20, 0));
        assert!(!slots.is_empty());
        assert!(s.buf.len() <= 180_000 + 2_000);
    }
}
