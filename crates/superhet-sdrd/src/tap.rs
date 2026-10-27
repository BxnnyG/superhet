//! Plugin input taps (`superhet_bus::tap`): the lease table shared with the VFO thread and the
//! per-tap conversion (resample, encode, chunk) that thread runs.
//!
//! Control requests (`ctl.tap.open` / `ctl.tap.close`) only edit the table; the VFO thread
//! notices the new [`TapShared::generation`], builds a [`TapStream`] for every tap it can serve
//! and publishes the encoded chunks through the tap sink. A tap that is not refreshed within
//! [`superhet_bus::tap::LEASE_TTL`] is dropped by the expiry task of the service.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use superhet_bus::tap::{MAX_TAPS, TapFormat, TapOpen};
use superhet_device::Complex32;
use superhet_dsp::resample::Resampler;

use crate::control::{CtlError, ErrorCode};

/// Largest message the tap publisher sends (bytes); NATS allows 1 MiB by default.
const MAX_CHUNK: usize = 64 * 1024;
/// Chunk duration (seconds) the converter aims for before it publishes.
const CHUNK_SECONDS: f64 = 0.02;
/// Fewest samples per chunk, so tiny rates still batch.
const MIN_CHUNK_SAMPLES: usize = 64;

struct Entry {
    open: TapOpen,
    deadline: Instant,
}

/// The taps requested at one device, shared between the control service and the VFO thread.
#[derive(Default)]
pub struct TapShared {
    entries: Mutex<Vec<Entry>>,
    generation: AtomicU64,
}

impl TapShared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Create a tap or refresh the lease of an identical one; a different request for the same
    /// id replaces it. Returns whether the set of taps changed.
    ///
    /// # Errors
    /// `invalid` when the table is full.
    pub fn open(&self, open: TapOpen, ttl: Duration) -> Result<bool, CtlError> {
        let deadline = Instant::now() + ttl;
        let mut entries = self.lock();
        if let Some(entry) = entries.iter_mut().find(|e| e.open.id == open.id) {
            entry.deadline = deadline;
            if entry.open == open {
                return Ok(false);
            }
            entry.open = open;
        } else {
            if entries.len() >= MAX_TAPS {
                return Err(CtlError::new(
                    ErrorCode::Invalid,
                    format!("at most {MAX_TAPS} taps per device"),
                ));
            }
            entries.push(Entry { open, deadline });
        }
        self.generation.fetch_add(1, Ordering::Release);
        Ok(true)
    }

    /// Remove a tap. Returns whether it existed.
    pub fn close(&self, id: &str) -> bool {
        let mut entries = self.lock();
        let before = entries.len();
        entries.retain(|e| e.open.id != id);
        let changed = entries.len() != before;
        if changed {
            self.generation.fetch_add(1, Ordering::Release);
        }
        changed
    }

    /// Drop taps whose lease ran out; returns their ids.
    pub fn expire(&self, now: Instant) -> Vec<String> {
        let mut entries = self.lock();
        let mut gone = Vec::new();
        entries.retain(|e| {
            let keep = e.deadline > now;
            if !keep {
                gone.push(e.open.id.clone());
            }
            keep
        });
        if !gone.is_empty() {
            self.generation.fetch_add(1, Ordering::Release);
        }
        gone
    }

    /// Changes whenever the set of taps (not just a lease) changes.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// The current taps.
    pub fn snapshot(&self) -> Vec<TapOpen> {
        self.lock().iter().map(|e| e.open.clone()).collect()
    }

    /// Number of taps.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lock().len()
    }
}

/// Where encoded chunks go: `(tap id, bytes)`. Runs on the VFO thread and must not block.
pub type TapSink = Box<dyn FnMut(&str, Vec<u8>) + Send>;

/// What the VFO thread needs to serve taps.
pub struct TapOutput {
    /// The lease table.
    pub shared: std::sync::Arc<TapShared>,
    /// Receiver of the encoded chunks.
    pub sink: TapSink,
}

