//! Shares one bus subscription per (device, VFO) between all audio WebSocket clients.
//!
//! Every stream (`sh.v1.dev.<dev>.audio.<vfo>`) has a forwarder task that validates the
//! `AudioFrame` v1 messages and pushes the unchanged bytes into the bounded queue of each client
//! subscribed to it. The same task also follows the VFO's RDS state
//! (`sh.v1.dev.<dev>.rds.<vfo>`, JSON) and queues each valid message as a text frame. A queue that is full drops its oldest frame and counts it, so a slow client
//! loses audio but never delays the forwarder, the bus subscription or other clients. The bus
//! subscription is created for the first client of a stream and dropped with the last one.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};

use axum::body::Bytes;
use futures_util::StreamExt;
use superhet_bus::audio::AudioFrame;
use superhet_bus::rds::RdsMessage;
use tokio::sync::{Mutex, Notify, oneshot};

/// Frames buffered per client before the oldest are dropped (20 ms each, shared by all VFOs).
pub const QUEUE_CAPACITY: usize = 32;
/// Upper bound on distinct (device, VFO) streams forwarded at the same time.
const MAX_STREAMS: usize = 64;

/// Where frames come from.
enum Source {
    /// NATS subjects published by sdrd.
    Bus(async_nats::Client),
    /// In-process injection via `AudioHub::publish_local` (tests).
    #[cfg(test)]
    Local,
    /// No bus configured; nothing can be streamed.
    Unavailable,
}

/// Why a stream could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum HubError {
    /// No bus is configured.
    #[error("audio source unavailable")]
    Unavailable,
    /// The device id is not usable as a subject token.
    #[error("invalid device id")]
    InvalidDevice,
    /// Too many distinct streams are forwarded at once.
    #[error("too many audio streams")]
    TooManyStreams,
    /// The bus subscription failed.
    #[error("subscribe failed: {0}")]
    Subscribe(String),
}

/// One message for a client: a binary audio frame or a text (JSON) message of a VFO.
#[derive(Debug, Clone)]
pub struct Queued {
    /// VFO the message belongs to.
    vfo: u32,
    /// True for a text message (RDS state), false for a binary audio frame.
    pub text: bool,
    /// The bytes exactly as published.
    pub data: Bytes,
}

impl std::ops::Deref for Queued {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.data
    }
}

/// Bounded message queue of one client; drops the oldest message when full.
pub struct ClientQueue {
    frames: StdMutex<VecDeque<Queued>>,
    ready: Notify,
    capacity: usize,
    dropped: AtomicU64,
}

impl ClientQueue {
    fn new(capacity: usize) -> Self {
        Self {
            frames: StdMutex::new(VecDeque::with_capacity(capacity)),
            ready: Notify::new(),
            capacity,
            dropped: AtomicU64::new(0),
        }
    }

    /// Queue a frame, dropping (and counting) the oldest one if the queue is full. Never blocks.
    pub fn push(&self, frame: Bytes) {
        // The VFO id sits at header bytes 12..16 (frames were validated before).
        let vfo = frame
            .get(12..16)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map_or(0, u32::from_le_bytes);
        self.enqueue(Queued {
            vfo,
            text: false,
            data: frame,
        });
    }

    /// Queue a text message of `vfo` like [`Self::push`].
    pub fn push_text(&self, vfo: u32, text: Bytes) {
        self.enqueue(Queued {
            vfo,
            text: true,
            data: text,
        });
    }

