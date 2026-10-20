//! Non-blocking bridge from the DSP thread to NATS core publishing.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_nats::{Client, connection::State};
use tokio::sync::mpsc::{self, error::TrySendError};

/// Frames buffered between the DSP thread and the publisher task.
const QUEUE_FRAMES: usize = 8;

/// Start a publisher task for `subject` and return a sink for the DSP thread.
///
/// The sink never blocks: while NATS is not connected, or when the queue is full (NATS
/// lagging), the frame is dropped and counted. Spectrum is live, lossy data and is published
/// on core NATS (no JetStream); nothing is buffered across an outage.
pub fn spawn_spectrum_publisher(
    client: Client,
    subject: String,
) -> impl FnMut(Vec<u8>) + Send + 'static {
    let status = client.clone();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(QUEUE_FRAMES);
    let dropped = Arc::new(AtomicU64::new(0));
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if let Err(e) = client.publish(subject.clone(), frame.into()).await {
                tracing::warn!(error = %e, "spectrum publish failed");
            }
        }
    });
    move |frame| {
        let reason = if status.connection_state() != State::Connected {
            "bus is disconnected"
        } else {
            match tx.try_send(frame) {
                Ok(()) => return,
                Err(TrySendError::Full(_)) => "bus is lagging",
                Err(TrySendError::Closed(_)) => return,
            }
        };
        let n = dropped.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 || n.is_multiple_of(250) {
            tracing::warn!(total = n, reason, "dropping spectrum frames");
        }
    }
}

/// Frames buffered between the VFO thread and the audio publisher task (about 0.6 s of one VFO
/// at 20 ms per frame, with headroom for a few VFOs).
const AUDIO_QUEUE_FRAMES: usize = 128;

/// Start a publisher task for the audio of `device` and return a sink for the VFO thread.
///
/// The sink takes `(vfo_id, encoded AudioFrame)` and publishes on `sh.v1.dev.<dev>.audio.<vfo>`.
/// Like the spectrum sink it never blocks and drops (and counts) frames while the bus is
/// disconnected or lagging; audio is live data on core NATS.
///
/// # Errors
/// Fails if `device` is not usable as a subject token.
pub fn spawn_audio_publisher(
    client: Client,
    device: &str,
) -> Result<impl FnMut(u32, Vec<u8>) + Send + 'static, superhet_bus::subjects::InvalidToken> {
    // Validate the device token once; per-frame subjects only add a numeric token.
    superhet_bus::subjects::audio(device, "0")?;
    let device = device.to_owned();
    let status = client.clone();
    let (tx, mut rx) = mpsc::channel::<(u32, Vec<u8>)>(AUDIO_QUEUE_FRAMES);
    let dropped = Arc::new(AtomicU64::new(0));
    tokio::spawn(async move {
        while let Some((vfo, frame)) = rx.recv().await {
            let subject = format!(
                "{}.dev.{device}.audio.{vfo}",
                superhet_bus::subjects::PREFIX
            );
            if let Err(e) = client.publish(subject, frame.into()).await {
                tracing::warn!(error = %e, "audio publish failed");
            }
        }
    });
    Ok(move |vfo, frame| {
        let reason = if status.connection_state() != State::Connected {
            "bus is disconnected"
        } else {
            match tx.try_send((vfo, frame)) {
                Ok(()) => return,
                Err(TrySendError::Full(_)) => "bus is lagging",
                Err(TrySendError::Closed(_)) => return,
            }
        };
        let n = dropped.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 || n.is_multiple_of(250) {
            tracing::warn!(total = n, reason, "dropping audio frames");
        }
    })
}