/// Append `samples` (full scale = 1.0) in a real `format`.
fn encode_real(samples: &[f32], format: TapFormat, out: &mut Vec<u8>) {
    match format {
        TapFormat::S16le => {
            for s in samples {
                out.extend_from_slice(
                    &((s.clamp(-1.0, 1.0) * 32_767.0).round() as i16).to_le_bytes(),
                );
            }
        }
        TapFormat::F32le => {
            for s in samples {
                out.extend_from_slice(&s.to_le_bytes());
            }
        }
        _ => {}
    }
}

/// Append `samples` (full scale = 1.0) as interleaved I/Q in a complex `format`.
fn encode_iq(samples: &[Complex32], format: TapFormat, out: &mut Vec<u8>) {
    match format {
        TapFormat::Cu8 => {
            for s in samples {
                out.push((s.re.clamp(-1.0, 1.0) * 127.5 + 127.5).round() as u8);
                out.push((s.im.clamp(-1.0, 1.0) * 127.5 + 127.5).round() as u8);
            }
        }
        TapFormat::Cs16 => {
            for s in samples {
                out.extend_from_slice(
                    &((s.re.clamp(-1.0, 1.0) * 32_767.0).round() as i16).to_le_bytes(),
                );
                out.extend_from_slice(
                    &((s.im.clamp(-1.0, 1.0) * 32_767.0).round() as i16).to_le_bytes(),
                );
            }
        }
        TapFormat::Cf32 => {
            for s in samples {
                out.extend_from_slice(&s.re.to_le_bytes());
                out.extend_from_slice(&s.im.to_le_bytes());
            }
        }
        _ => {}
    }
}

/// Resamplers of a tap, rebuilt when the input rate changes.
struct Rate {
    input: f64,
    re: Resampler,
    im: Resampler,
}

/// Conversion state of one tap on the VFO thread.
pub struct TapStream {
    /// The request this stream serves.
    pub open: TapOpen,
    rate: Option<Rate>,
    pending: Vec<u8>,
    chunk_bytes: usize,
    re_in: Vec<f32>,
    im_in: Vec<f32>,
    re_out: Vec<f32>,
    im_out: Vec<f32>,
    iq_out: Vec<Complex32>,
}

impl TapStream {
    /// A stream for `open`.
    #[must_use]
    pub fn new(open: TapOpen) -> Self {
        let sample = open.format.sample_bytes();
        let samples = ((f64::from(open.rate) * CHUNK_SECONDS) as usize).max(MIN_CHUNK_SAMPLES);
        let chunk_bytes = (samples * sample).min(MAX_CHUNK - MAX_CHUNK % sample);
        Self {
            open,
            rate: None,
            pending: Vec::new(),
            chunk_bytes,
            re_in: Vec::new(),
            im_in: Vec::new(),
            re_out: Vec::new(),
            im_out: Vec::new(),
            iq_out: Vec::new(),
        }
    }

    fn resampler(&mut self, input: f64) -> Option<&mut Rate> {
        let output = f64::from(self.open.rate);
        if (input - output).abs() < 0.5 {
            self.rate = None;
            return None;
        }
        if self
            .rate
            .as_ref()
            .is_none_or(|r| (r.input - input).abs() > 0.5)
        {
            self.rate = Some(Rate {
                input,
                re: Resampler::new(input, output),
                im: Resampler::new(input, output),
            });
        }
        self.rate.as_mut()
    }

    /// Feed mono audio at 48 kHz (`audio` taps).
    pub fn push_audio(&mut self, mono: &[f32], rate_in: f64, sink: &mut dyn FnMut(&str, Vec<u8>)) {
        let format = self.open.format;
        if let Some(rate) = self.resampler(rate_in) {
            let mut out = Vec::with_capacity(mono.len());
            rate.re.process(mono, &mut out);
            encode_real(&out, format, &mut self.pending);
        } else {
            encode_real(mono, format, &mut self.pending);
        }
        self.flush(sink);
    }