    fn enqueue(&self, item: Queued) {
        {
            let mut frames = self.frames.lock().unwrap_or_else(PoisonError::into_inner);
            if frames.len() >= self.capacity {
                frames.pop_front();
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            frames.push_back(item);
        }
        // Stores a permit when nobody waits, so a push is never missed.
        self.ready.notify_one();
    }

    /// Take the oldest queued message without waiting.
    pub fn try_pop(&self) -> Option<Queued> {
        self.frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    /// Discard queued messages whose VFO is not in `vfos`.
    fn retain_vfos(&self, vfos: &BTreeSet<u32>) {
        self.frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|item| vfos.contains(&item.vfo));
    }

    /// Wait for the next message. Cancel safe.
    pub async fn pop(&self) -> Queued {
        loop {
            if let Some(frame) = self.try_pop() {
                return frame;
            }
            self.ready.notified().await;
        }
    }

    /// Frames dropped so far because the client was too slow.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

type Subscribers = Arc<StdMutex<Vec<(u64, Arc<ClientQueue>)>>>;

/// A shared per-stream forwarder. Dropping it stops the bus subscription feeding it.
struct Stream {
    subscribers: Subscribers,
    _stop: oneshot::Sender<()>,
}

type StreamKey = (String, u32);
type Streams = HashMap<StreamKey, Stream>;

/// Audio frame distribution hub.
pub struct AudioHub {
    source: Source,
    streams: Arc<Mutex<Streams>>,
    next_client: AtomicU64,
}

impl AudioHub {
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
            streams: Arc::default(),
            next_client: AtomicU64::new(1),
        }
    }

    /// Whether a bus is configured at all.
    #[must_use]
    pub fn available(&self) -> bool {
        !matches!(self.source, Source::Unavailable)
    }

    /// Number of streams currently forwarded.
    #[cfg(test)]
    pub async fn stream_count(&self) -> usize {
        self.streams.lock().await.len()
    }

    /// Inject a raw frame for a stream (delivered to its current subscribers).
    #[cfg(test)]
    pub async fn publish_local(&self, device: &str, vfo: u32, frame: Vec<u8>) {
        let subscribers = self
            .streams
            .lock()
            .await
            .get(&(device.to_owned(), vfo))
            .map(|stream| Arc::clone(&stream.subscribers));
        if let Some(subscribers) = subscribers {
            deliver(&subscribers, &Item::Audio(Bytes::from(frame)));
        }
    }

    /// Inject an RDS message for a stream, going through the same validation as the bus path.
    #[cfg(test)]
    pub async fn publish_local_rds(&self, device: &str, vfo: u32, message: &[u8]) {
        let subscribers = self
            .streams
            .lock()
            .await
            .get(&(device.to_owned(), vfo))
            .map(|stream| Arc::clone(&stream.subscribers));
        if let Some(subscribers) = subscribers
            && let Some(item) = rds_item(message, vfo)
        {
            deliver(&subscribers, &item);
        }
    }

    /// Open a client session for `device`. Fails early for an unusable device id or no bus.
    ///
    /// # Errors
    /// See [`HubError`].
    pub fn session(self: &Arc<Self>, device: &str) -> Result<Session, HubError> {
        if !self.available() {
            return Err(HubError::Unavailable);
        }
        // Validates the device token; the VFO token is any decimal number.
        superhet_bus::subjects::audio(device, "0").map_err(|_| HubError::InvalidDevice)?;
        Ok(Session {
            hub: Arc::clone(self),
            device: device.to_owned(),
            id: self.next_client.fetch_add(1, Ordering::Relaxed),
            queue: Arc::new(ClientQueue::new(QUEUE_CAPACITY)),
            vfos: BTreeSet::new(),
        })
    }

