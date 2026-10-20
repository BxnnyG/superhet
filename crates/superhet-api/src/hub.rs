//! Shares one bus subscription per device between all WebSocket clients.
//!
//! Each device gets a [`broadcast`] channel of decoded frames. The NATS subscription behind it
//! is created on first use and dropped once no client listens any more. Receivers that fall
//! behind lose the oldest frames (`Lagged`) and never delay the subscription or other clients.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::StreamExt;
use superhet_bus::spectrum::SpectrumFrame;
use tokio::sync::{Mutex, broadcast, oneshot};

/// Frames buffered per client before the oldest are dropped (a few frames of latency at most).
const CHANNEL_CAPACITY: usize = 4;
/// Upper bound on distinct devices streamed at the same time.
const MAX_DEVICES: usize = 16;

/// Where frames come from.
enum Source {
    /// NATS subjects published by sdrd.
    Bus(async_nats::Client),
    /// In-process injection via `Hub::publish_local` (tests).
    #[cfg(test)]
    Local,
    /// No bus configured; nothing can be streamed.
    Unavailable,
}

/// Why a device stream could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum HubError {
    /// No bus is configured.
    #[error("spectrum source unavailable")]
    Unavailable,
    /// The device id is not usable as a subject token.
    #[error("invalid device id")]
    InvalidDevice,
    /// Too many distinct devices are streamed at once.
    #[error("too many devices")]
    TooManyDevices,
    /// The bus subscription failed.
    #[error("subscribe failed: {0}")]
    Subscribe(String),
}

/// A shared per-device stream. Dropping it stops the bus subscription feeding it.
struct Entry {
    tx: broadcast::Sender<Arc<SpectrumFrame>>,
    _stop: oneshot::Sender<()>,
}

type Devices = HashMap<String, Entry>;

/// Frame distribution hub.
pub struct Hub {
    source: Source,
    devices: Arc<Mutex<Devices>>,
}

impl Hub {
    /// Hub fed by the given bus client, or an unavailable one when there is none.
    #[must_use]
    pub fn new(nats: Option<async_nats::Client>) -> Self {
        Self::with_source(nats.map_or(Source::Unavailable, Source::Bus))
    }

    /// Hub without a bus; frames are injected with `publish_local`.
    #[cfg(test)]
    #[must_use]
    pub fn local() -> Self {
        Self::with_source(Source::Local)
    }

    fn with_source(source: Source) -> Self {
        Self {
            source,
            devices: Arc::default(),
        }
    }

    /// Number of devices currently streamed.
    #[cfg(test)]
    pub async fn device_count(&self) -> usize {
        self.devices.lock().await.len()
    }

    /// Inject a frame for `device` (delivered to current listeners).
    #[cfg(test)]
    pub async fn publish_local(&self, device: &str, frame: SpectrumFrame) {
        if let Some(entry) = self.devices.lock().await.get(device) {
            let _ = entry.tx.send(Arc::new(frame));
        }
    }

    /// Listen to the frames of `device`, starting the bus subscription if needed.
    ///
    /// # Errors
    /// See [`HubError`].
    pub async fn subscribe(
        &self,
        device: &str,
    ) -> Result<broadcast::Receiver<Arc<SpectrumFrame>>, HubError> {
        if matches!(self.source, Source::Unavailable) {
            return Err(HubError::Unavailable);
        }
        let subject =
            superhet_bus::subjects::spectrum(device).map_err(|_| HubError::InvalidDevice)?;
        let mut devices = self.devices.lock().await;
        if let Some(entry) = devices.get(device) {
            return Ok(entry.tx.subscribe());
        }
        if devices.len() >= MAX_DEVICES {
            return Err(HubError::TooManyDevices);
        }
        let (tx, rx) = broadcast::channel(CHANNEL_CAPACITY);
        let (stop, stopped) = oneshot::channel();
        if let Source::Bus(client) = &self.source {
            let sub = client
                .subscribe(subject)
                .await
                .map_err(|e| HubError::Subscribe(e.to_string()))?;
            tokio::spawn(forward(
                sub,
                tx.clone(),
                stopped,
                Arc::clone(&self.devices),
                device.to_owned(),
            ));
        }
        devices.insert(device.to_owned(), Entry { tx, _stop: stop });
        Ok(rx)
    }