    /// Feed complex samples taken at `rate_in` (`iq` and `window` taps).
    pub fn push_iq(&mut self, iq: &[Complex32], rate_in: f64, sink: &mut dyn FnMut(&str, Vec<u8>)) {
        let format = self.open.format;
        if self.resampler(rate_in).is_some() {
            self.re_in.clear();
            self.im_in.clear();
            self.re_in.extend(iq.iter().map(|s| s.re));
            self.im_in.extend(iq.iter().map(|s| s.im));
            self.re_out.clear();
            self.im_out.clear();
            if let Some(rate) = self.rate.as_mut() {
                rate.re.process(&self.re_in, &mut self.re_out);
                rate.im.process(&self.im_in, &mut self.im_out);
            }
            let n = self.re_out.len().min(self.im_out.len());
            self.iq_out.clear();
            self.iq_out.extend(
                self.re_out[..n]
                    .iter()
                    .zip(&self.im_out[..n])
                    .map(|(re, im)| Complex32::new(*re, *im)),
            );
            encode_iq(&self.iq_out, format, &mut self.pending);
        } else {
            encode_iq(iq, format, &mut self.pending);
        }
        self.flush(sink);
    }

    /// Publish every full chunk.
    fn flush(&mut self, sink: &mut dyn FnMut(&str, Vec<u8>)) {
        if self.pending.len() < self.chunk_bytes {
            return;
        }
        let whole = self.pending.len() - self.pending.len() % self.open.format.sample_bytes();
        let mut start = 0;
        while whole - start >= self.chunk_bytes {
            sink(
                &self.open.id,
                self.pending[start..start + self.chunk_bytes].to_vec(),
            );
            start += self.chunk_bytes;
        }
        self.pending.drain(..start);
    }
}