    async fn add(&self, key: StreamKey, id: u64, queue: &Arc<ClientQueue>) -> Result<(), HubError> {
        let mut streams = self.streams.lock().await;
        if let Some(stream) = streams.get(&key) {
            let mut subscribers = stream
                .subscribers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            subscribers.push((id, Arc::clone(queue)));
            return Ok(());
        }
        if streams.len() >= MAX_STREAMS {
            return Err(HubError::TooManyStreams);
        }
        let subscribers: Subscribers = Arc::new(StdMutex::new(vec![(id, Arc::clone(queue))]));
        let (stop, stopped) = oneshot::channel();
        if let Source::Bus(client) = &self.source {
            let subject = superhet_bus::subjects::audio(&key.0, &key.1.to_string())
                .map_err(|_| HubError::InvalidDevice)?;
            let sub = client
                .subscribe(subject)
                .await
                .map_err(|e| HubError::Subscribe(e.to_string()))?;
            // RDS is a bonus: audio keeps working when only this subscription fails.
            let rds = match superhet_bus::subjects::rds(&key.0, &key.1.to_string()) {
                Ok(subject) => client
                    .subscribe(subject)
                    .await
                    .inspect_err(|e| tracing::warn!(error = %e, "rds subscribe failed"))
                    .ok(),
                Err(_) => None,
            };
            tokio::spawn(forward(
                sub,
                rds,
                Arc::clone(&subscribers),
                stopped,
                Arc::clone(&self.streams),
                key.clone(),
            ));
        }
        streams.insert(
            key,
            Stream {
                subscribers,
                _stop: stop,
            },
        );
        Ok(())
    }

    async fn remove(&self, key: &StreamKey, id: u64) {
        let mut streams = self.streams.lock().await;
        let Some(stream) = streams.get(key) else {
            return;
        };
        let empty = {
            let mut subscribers = stream
                .subscribers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            subscribers.retain(|(client, _)| *client != id);
            subscribers.is_empty()
        };
        if empty {
            // Dropping the entry stops the forwarder and with it the bus subscription.
            streams.remove(key);
        }
    }
}

/// A validated message on its way to the client queues.
enum Item {
    Audio(Bytes),
    Rds { vfo: u32, json: Bytes },
}

/// Validate an RDS message of the stream's VFO.
fn rds_item(payload: &[u8], vfo: u32) -> Option<Item> {
    match RdsMessage::parse(payload) {
        Ok(message) if message.vfo == vfo => Some(Item::Rds {
            vfo,
            json: Bytes::copy_from_slice(payload),
        }),
        Ok(_) => {
            tracing::debug!(vfo, "dropping rds message with foreign vfo id");
            None
        }
        Err(err) => {
            tracing::debug!(%err, vfo, "dropping invalid rds message");
            None
        }
    }
}

/// Push a message into every subscribed client queue.
fn deliver(subscribers: &Subscribers, item: &Item) {
    let subscribers = subscribers.lock().unwrap_or_else(PoisonError::into_inner);
    for (_, queue) in subscribers.iter() {
        match item {
            Item::Audio(frame) => queue.push(frame.clone()),
            Item::Rds { vfo, json } => queue.push_text(*vfo, json.clone()),
        }
    }
}

/// Wait for the next message of an optional subscription; never ready when there is none.
async fn next_rds(sub: &mut Option<async_nats::Subscriber>) -> Option<async_nats::Message> {
    match sub {
        Some(sub) => sub.next().await,
        None => std::future::pending().await,
    }
}

/// Validate bus messages and fan them out until stopped (entry dropped) or the bus ends.
async fn forward(
    mut sub: async_nats::Subscriber,
    mut rds: Option<async_nats::Subscriber>,
    subscribers: Subscribers,
    mut stopped: oneshot::Receiver<()>,
    registry: Arc<Mutex<Streams>>,
    key: StreamKey,
) {
    loop {
        tokio::select! {
            _ = &mut stopped => return,
            msg = sub.next() => {
                let Some(msg) = msg else { break };
                match AudioFrame::decode(&msg.payload) {
                    Ok(frame) if frame.vfo_id == key.1 => {
                        deliver(&subscribers, &Item::Audio(msg.payload));
                    }
                    Ok(_) => tracing::debug!(device = key.0, vfo = key.1, "dropping audio frame with foreign vfo id"),
                    Err(err) => tracing::debug!(%err, device = key.0, vfo = key.1, "dropping undecodable audio frame"),
                }
            }
            msg = next_rds(&mut rds) => {
                match msg {
                    Some(msg) => {
                        if let Some(item) = rds_item(&msg.payload, key.1) {
                            deliver(&subscribers, &item);
                        }
                    }
                    // Only the RDS subscription ended: carry on with audio alone.
                    None => rds = None,
                }
            }
        }
    }
    // The subscription ended: forget the entry (unless it was already replaced) so that the
    // next subscriber resubscribes.
    let mut streams = registry.lock().await;
    if streams
        .get(&key)
        .is_some_and(|stream| Arc::ptr_eq(&stream.subscribers, &subscribers))
    {
        streams.remove(&key);
    }
}