    /// Stop streaming `device` if nobody listens any more. Call after dropping the receiver.
    pub async fn release(&self, device: &str) {
        let mut devices = self.devices.lock().await;
        if devices
            .get(device)
            .is_some_and(|entry| entry.tx.receiver_count() == 0)
        {
            devices.remove(device);
        }
    }
}

/// Decode bus messages and fan them out until stopped (entry dropped) or the bus ends.
async fn forward(
    mut sub: async_nats::Subscriber,
    tx: broadcast::Sender<Arc<SpectrumFrame>>,
    mut stopped: oneshot::Receiver<()>,
    registry: Arc<Mutex<Devices>>,
    device: String,
) {
    loop {
        tokio::select! {
            _ = &mut stopped => return,
            msg = sub.next() => {
                let Some(msg) = msg else { break };
                match SpectrumFrame::decode(&msg.payload) {
                    // An error only means that no client listens right now.
                    Ok(frame) => drop(tx.send(Arc::new(frame))),
                    Err(err) => tracing::debug!(%err, device, "dropping undecodable spectrum frame"),
                }
            }
        }
    }
    // The subscription ended: forget the entry (unless it was already replaced) so that the
    // next listener resubscribes.
    let mut devices = registry.lock().await;
    if devices
        .get(&device)
        .is_some_and(|entry| entry.tx.same_channel(&tx))
    {
        devices.remove(&device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> SpectrumFrame {
        SpectrumFrame::quantize(1, 2, 3, -100.0, 0.0, &[-50.0; 8])
    }

    #[tokio::test]
    async fn local_frames_reach_all_listeners() {
        let hub = Hub::local();
        let mut a = hub.subscribe("dev0").await.expect("subscribe");
        let mut b = hub.subscribe("dev0").await.expect("subscribe");
        hub.publish_local("dev0", frame()).await;
        assert_eq!(*a.recv().await.expect("a"), frame());
        assert_eq!(*b.recv().await.expect("b"), frame());
    }

    #[tokio::test]
    async fn slow_listener_loses_old_frames_without_blocking_publisher() {
        let hub = Hub::local();
        let mut slow = hub.subscribe("dev0").await.expect("subscribe");
        for _ in 0..(CHANNEL_CAPACITY * 3) {
            hub.publish_local("dev0", frame()).await;
        }
        assert!(matches!(
            slow.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ));
        assert!(slow.recv().await.is_ok());
    }

    #[tokio::test]
    async fn rejects_invalid_and_unavailable() {
        let hub = Hub::local();
        assert!(matches!(
            hub.subscribe("a.b").await,
            Err(HubError::InvalidDevice)
        ));
        assert!(matches!(
            Hub::new(None).subscribe("dev0").await,
            Err(HubError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn release_frees_idle_devices_only() {
        let hub = Hub::local();
        let rx = hub.subscribe("dev0").await.expect("subscribe");
        hub.release("dev0").await;
        assert_eq!(hub.devices.lock().await.len(), 1, "still has a listener");
        drop(rx);
        hub.release("dev0").await;
        assert!(hub.devices.lock().await.is_empty());
    }

    #[tokio::test]
    async fn caps_distinct_devices() {
        let hub = Hub::local();
        let mut keep = Vec::new();
        for i in 0..MAX_DEVICES {
            keep.push(hub.subscribe(&format!("d{i}")).await.expect("subscribe"));
        }
        assert!(matches!(
            hub.subscribe("extra").await,
            Err(HubError::TooManyDevices)
        ));
    }
}
