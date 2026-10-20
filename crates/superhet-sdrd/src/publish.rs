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