/// One client's view on the hub: its queue and the VFOs it is subscribed to.
pub struct Session {
    hub: Arc<AudioHub>,
    device: String,
    id: u64,
    queue: Arc<ClientQueue>,
    vfos: BTreeSet<u32>,
}

impl Session {
    /// The queue that receives this client's frames.
    #[must_use]
    pub fn queue(&self) -> &Arc<ClientQueue> {
        &self.queue
    }

    /// The VFOs currently subscribed.
    #[must_use]
    pub fn vfos(&self) -> &BTreeSet<u32> {
        &self.vfos
    }

    /// Replace the subscribed set with `wanted`: unsubscribes from the VFOs no longer listed and
    /// subscribes to the new ones. On error the session keeps what was applied so far.
    ///
    /// # Errors
    /// See [`HubError`].
    pub async fn set(&mut self, wanted: &BTreeSet<u32>) -> Result<(), HubError> {
        let stale: Vec<u32> = self.vfos.difference(wanted).copied().collect();
        for vfo in stale {
            self.hub.remove(&(self.device.clone(), vfo), self.id).await;
            self.vfos.remove(&vfo);
        }
        let added: Vec<u32> = wanted.difference(&self.vfos).copied().collect();
        for vfo in added {
            self.hub
                .add((self.device.clone(), vfo), self.id, &self.queue)
                .await?;
            self.vfos.insert(vfo);
        }
        // Frames of VFOs the client left must not be delivered after the acknowledgement.
        self.queue.retain_vfos(&self.vfos);
        Ok(())
    }