/// Make `streams` serve exactly `wanted`: keep the state of unchanged taps, add new ones, drop
/// the rest.
pub fn sync_streams<'a>(streams: &mut Vec<TapStream>, wanted: impl Iterator<Item = &'a TapOpen>) {
    let wanted: Vec<&TapOpen> = wanted.collect();
    streams.retain(|s| wanted.iter().any(|w| **w == s.open));
    for open in wanted {
        if !streams.iter().any(|s| s.open == *open) {
            streams.push(TapStream::new(open.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use superhet_bus::tap::TapKind;

    fn open(id: &str, kind: TapKind, format: TapFormat, rate: u32) -> TapOpen {
        TapOpen {
            v: 1,
            id: id.into(),
            kind,
            vfo: (kind != TapKind::Window).then_some(1),
            rate,
            format,
            center_hz: None,
        }
    }

    #[test]
    fn leases_refresh_replace_and_expire() {
        let taps = TapShared::default();
        let a = open("a", TapKind::Audio, TapFormat::S16le, 22_050);
        assert_eq!(taps.open(a.clone(), Duration::from_secs(10)), Ok(true));
        let generation = taps.generation();
        // Same request: lease refreshed, nothing changes for the VFO thread.
        assert_eq!(taps.open(a.clone(), Duration::from_secs(10)), Ok(false));
        assert_eq!(taps.generation(), generation);
        let changed = TapOpen { rate: 11_025, ..a };
        assert_eq!(
            taps.open(changed.clone(), Duration::from_secs(10)),
            Ok(true)
        );
        assert_eq!(taps.snapshot(), vec![changed]);
        assert!(taps.expire(Instant::now()).is_empty());
        assert_eq!(taps.expire(Instant::now() + Duration::from_secs(11)), ["a"]);
        assert_eq!(taps.len(), 0);
        assert!(!taps.close("a"));
    }

    #[test]
    fn the_table_is_bounded() {
        let taps = TapShared::default();
        for n in 0..MAX_TAPS {
            let tap = open(&format!("t{n}"), TapKind::Audio, TapFormat::S16le, 8000);
            assert_eq!(taps.open(tap, Duration::from_secs(1)), Ok(true));
        }
        let extra = open("extra", TapKind::Audio, TapFormat::S16le, 8000);
        assert!(taps.open(extra, Duration::from_secs(1)).is_err());
        assert!(taps.close("t0"));
    }

    fn collect(
        stream: &mut TapStream,
        run: impl FnOnce(&mut TapStream, &mut dyn FnMut(&str, Vec<u8>)),
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        run(stream, &mut |id: &str, chunk: Vec<u8>| {
            assert_eq!(id, "x");
            assert!(chunk.len() <= MAX_CHUNK);
            bytes.extend(chunk);
        });
        bytes
    }

    #[test]
    fn audio_is_resampled_and_encoded_as_s16le() {
        let mut stream = TapStream::new(open("x", TapKind::Audio, TapFormat::S16le, 24_000));
        let tone: Vec<f32> = (0..48_000)
            .map(|n| (std::f32::consts::TAU * 1000.0 * n as f32 / 48_000.0).sin() * 0.5)
            .collect();
        let bytes = collect(&mut stream, |s, sink| {
            for block in tone.chunks(960) {
                s.push_audio(block, 48_000.0, sink);
            }
        });
        let samples = bytes.len() / 2;
        // One second at 24 kHz, minus what is still pending in the last partial chunk.
        assert!((23_000..=24_000).contains(&samples), "{samples}");
        let peak = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b).unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!((15_500..=17_200).contains(&peak), "peak {peak}");
    }

    #[test]
    fn iq_formats_have_the_expected_layout() {
        let sample = [Complex32::new(1.0, -1.0), Complex32::new(0.0, 0.5)];
        let mut cu8 = Vec::new();
        encode_iq(&sample, TapFormat::Cu8, &mut cu8);
        assert_eq!(cu8, [255, 0, 128, 191]);
        let mut cs16 = Vec::new();
        encode_iq(&sample[..1], TapFormat::Cs16, &mut cs16);
        assert_eq!(cs16, [0xff, 0x7f, 0x01, 0x80]);
        let mut cf32 = Vec::new();
        encode_iq(&sample[1..], TapFormat::Cf32, &mut cf32);
        assert_eq!(cf32.len(), 8);
        assert_eq!(
            f32::from_le_bytes([cf32[4], cf32[5], cf32[6], cf32[7]]),
            0.5
        );
    }

    #[test]
    fn iq_at_the_same_rate_passes_through_in_whole_samples() {
        let mut stream = TapStream::new(open("x", TapKind::Window, TapFormat::Cu8, 250_000));
        let block = vec![Complex32::new(0.5, -0.5); 4097];
        let bytes = collect(&mut stream, |s, sink| {
            for _ in 0..3 {
                s.push_iq(&block, 250_000.0, sink);
            }
        });
        assert_eq!(bytes.len() % 2, 0);
        assert!(bytes.len() >= 2 * 4097 * 2, "{}", bytes.len());
        assert_eq!(&bytes[..2], &[191, 64]);
    }

    #[test]
    fn iq_is_resampled_to_the_requested_rate() {
        let mut stream = TapStream::new(open("x", TapKind::Iq, TapFormat::Cf32, 12_500));
        let block = vec![Complex32::new(0.25, 0.0); 2_500];
        let bytes = collect(&mut stream, |s, sink| {
            for _ in 0..20 {
                s.push_iq(&block, 25_000.0, sink);
            }
        });
        // 50 000 input samples at 25 kHz are two seconds, i.e. about 25 000 output samples.
        let samples = bytes.len() / 8;
        assert!((24_000..=25_000).contains(&samples), "{samples}");
    }

    #[test]
    fn streams_follow_the_wanted_set_and_keep_unchanged_ones() {
        let a = open("a", TapKind::Audio, TapFormat::S16le, 8000);
        let b = open("b", TapKind::Audio, TapFormat::F32le, 8000);
        let mut streams = Vec::new();
        sync_streams(&mut streams, [&a, &b].into_iter());
        assert_eq!(streams.len(), 2);
        let mut sink = |_: &str, _: Vec<u8>| {};
        streams[0].push_audio(&[0.1; 100], 48_000.0, &mut sink);
        let had_rate = streams[0].rate.is_some();
        sync_streams(&mut streams, [&a].into_iter());
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].rate.is_some(), had_rate, "state is kept");
    }
}
