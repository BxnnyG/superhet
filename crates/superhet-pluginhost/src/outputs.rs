//! Plugin output towards the rest of the system: PCM audio of a framed plugin becomes Opus
//! frames on the normal audio subject.
//!
//! A plugin with `[output.audio]` sends PCM in `audio` frames. [`BusOutputs`] cuts it into
//! 20 ms blocks, encodes them with libopus (48 kHz, VBR, 96 kbit/s stereo or 64 kbit/s mono, like
//! WFM) and publishes `AudioFrame` v1 on `sh.v1.dev.<dev>.audio.<id>`, where `<id>` is the
//! instance's pseudo VFO (`superhet_plugin_protocol::ctl::audio_vfo_of`). For the API, browsers and
//! recorders it is a stream like any VFO's (`docs/protocol-audio.md`). Like all audio it is live
//! data on core NATS: a full queue or a disconnected bus drops frames.

use async_nats::{Client, connection::State};
use superhet_bus::audio::{AudioFrame, FLAG_SQUELCH_OPEN, FRAME_SAMPLES};
use superhet_bus::opus::{Encoder, SAMPLE_RATE};
use superhet_bus::subjects;
use tokio::sync::mpsc::{self, error::TrySendError};

/// Frames buffered between the session and the publisher task (about 2.5 s at 20 ms).
const QUEUE_FRAMES: usize = 128;
/// PCM kept while waiting for a full block; more means the plugin sends faster than real time
/// and the oldest samples are dropped (about 1 s of stereo).
const MAX_PENDING_SAMPLES: usize = 48_000 * 2;

/// What a plugin's audio output looks like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSpec {
    /// Device the audio belongs to (subject token).
    pub device: String,
    /// Pseudo VFO id the frames carry.
    pub vfo_id: u32,
    /// Sample rate (Hz) of the PCM; only 48000 is supported.
    pub sample_rate: u32,
    /// 1 or 2.
    pub channels: u8,
}

/// Takes the PCM of one instance. Must not block.
pub trait AudioOut: Send {
    /// PCM s16le, interleaved, as it arrived in one `audio` frame.
    fn push(&mut self, pcm: &[u8]);
}

/// Creates audio outputs for framed instances.
pub trait Outputs: Send + Sync + 'static {
    /// The output for `spec`, or `None` when it cannot be provided (the audio is then dropped).
    fn open_audio(&self, spec: &AudioSpec) -> Option<Box<dyn AudioOut>>;
}

/// No outputs: audio frames are discarded.
#[derive(Debug, Default)]
pub struct NoOutputs;

impl Outputs for NoOutputs {
    fn open_audio(&self, _spec: &AudioSpec) -> Option<Box<dyn AudioOut>> {
        None
    }
}

/// [`Outputs`] over the bus.
#[derive(Clone)]
pub struct BusOutputs {
    client: Client,
}

impl BusOutputs {
    /// Outputs publishing through `client`.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

impl Outputs for BusOutputs {
    fn open_audio(&self, spec: &AudioSpec) -> Option<Box<dyn AudioOut>> {
        if i64::from(spec.sample_rate) != i64::from(SAMPLE_RATE) {
            tracing::warn!(
                rate = spec.sample_rate,
                "plugin audio must be 48000 Hz; dropping it"
            );
            return None;
        }
        let channels = usize::from(spec.channels);
        let bitrate = if channels == 2 { 96_000 } else { 64_000 };
        let encoder = Encoder::with_channels(channels, bitrate)
            .map_err(|err| tracing::warn!(%err, "cannot create the Opus encoder"))
            .ok()?;
        let subject = subjects::audio(&spec.device, &spec.vfo_id.to_string()).ok()?;
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(QUEUE_FRAMES);
        let client = self.client.clone();
        let status = client.clone();
        let task = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                if let Err(err) = client.publish(subject.clone(), frame.into()).await {
                    tracing::warn!(%err, "plugin audio publish failed");
                }
            }
        });
        Some(Box::new(OpusOut {
            encoder,
            channels,
            vfo_id: spec.vfo_id,
            pending: Vec::new(),
            seq: 0,
            sample_pos: 0,
            dropped: 0,
            tx,
            status,
            task,
        }))
    }
}

struct OpusOut {
    encoder: Encoder,
    channels: usize,
    vfo_id: u32,
    /// Interleaved samples not yet encoded.
    pending: Vec<f32>,
    seq: u32,
    sample_pos: u64,
    dropped: u64,
    tx: mpsc::Sender<Vec<u8>>,
    status: Client,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for OpusOut {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl AudioOut for OpusOut {
    fn push(&mut self, pcm: &[u8]) {
        self.pending.extend(
            pcm.as_chunks::<2>()
                .0
                .iter()
                .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0),
        );
        let block = FRAME_SAMPLES * self.channels;
        if self.pending.len() > MAX_PENDING_SAMPLES {
            let excess = self.pending.len() - MAX_PENDING_SAMPLES;
            self.pending.drain(..excess - excess % self.channels);
        }
        let mut out = [0u8; 1500];
        while self.pending.len() >= block {
            let chunk: Vec<f32> = self.pending.drain(..block).collect();
            let power = power_dbfs(&chunk);
            let Ok(n) = self.encoder.encode_float(&chunk, &mut out) else {
                continue;
            };
            let frame = AudioFrame {
                channels: u8::try_from(self.channels).unwrap_or(1),
                flags: FLAG_SQUELCH_OPEN,
                sample_rate: 48_000,
                vfo_id: self.vfo_id,
                seq: self.seq,
                sample_pos: self.sample_pos,
                capture_unix_ns: unix_ns(),
                power_dbfs: power,
                payload: out[..n].to_vec(),
            };
            self.seq = self.seq.wrapping_add(1);
            self.sample_pos += FRAME_SAMPLES as u64;
            if self.status.connection_state() != State::Connected
                || matches!(self.tx.try_send(frame.encode()), Err(TrySendError::Full(_)))
            {
                self.dropped += 1;
                if self.dropped == 1 || self.dropped.is_multiple_of(500) {
                    tracing::warn!(total = self.dropped, "dropping plugin audio frames");
                }
            }
        }
    }
}

/// Mean power of a block in dBFS (a full-scale square wave is 0 dBFS).
fn power_dbfs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -200.0;
    }
    let mean = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    (10.0 * mean.max(1e-20).log10()).max(-200.0)
}

fn unix_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_vfo_range_matches_the_bus_constant() {
        assert_eq!(
            superhet_bus::audio::PLUGIN_VFO_BASE,
            superhet_plugin_protocol::ctl::AUDIO_VFO_BASE
        );
    }

    #[test]
    fn power_of_silence_and_a_full_scale_square() {
        assert!(power_dbfs(&[0.0; 960]) <= -190.0);
        assert!(power_dbfs(&[1.0; 960]).abs() < 0.01);
    }
}
