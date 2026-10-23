//! Shares one bus subscription per device between all WebSocket clients.
//!
//! Each device gets two [`broadcast`] channels of decoded frames: the full-band spectrum and the
//! zoom spectrum (a narrow slice with real resolution, see `docs/adr/0009-zoom-fft.md`). The NATS
//! subscriptions behind them are created on first use and dropped once no client listens any
//! more. Receivers that fall behind lose the oldest frames (`Lagged`) and never delay the
//! subscriptions or other clients.
//!
//! The hub also owns the zoom lease: sessions report the span they show ([`Hub::zoom_set`]); the
//! latest request wins and is leased to the daemon (and repeated while it is needed).

use std::collections::HashMap;
use std::sync::Arc;

use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt;
use superhet_bus::spectrum::SpectrumFrame;
use superhet_bus::zoom::{REFRESH, ZoomCommand};
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
    zoom_tx: broadcast::Sender<Arc<SpectrumFrame>>,
    _stop: oneshot::Sender<()>,
}

/// The two frame streams of a device.
pub struct Streams {
    /// Full-band spectrum frames.
    pub full: broadcast::Receiver<Arc<SpectrumFrame>>,
    /// Zoom spectrum frames (empty while nobody leases a slice).
    pub zoom: broadcast::Receiver<Arc<SpectrumFrame>>,
}

/// Who wants which slice of a device: the latest request wins.
#[derive(Debug, Default)]
struct ZoomBook {
    /// Per session: sequence number of its last change and the slice it shows.
    views: HashMap<u64, (u64, (u64, u32))>,
    next_seq: u64,
    /// What the daemon was last asked for; `None` when no lease is active.
    sent: Option<ZoomCommand>,
}

impl ZoomBook {
    /// The request that reflects the current wishes.
    fn effective(&self) -> ZoomCommand {
        self.views.values().max_by_key(|(seq, _)| *seq).map_or(
            ZoomCommand::Off,
            |(_, (center_hz, span_hz))| ZoomCommand::View {
                center_hz: *center_hz,
                span_hz: *span_hz,
            },
        )
    }

    /// Record the slice `owner` shows (`None` = nothing zoomed). Returns the command to send if
    /// it differs from the one the daemon holds. An unchanged view keeps its age, so a hidden
    /// tab re-sending its view does not take over from the tab being used.
    fn set(&mut self, owner: u64, view: Option<(u64, u32)>) -> Option<ZoomCommand> {
        match view {
            None => {
                self.views.remove(&owner);
            }
            Some(v) => {
                if self.views.get(&owner).map(|(_, old)| *old) != Some(v) {
                    self.next_seq += 1;
                    self.views.insert(owner, (self.next_seq, v));
                }
            }
        }
        let wanted = self.effective();
        match (self.sent, wanted) {
            (None, ZoomCommand::Off) => None,
            (Some(prev), now) if prev == now => None,
            (_, now) => {
                self.sent = (now != ZoomCommand::Off).then_some(now);
                Some(now)
            }
        }
    }
}

type Devices = HashMap<String, Entry>;

/// Frame distribution hub.
pub struct Hub {
    source: Source,
    devices: Arc<Mutex<Devices>>,
    zooms: Arc<Mutex<HashMap<String, ZoomBook>>>,
    refresher: AtomicBool,
    /// Zoom commands "published" by a local hub (tests).
    #[cfg(test)]
    zoom_log: std::sync::Mutex<Vec<(String, ZoomCommand)>>,
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
            zooms: Arc::default(),
            refresher: AtomicBool::new(false),
            #[cfg(test)]
            zoom_log: std::sync::Mutex::default(),
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

    /// Inject a zoom frame for `device` (delivered to current listeners).
    #[cfg(test)]
    pub async fn publish_local_zoom(&self, device: &str, frame: SpectrumFrame) {
        if let Some(entry) = self.devices.lock().await.get(device) {
            let _ = entry.zoom_tx.send(Arc::new(frame));
        }
    }

