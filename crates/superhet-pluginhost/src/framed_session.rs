//! The host side of one framed-transport session (`docs/plugin-contract.md`, "Framed
//! transport").
//!
//! The host listens on a Unix socket in a private directory and passes the path in
//! `SUPERHET_SOCKET`. After the `hello` exchange it
//!
//! - writes the instance's input as `samples` frames (sequence numbers that jump over dropped
//!   chunks, host-counted tuning epoch) and a `heartbeat` every few seconds,
//! - writes `control` frames queued by the control API,
//! - reads `event` (validated like a stdout line), `audio` (PCM to the Opus publisher),
//!   `log`, `heartbeat`, `status` (latest document kept for the control API), `media` (latest
//!   object per key kept for the control API) and `tune` (an exclusive plugin moves its claimed
//!   window inside the manifest's range) frames.
//!
//! Every frame from the plugin counts as activity for the watchdog. Any protocol violation or
//! a closed socket ends the session; the supervisor then treats the process as crashed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use superhet_plugin_protocol::PROTOCOL_VERSION;
use superhet_plugin_protocol::ctl::audio_vfo_of;
use superhet_plugin_protocol::framed::{
    self, Frame, FrameDecoder, FrameKind, HostHello, MAX_MEDIA, MAX_MEDIA_KEYS, MAX_STATUS,
    MediaHeader, PluginHello, SamplesHeader, TuneRequest,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::outputs::{AudioOut, AudioSpec, Outputs};
use crate::supervisor::{EventSink, InputHealth, Inputs, MediaObject, Shared, deliver, unix_ms};

/// How long a freshly started plugin has to connect to its socket.
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the plugin has to answer the host's `hello`.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Interval of the host's `heartbeat` frames.
const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);
/// Control messages waiting for the writer.
const CONTROL_QUEUE: usize = 16;

/// Everything a session needs from the supervisor.
pub(crate) struct Session {
    pub(crate) shared: Arc<Shared>,
    pub(crate) sink: Arc<dyn EventSink>,
    pub(crate) inputs: Arc<dyn Inputs>,
    pub(crate) outputs: Arc<dyn Outputs>,
    pub(crate) activity: Arc<AtomicU64>,
    pub(crate) started: Instant,
}

/// Aborts the task when dropped.
struct Aborting<T>(JoinHandle<T>);

impl<T> Drop for Aborting<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Forgets what the plugin told the host when the session ends (the data is stale then).
struct ClearLive(Arc<Shared>);

impl Drop for ClearLive {
    fn drop(&mut self) {
        let mut live = self.0.live();
        live.control = None;
        live.status = None;
        live.media.clear();
    }
}

/// Run the session until it ends; the return value says why.
pub(crate) async fn run(listener: UnixListener, session: Session) -> String {
    match serve(listener, session).await {
        Ok(()) => "the framed session ended".to_owned(),
        Err(reason) => reason,
    }
}

async fn serve(listener: UnixListener, session: Session) -> Result<(), String> {
    let Session {
        shared,
        sink,
        inputs,
        outputs,
        activity,
        started,
    } = session;
    let (stream, _) = tokio::time::timeout(ACCEPT_TIMEOUT, listener.accept())
        .await
        .map_err(|_| "the plugin did not connect to its socket within 20 s".to_owned())?
        .map_err(|err| format!("accepting the plugin connection failed: {err}"))?;
    drop(listener);
    let (read, mut write) = stream.into_split();
    let mut reader = FrameReader::new(read);

    // Handshake: host hello first, then the plugin's.
    let hello = HostHello {
        protocol: PROTOCOL_VERSION,
        plugin_id: shared.package.manifest.plugin.id.clone(),
        instance_id: shared.id.clone(),
        device: shared.spec.device.clone(),
        freq_hz: shared.freq_hz(),
        config: shared.config(),
    };
    send_json(&mut write, FrameKind::Hello, &hello).await?;
    let frame = tokio::time::timeout(HELLO_TIMEOUT, reader.next())
        .await
        .map_err(|_| "the plugin did not answer the hello within 10 s".to_owned())??;
    if frame.kind != FrameKind::Hello {
        return Err("the first frame of the plugin was not a hello".to_owned());
    }
    let reply: PluginHello = serde_json::from_slice(&frame.payload)
        .map_err(|err| format!("malformed plugin hello: {err}"))?;
    if reply.protocol != PROTOCOL_VERSION {
        return Err(format!(
            "the plugin speaks protocol {} but the host speaks {PROTOCOL_VERSION}",
            reply.protocol
        ));
    }
    touch(&activity, started);

    let audio = open_audio(&shared, outputs.as_ref());

    // The input flows only now, so nothing is dropped while the plugin starts up.
    let mut chunks = None;
    let mut retune = None;
    let mut _input_guard: Option<Box<dyn Send>> = None;
    let mut _watcher = None;
    if let Some(mut request) = shared.input.clone() {
        if let (Some(freq), Some(claim)) = (shared.live().freq_hz, request.claim.as_mut()) {
            claim.center_hz = freq;
            request.tap.center_hz = Some(freq);
        }
        let (status, mut state) = watch::channel(None);
        let health = InputHealth {
            dropped: Arc::clone(&shared.input_dropped),
            status,
        };
        if let Some(attachment) = inputs.attach(&request, health) {
            chunks = Some(attachment.chunks);
            retune = attachment.retune;
            _input_guard = Some(attachment.guard);
            let watch_shared = Arc::clone(&shared);
            _watcher = Some(Aborting(tokio::spawn(async move {
                while state.changed().await.is_ok() {
                    let error = state.borrow_and_update().clone();
                    watch_shared.set_input_error(error);
                }
            })));
        }
    }

    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE);
    shared.live().control = Some(control_tx);
    let _clear = ClearLive(Arc::clone(&shared));
    let epoch = Arc::new(AtomicU32::new(0));

    let writer = tokio::spawn(write_loop(
        write,
        chunks,
        control_rx,
        Arc::clone(&shared),
        Arc::clone(&epoch),
    ));
    let mut incoming = Incoming {
        shared: Arc::clone(&shared),
        sink,
        audio,
        retune,
        epoch,
        activity,
        started,
        unexpected: 0,
    };
    let mut writer = Aborting(writer);
    let outcome = tokio::select! {
        result = incoming.read_loop(reader) => result,
        joined = &mut writer.0 => Err(joined.unwrap_or_else(|err| format!("writer task failed: {err}"))),
    };
    outcome
}