    /// Leave all streams. Call when the client is gone.
    pub async fn close(mut self) {
        let empty = BTreeSet::new();
        // Removal cannot fail.
        let _ = self.set(&empty).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(vfos: &[u32]) -> BTreeSet<u32> {
        vfos.iter().copied().collect()
    }

    /// A stand-in frame: 16 bytes with the VFO id at offset 12 and a tag in byte 0.
    fn raw(vfo: u32, tag: u8) -> Vec<u8> {
        let mut data = vec![0u8; 16];
        data[0] = tag;
        data[12..16].copy_from_slice(&vfo.to_le_bytes());
        data
    }

    fn frame(tag: u8) -> Bytes {
        Bytes::from(raw(1, tag))
    }

    #[test]
    fn full_queue_drops_the_oldest_and_counts() {
        let queue = ClientQueue::new(3);
        for tag in 0..5 {
            queue.push(frame(tag));
        }
        assert_eq!(queue.dropped(), 2);
        let rest: Vec<u8> = std::iter::from_fn(|| queue.try_pop())
            .map(|f| f[0])
            .collect();
        assert_eq!(rest, vec![2, 3, 4]);
        assert_eq!(queue.dropped(), 2);
    }

    #[tokio::test]
    async fn pop_waits_for_a_push() {
        let queue = Arc::new(ClientQueue::new(3));
        let waiter = tokio::spawn({
            let queue = Arc::clone(&queue);
            async move { queue.pop().await }
        });
        tokio::task::yield_now().await;
        queue.push(frame(7));
        assert_eq!(waiter.await.expect("join")[0], 7);
        // A push before the wait is not lost either.
        queue.push(frame(8));
        assert_eq!(queue.pop().await[0], 8);
    }

    #[tokio::test]
    async fn frames_reach_only_subscribed_vfos() {
        let hub = Arc::new(AudioHub::local());
        let mut a = hub.session("dev0").expect("session");
        let mut b = hub.session("dev0").expect("session");
        a.set(&set(&[1, 2])).await.expect("set");
        b.set(&set(&[2])).await.expect("set");
        hub.publish_local("dev0", 1, raw(1, 1)).await;
        hub.publish_local("dev0", 2, raw(2, 2)).await;
        hub.publish_local("dev0", 3, raw(3, 3)).await;
        hub.publish_local("other", 1, raw(1, 9)).await;
        let drain = |s: &Session| -> Vec<u8> {
            std::iter::from_fn(|| s.queue().try_pop())
                .map(|f| f[0])
                .collect()
        };
        assert_eq!(drain(&a), vec![1, 2]);
        assert_eq!(drain(&b), vec![2]);
    }

    #[tokio::test]
    async fn streams_are_refcounted() {
        let hub = Arc::new(AudioHub::local());
        let mut a = hub.session("dev0").expect("session");
        let mut b = hub.session("dev0").expect("session");
        a.set(&set(&[1, 2])).await.expect("set");
        b.set(&set(&[2, 3])).await.expect("set");
        assert_eq!(hub.stream_count().await, 3);
        // Shrinking releases vfo 1 only; vfo 2 is still used by b.
        a.set(&set(&[2])).await.expect("set");
        assert_eq!(hub.stream_count().await, 3 - 1);
        a.set(&set(&[])).await.expect("set");
        assert_eq!(hub.stream_count().await, 2);
        assert!(a.vfos().is_empty());
        // Setting the same set again is a no-op; closing frees the rest.
        b.set(&set(&[2, 3])).await.expect("set");
        assert_eq!(hub.stream_count().await, 2);
        b.close().await;
        assert_eq!(hub.stream_count().await, 0);
        a.close().await;
    }

    #[tokio::test]
    async fn leaving_a_vfo_discards_its_queued_frames() {
        let hub = Arc::new(AudioHub::local());
        let mut a = hub.session("dev0").expect("session");
        a.set(&set(&[1, 2])).await.expect("set");
        hub.publish_local("dev0", 1, raw(1, 1)).await;
        hub.publish_local("dev0", 2, raw(2, 2)).await;
        a.set(&set(&[2])).await.expect("set");
        assert_eq!(a.queue().try_pop().map(|f| f[0]), Some(2));
        assert!(a.queue().try_pop().is_none());
    }

    #[tokio::test]
    async fn slow_client_does_not_affect_others() {
        let hub = Arc::new(AudioHub::local());
        let mut slow = hub.session("dev0").expect("session");
        let mut fast = hub.session("dev0").expect("session");
        slow.set(&set(&[1])).await.expect("set");
        fast.set(&set(&[1])).await.expect("set");
        let total = QUEUE_CAPACITY * 2;
        let mut seen = 0usize;
        for i in 0..total {
            hub.publish_local("dev0", 1, raw(1, i as u8)).await;
            while fast.queue().try_pop().is_some() {
                seen += 1;
            }
        }
        assert_eq!(seen, total);
        assert_eq!(fast.queue().dropped(), 0);
        assert_eq!(slow.queue().dropped() as usize, total - QUEUE_CAPACITY);
    }

    #[tokio::test]
    async fn rejects_invalid_device_unavailable_bus_and_stream_flood() {
        let hub = Arc::new(AudioHub::local());
        assert!(matches!(hub.session("a.b"), Err(HubError::InvalidDevice)));
        assert!(matches!(
            Arc::new(AudioHub::new(None)).session("dev0"),
            Err(HubError::Unavailable)
        ));
        let mut sessions = Vec::new();
        for d in 0..(MAX_STREAMS / 8) {
            let mut s = hub.session(&format!("d{d}")).expect("session");
            s.set(&(1..=8).collect()).await.expect("set");
            sessions.push(s);
        }
        let mut extra = hub.session("extra").expect("session");
        assert!(matches!(
            extra.set(&set(&[1])).await,
            Err(HubError::TooManyStreams)
        ));
        assert!(extra.vfos().is_empty());
    }
}