    /// Zoom commands a local hub was asked to send.
    #[cfg(test)]
    pub fn zoom_commands(&self) -> Vec<(String, ZoomCommand)> {
        self.zoom_log
            .lock()
            .map(|log| log.clone())
            .unwrap_or_default()
    }

    /// Record the slice `session` shows on `device` (`(center_hz, span_hz)`, `None` = not
    /// zoomed or gone) and lease the newest wish to the daemon. One slice exists per device: the
    /// most recent change by any session wins, the others keep receiving the full band.
    pub async fn zoom_set(&self, device: &str, session: u64, view: Option<(u64, u32)>) {
        let command = {
            let mut zooms = self.zooms.lock().await;
            let book = zooms.entry(device.to_owned()).or_default();
            let command = book.set(session, view);
            if book.views.is_empty() && book.sent.is_none() {
                zooms.remove(device);
            }
            command
        };
        if let Some(command) = command {
            self.send_zoom(device, command).await;
        }
        if view.is_some() {
            self.start_refresher();
        }
    }

    async fn send_zoom(&self, device: &str, command: ZoomCommand) {
        match &self.source {
            Source::Bus(client) => publish_zoom(client, device, command).await,
            #[cfg(test)]
            Source::Local => {
                if let Ok(mut log) = self.zoom_log.lock() {
                    log.push((device.to_owned(), command));
                }
            }
            Source::Unavailable => {}
        }
    }