fn touch(activity: &AtomicU64, started: Instant) {
    activity.store(
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
}

fn open_audio(shared: &Shared, outputs: &dyn Outputs) -> Option<Box<dyn AudioOut>> {
    let spec = shared.package.manifest.output.audio.as_ref()?;
    let Some(device) = shared.spec.device.clone() else {
        tracing::warn!(instance = %shared.id, "plugin audio needs a device binding; dropping it");
        return None;
    };
    outputs.open_audio(&AudioSpec {
        device,
        vfo_id: audio_vfo_of(&shared.id),
        sample_rate: spec.sample_rate,
        channels: spec.channels,
    })
}

async fn send_json<T: serde::Serialize>(
    write: &mut OwnedWriteHalf,
    kind: FrameKind,
    value: &T,
) -> Result<(), String> {
    let body = serde_json::to_vec(value).map_err(|err| err.to_string())?;
    send(write, kind, &body).await
}

async fn send(write: &mut OwnedWriteHalf, kind: FrameKind, payload: &[u8]) -> Result<(), String> {
    let wire = framed::encode(kind, payload).map_err(|err| err.to_string())?;
    write
        .write_all(&wire)
        .await
        .map_err(|err| format!("writing to the plugin failed: {err}"))
}

/// Reads whole frames from the socket.
struct FrameReader {
    read: OwnedReadHalf,
    decoder: FrameDecoder,
    buf: Vec<u8>,
}

impl FrameReader {
    fn new(read: OwnedReadHalf) -> Self {
        Self {
            read,
            decoder: FrameDecoder::new(),
            buf: vec![0; 64 * 1024],
        }
    }

    async fn next(&mut self) -> Result<Frame, String> {
        loop {
            match self.decoder.next_frame() {
                Ok(Some(frame)) => return Ok(frame),
                Ok(None) => {}
                Err(err) => return Err(format!("protocol violation by the plugin: {err}")),
            }
            let n = self
                .read
                .read(&mut self.buf)
                .await
                .map_err(|err| format!("reading from the plugin failed: {err}"))?;
            if n == 0 {
                return Err("the plugin closed its socket".to_owned());
            }
            self.decoder.push(&self.buf[..n]);
        }
    }
}

/// Frames from the plugin.
struct Incoming {
    shared: Arc<Shared>,
    sink: Arc<dyn EventSink>,
    audio: Option<Box<dyn AudioOut>>,
    retune: Option<watch::Sender<u64>>,
    epoch: Arc<AtomicU32>,
    activity: Arc<AtomicU64>,
    started: Instant,
    unexpected: u64,
}

impl Incoming {
    async fn read_loop(&mut self, mut reader: FrameReader) -> Result<(), String> {
        loop {
            let frame = reader.next().await?;
            touch(&self.activity, self.started);
            self.handle(frame)?;
        }
    }

    fn handle(&mut self, frame: Frame) -> Result<(), String> {
        match frame.kind {
            FrameKind::Event => {
                let verdict = self
                    .shared
                    .package
                    .validator
                    .check_line(&frame.payload)
                    .map_err(|err| err.to_string());
                deliver(&self.shared, self.sink.as_ref(), verdict);
            }
            FrameKind::Audio => {
                if let Some(audio) = self.audio.as_mut() {
                    audio.push(&frame.payload);
                }
            }
            FrameKind::Log => {
                let text = String::from_utf8_lossy(&frame.payload);
                let text = text.trim_end();
                if !text.is_empty() {
                    tracing::info!(
                        instance = %self.shared.id,
                        plugin = %self.shared.package.manifest.plugin.id,
                        "{text}"
                    );
                }
            }
            FrameKind::Heartbeat => {}
            FrameKind::Status => self.status(&frame.payload),
            FrameKind::Media => self.media(&frame.payload),
            FrameKind::Tune => self.tune(&frame.payload),
            // The host does not take voice frames yet (no codec plugin consumes them); anything
            // else is not valid in this direction.
            FrameKind::VoiceFrame => self.note_unexpected("voice_frame"),
            FrameKind::Hello | FrameKind::Samples | FrameKind::Control => {
                return Err(format!(
                    "the plugin sent a {:?} frame, which only the host may send",
                    frame.kind
                ));
            }
        }
        Ok(())
    }

    fn note_unexpected(&mut self, what: &str) {
        self.unexpected += 1;
        if self.unexpected == 1 || self.unexpected.is_multiple_of(1000) {
            tracing::warn!(
                instance = %self.shared.id,
                total = self.unexpected,
                "dropped a {what} frame of the plugin"
            );
        }
    }

    fn status(&mut self, payload: &[u8]) {
        if payload.len() > MAX_STATUS {
            self.note_unexpected("oversized status");
            return;
        }
        match serde_json::from_slice::<Value>(payload) {
            Ok(doc) if doc.is_object() => {
                self.shared.live().status = Some((doc, unix_ms()));
            }
            _ => self.note_unexpected("malformed status"),
        }
    }

    fn media(&mut self, payload: &[u8]) {
        let Ok((header, bytes)) = MediaHeader::decode(payload) else {
            self.note_unexpected("malformed media");
            return;
        };
        if !MediaHeader::valid_key(&header.key) || bytes.len() > MAX_MEDIA || bytes.is_empty() {
            self.note_unexpected("invalid media");
            return;
        }
        let mut live = self.shared.live();
        let revision = live.media.get(&header.key).map_or(0, |m| m.revision) + 1;
        if revision == 1 && live.media.len() >= MAX_MEDIA_KEYS {
            drop(live);
            self.note_unexpected("media (too many keys)");
            return;
        }
        live.media.insert(
            header.key,
            MediaObject {
                mime: header.mime,
                bytes: Arc::new(bytes.to_vec()),
                revision,
            },
        );
    }

    fn tune(&mut self, payload: &[u8]) {
        let Ok(request) = serde_json::from_slice::<TuneRequest>(payload) else {
            self.note_unexpected("malformed tune");
            return;
        };
        let manifest = &self.shared.package.manifest;
        let device = &manifest.device;
        let allowed = manifest.is_exclusive()
            && device.center_hz.is_none()
            && matches!(
                (device.center_hz_min, device.center_hz_max),
                (Some(min), Some(max)) if (min..=max).contains(&request.freq_hz)
            );
        let Some(retune) = self.retune.as_ref().filter(|_| allowed) else {
            tracing::warn!(
                instance = %self.shared.id,
                freq_hz = request.freq_hz,
                "refused a tune request (not exclusive, fixed centre, outside the declared range or no claim)"
            );
            return;
        };
        if self.shared.freq_hz() == Some(request.freq_hz) {
            return;
        }
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.shared.set_freq(request.freq_hz);
        let _ = retune.send(request.freq_hz);
        tracing::info!(instance = %self.shared.id, freq_hz = request.freq_hz, "plugin moved its window");
    }
}

/// Resolves with the next chunk; never completes once the queue is gone or absent.
async fn next_chunk(chunks: &mut Option<mpsc::Receiver<Vec<u8>>>) -> Vec<u8> {
    if let Some(rx) = chunks.as_mut() {
        if let Some(chunk) = rx.recv().await {
            return chunk;
        }
        *chunks = None;
    }
    std::future::pending().await
}

async fn write_loop(
    mut write: OwnedWriteHalf,
    mut chunks: Option<mpsc::Receiver<Vec<u8>>>,
    mut control: mpsc::Receiver<Vec<u8>>,
    shared: Arc<Shared>,
    epoch: Arc<AtomicU32>,
) -> String {
    let mut beat = tokio::time::interval(HEARTBEAT_EVERY);
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut seq: u64 = 0;
    let mut seen_dropped: u64 = 0;
    loop {
        let sent = tokio::select! {
            chunk = next_chunk(&mut chunks) => {
                // Chunks the host had to drop show up as a jump in `seq`.
                let dropped = shared.input_dropped.load(Ordering::Relaxed);
                seq += dropped.saturating_sub(seen_dropped);
                seen_dropped = dropped;
                let header = SamplesHeader {
                    seq,
                    timestamp_ns: unix_ns(),
                    tuning_epoch: epoch.load(Ordering::Relaxed),
                };
                seq += 1;
                crate::metrics::input_chunk(chunk.len());
                send(&mut write, FrameKind::Samples, &header.encode(&chunk)).await
            }
            Some(message) = control.recv() => send(&mut write, FrameKind::Control, &message).await,
            _ = beat.tick() => send(&mut write, FrameKind::Heartbeat, &[]).await,
        };
        if let Err(reason) = sent {
            return reason;
        }
    }
}

fn unix_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}