    /// Repeat active leases every [`REFRESH`] (started on first use).
    fn start_refresher(&self) {
        let Source::Bus(client) = &self.source else {
            return;
        };
        if self.refresher.swap(true, Ordering::AcqRel) {
            return;
        }
        let client = client.clone();
        let zooms = Arc::clone(&self.zooms);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(REFRESH);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let active: Vec<(String, ZoomCommand)> = zooms
                    .lock()
                    .await
                    .iter()
                    .filter_map(|(dev, book)| book.sent.map(|c| (dev.clone(), c)))
                    .collect();
                for (device, command) in active {
                    publish_zoom(&client, &device, command).await;
                }
            }
        });
    }

    /// Listen to the frames of `device`, starting the bus subscriptions if needed.
    ///
    /// # Errors
    /// See [`HubError`].
    pub async fn subscribe(&self, device: &str) -> Result<Streams, HubError> {
        if matches!(self.source, Source::Unavailable) {
            return Err(HubError::Unavailable);
        }
        let subject =
            superhet_bus::subjects::spectrum(device).map_err(|_| HubError::InvalidDevice)?;
        let zoom_subject =
            superhet_bus::subjects::spectrum_zoom(device).map_err(|_| HubError::InvalidDevice)?;
        let mut devices = self.devices.lock().await;
        if let Some(entry) = devices.get(device) {
            return Ok(Streams {
                full: entry.tx.subscribe(),
                zoom: entry.zoom_tx.subscribe(),
            });
        }
        if devices.len() >= MAX_DEVICES {
            return Err(HubError::TooManyDevices);
        }
        let (tx, rx) = broadcast::channel(CHANNEL_CAPACITY);
        let (zoom_tx, zoom_rx) = broadcast::channel(CHANNEL_CAPACITY);
        let (stop, stopped) = oneshot::channel();
        if let Source::Bus(client) = &self.source {
            let sub = client
                .subscribe(subject)
                .await
                .map_err(|e| HubError::Subscribe(e.to_string()))?;
            let zoom_sub = client
                .subscribe(zoom_subject)
                .await
                .map_err(|e| HubError::Subscribe(e.to_string()))?;
            tokio::spawn(forward(
                sub,
                zoom_sub,
                tx.clone(),
                zoom_tx.clone(),
                stopped,
                Arc::clone(&self.devices),
                device.to_owned(),
            ));
        }
        devices.insert(
            device.to_owned(),
            Entry {
                tx,
                zoom_tx,
                _stop: stop,
            },
        );
        Ok(Streams {
            full: rx,
            zoom: zoom_rx,
        })
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

/// Ask the daemon for a zoom slice (or to stop); best effort, the lease is repeated.
async fn publish_zoom(client: &async_nats::Client, device: &str, command: ZoomCommand) {
    let Ok(subject) = superhet_bus::subjects::ctl_spectrum_zoom(device) else {
        return;
    };
    if let Err(err) = client.publish(subject, command.encode().into()).await {
        tracing::debug!(%err, device, "zoom request not sent");
    }
}

/// Decode bus messages and fan them out until stopped (entry dropped) or the bus ends.
async fn forward(
    mut sub: async_nats::Subscriber,
    mut zoom_sub: async_nats::Subscriber,
    tx: broadcast::Sender<Arc<SpectrumFrame>>,
    zoom_tx: broadcast::Sender<Arc<SpectrumFrame>>,
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
            msg = zoom_sub.next() => {
                let Some(msg) = msg else { break };
                match SpectrumFrame::decode(&msg.payload) {
                    Ok(frame) => drop(zoom_tx.send(Arc::new(frame))),
                    Err(err) => tracing::debug!(%err, device, "dropping undecodable zoom frame"),
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
        assert_eq!(*a.full.recv().await.expect("a"), frame());
        assert_eq!(*b.full.recv().await.expect("b"), frame());
        hub.publish_local_zoom("dev0", frame()).await;
        assert_eq!(*a.zoom.recv().await.expect("a"), frame());
        assert_eq!(*b.zoom.recv().await.expect("b"), frame());
    }

    #[tokio::test]
    async fn slow_listener_loses_old_frames_without_blocking_publisher() {
        let hub = Hub::local();
        let mut slow = hub.subscribe("dev0").await.expect("subscribe");
        for _ in 0..(CHANNEL_CAPACITY * 3) {
            hub.publish_local("dev0", frame()).await;
        }
        assert!(matches!(
            slow.full.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ));
        assert!(slow.full.recv().await.is_ok());
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

    #[test]
    fn zoom_book_latest_change_wins_and_only_changes_are_sent() {
        let mut book = ZoomBook::default();
        let view = |c, s| ZoomCommand::View {
            center_hz: c,
            span_hz: s,
        };
        assert_eq!(book.set(1, None), None, "nothing to turn off");
        assert_eq!(book.set(1, Some((100, 10))), Some(view(100, 10)));
        assert_eq!(book.set(1, Some((100, 10))), None, "unchanged");
        // Session 2 takes over; session 1 re-sending the same view does not take it back.
        assert_eq!(book.set(2, Some((500, 20))), Some(view(500, 20)));
        assert_eq!(book.set(1, Some((100, 10))), None);
        // Session 1 changing its view does.
        assert_eq!(book.set(1, Some((101, 10))), Some(view(101, 10)));
        // The owner leaving hands the lease to the remaining session.
        assert_eq!(book.set(1, None), Some(view(500, 20)));
        assert_eq!(book.set(2, None), Some(ZoomCommand::Off));
        assert_eq!(book.sent, None);
        assert_eq!(book.set(2, None), None);
    }

    #[tokio::test]
    async fn zoom_set_sends_commands_and_forgets_idle_devices() {
        let hub = Hub::local();
        hub.zoom_set("dev0", 1, Some((100_000_000, 50_000))).await;
        hub.zoom_set("dev0", 1, Some((100_000_000, 50_000))).await;
        hub.zoom_set("dev0", 1, None).await;
        assert_eq!(
            hub.zoom_commands(),
            vec![
                (
                    "dev0".to_owned(),
                    ZoomCommand::View {
                        center_hz: 100_000_000,
                        span_hz: 50_000
                    }
                ),
                ("dev0".to_owned(), ZoomCommand::Off),
            ]
        );
        assert!(hub.zooms.lock().await.is_empty());
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
