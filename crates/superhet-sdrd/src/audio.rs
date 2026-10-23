//! VFO audio: one dedicated thread runs a [`Channel`] per active VFO over the shared IQ blocks,
//! encodes 48 kHz mono audio to 20 ms Opus frames and hands them out as encoded
//! [`AudioFrame`]s.
//!
//! The spectrum thread shares each IQ block as an `Arc` through an [`IqTap`] (no copy). The tap
//! queue is bounded; when this thread lags, blocks are dropped and counted, so VFO DSP can never
//! stall the spectrum path or the device.
//!
//! The thread follows the VFO registry through [`VfoShared::generation`] and the device tuning
//! through the blocks themselves: on every tuning epoch change each channel's offset is
//! recomputed and the channel is reset; VFOs that do not fit the passband are paused (no DSP, no
//! frames, no encoder state kept beyond the counters).

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use superhet_bus::audio::{AudioFrame, FLAG_SQUELCH_OPEN, FLAG_STEREO, FRAME_SAMPLES};
use superhet_bus::opus::Encoder;
use superhet_bus::rds::RdsMessage;
use superhet_device::IqBlock;
use superhet_dsp::{Channel, Mode, OUTPUT_RATE};

use crate::vfo::{Status, Tuning, Vfo, VfoShared};

/// IQ blocks buffered between the spectrum thread and the VFO thread.
const QUEUE_BLOCKS: usize = 16;
/// Blocks ignored after a retune while the front end settles (as in the spectrum path).
const SETTLE_BLOCKS: usize = 1;
/// Wake-up interval to notice a stop request while no IQ arrives.
const POLL: Duration = Duration::from_millis(100);
/// Interval for reporting dropped IQ blocks.
const DROP_REPORT: Duration = Duration::from_secs(5);
/// Opus bitrate for mono WFM in bits per second.
const BITRATE_WFM: i32 = 64_000;
/// Opus bitrate for stereo WFM in bits per second.
const BITRATE_WFM_STEREO: i32 = 96_000;
/// An unchanged non-empty RDS state is repeated this often, so late subscribers catch up.
const RDS_REPEAT: Duration = Duration::from_secs(1);
/// Opus bitrate for NFM and AM in bits per second.
const BITRATE_NARROW: i32 = 24_000;
/// Upper bound for one Opus packet (RFC 6716: 1275 bytes per frame).
const MAX_PACKET: usize = 1500;

/// Sending end of the IQ tap; cloneable and never blocking.
#[derive(Clone)]
pub struct IqTap {
    tx: SyncSender<Arc<IqBlock>>,
    dropped: Arc<AtomicU64>,
}

impl IqTap {
    /// Offer a block; it is dropped and counted when the consumer lags. Returns whether it was
    /// accepted.
    pub fn send(&self, block: &Arc<IqBlock>) -> bool {
        match self.tx.try_send(Arc::clone(block)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Blocks dropped so far because the consumer lagged.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Running VFO audio thread; dropping it stops and joins the thread.
pub struct AudioEngine {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    tap: IqTap,
}

impl AudioEngine {
    /// Start the thread without RDS publishing, see [`Self::start_with_rds`].
    ///
    /// # Errors
    /// Fails if the thread cannot be spawned.
    #[cfg(test)]
    pub fn start(
        vfos: Arc<VfoShared>,
        sink: impl FnMut(u32, Vec<u8>) + Send + 'static,
    ) -> io::Result<Self> {
        Self::start_with_rds(vfos, sink, |_, _| {})
    }

    /// Start the thread. `sink` receives `(vfo_id, encoded AudioFrame)`, `rds_sink` receives
    /// `(vfo_id, RDS state JSON)` (see `superhet_bus::rds`); both run on the engine thread and
    /// must never block.
    ///
    /// # Errors
    /// Fails if the thread cannot be spawned.
    pub fn start_with_rds(
        vfos: Arc<VfoShared>,
        sink: impl FnMut(u32, Vec<u8>) + Send + 'static,
        rds_sink: impl FnMut(u32, Vec<u8>) + Send + 'static,
    ) -> io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel(QUEUE_BLOCKS);
        let tap = IqTap {
            tx,
            dropped: Arc::new(AtomicU64::new(0)),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            let tap = tap.clone();
            std::thread::Builder::new()
                .name("sdrd-vfo".into())
                .spawn(move || run(&rx, &vfos, &stop, &tap, sink, rds_sink))?
        };
        Ok(Self {
            stop,
            thread: Some(thread),
            tap,
        })
    }

    /// Handle for feeding IQ blocks.
    #[must_use]
    pub fn tap(&self) -> IqTap {
        self.tap.clone()
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("vfo thread panicked");
        }
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn bitrate_for(mode: Mode, channels: usize) -> i32 {
    match mode {
        Mode::Wfm { .. } if channels == 2 => BITRATE_WFM_STEREO,
        Mode::Wfm { .. } => BITRATE_WFM,
        Mode::Nfm { .. } | Mode::Am | Mode::Usb | Mode::Lsb | Mode::Cw { .. } => BITRATE_NARROW,
    }
}

/// What the channel was built with; a change forces a rebuild.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Built {
    mode: Mode,
    bandwidth_hz: Option<u32>,
    input_rate: u32,
}

/// Runtime state of one VFO.
struct Slot {
    vfo: Vfo,
    channel: Option<Channel>,
    built: Option<Built>,
    encoder: Option<Encoder>,
    bitrate: i32,
    /// Audio not yet forming a full frame (interleaved if the encoder has two channels).
    pcm: Vec<f32>,
    scratch: Vec<f32>,
    seq: u32,
    /// Position of the first sample of the next frame (= samples framed so far).
    sample_pos: u64,
    /// Samples produced so far; always `sample_pos + pcm.len()`.
    produced: u64,
    /// `(sample index, capture ns)` at the start of recent IQ blocks.
    timeline: VecDeque<(u64, u64)>,
    power_dbfs: f32,
    squelch_open: bool,
    /// The stereo pilot is locked (at the end of the last block).
    stereo_locked: bool,
    /// Last published RDS state `(revision, synced)`; the 1 s repeat timer.
    rds_seen: (u64, bool),
    rds_at: Instant,
    /// A non-empty RDS state was published and has not been cleared since.
    rds_active: bool,
    /// The channel was replaced or removed: publish an empty RDS state.
    rds_clear: bool,
}

impl Slot {
    fn new(vfo: Vfo) -> Self {
        Self {
            vfo,
            channel: None,
            built: None,
            encoder: None,
            bitrate: 0,
            pcm: Vec::new(),
            scratch: Vec::new(),
            seq: 0,
            sample_pos: 0,
            produced: 0,
            timeline: VecDeque::new(),
            power_dbfs: -200.0,
            squelch_open: true,
            stereo_locked: false,
            rds_seen: (0, false),
            rds_at: Instant::now(),
            rds_active: false,
            rds_clear: false,
        }
    }

    /// Drop buffered audio and its timing; the counters keep running.
    fn discard_audio(&mut self) {
        self.pcm.clear();
        self.produced = self.sample_pos;
        self.timeline.clear();
    }

    fn deactivate(&mut self) {
        self.rds_clear |= self.rds_active;
        self.channel = None;
        self.built = None;
        self.discard_audio();
    }

    /// Bring the channel in line with the VFO and the tuning; `reset` clears the filter state.
    fn configure(&mut self, tuning: Tuning, reset: bool) {
        if self.vfo.status(tuning) == Status::Paused {
            self.deactivate();
            return;
        }
        let cfg = self.vfo.channel_config(tuning);
        let built = Built {
            mode: cfg.mode,
            bandwidth_hz: cfg.bandwidth_hz,
            input_rate: cfg.input_rate,
        };
        match self.channel.as_mut().filter(|_| self.built == Some(built)) {
            Some(channel) => {
                if channel.set_offset(cfg.offset_hz).is_err() {
                    self.deactivate();
                    return;
                }
                if channel.config().squelch_dbfs != cfg.squelch_dbfs {
                    channel.set_squelch(cfg.squelch_dbfs);
                }
                if reset {
                    channel.reset();
                    self.discard_audio();
                }
            }
            None => match Channel::new(cfg) {
                Ok(channel) => {
                    self.rds_clear |= self.rds_active;
                    self.rds_seen = (0, false);
                    self.channel = Some(channel);
                    self.built = Some(built);
                    self.discard_audio();
                }
                Err(e) => {
                    tracing::warn!(vfo = self.vfo.id, error = %e, "cannot run VFO; pausing it");
                    self.deactivate();
                    return;
                }
            },
        }
        let Some(channel) = self.channel.as_mut() else {
            return;
        };
        channel.set_stereo(self.vfo.stereo_enabled());
        let channels = channel.output_channels();
        self.ensure_encoder(bitrate_for(cfg.mode, channels), channels);
    }

    /// Make sure the encoder has `channels` channels and the given bitrate.
    fn ensure_encoder(&mut self, bitrate: i32, channels: usize) {
        if self
            .encoder
            .as_ref()
            .is_some_and(|e| e.channels() != channels)
        {
            // Mono/stereo switch: the stream format changes, start the encoder afresh.
            self.encoder = None;
            self.discard_audio();
        }
        match &mut self.encoder {
            Some(encoder) => {
                if bitrate != self.bitrate && encoder.set_bitrate(bitrate).is_ok() {
                    self.bitrate = bitrate;
                }
            }
            None => match Encoder::with_channels(channels, bitrate) {
                Ok(encoder) => {
                    self.encoder = Some(encoder);
                    self.bitrate = bitrate;
                }
                Err(e) => {
                    tracing::error!(vfo = self.vfo.id, error = %e, "cannot create opus encoder")
                }
            },
        }
    }

    /// Publish the RDS state when it changed, and repeat a non-empty one every second.
    fn publish_rds(&mut self, rds_sink: &mut impl FnMut(u32, Vec<u8>)) {
        let id = self.vfo.id;
        let message = match self.channel.as_ref() {
            Some(channel) => {
                let seen = (channel.rds_revision(), channel.rds_synced());
                let info = channel.rds_info();
                let nonempty = seen.1 || info.is_some_and(|i| !i.is_empty());
                let due = nonempty && self.rds_at.elapsed() >= RDS_REPEAT;
                if !self.rds_clear && seen == self.rds_seen && !due {
                    return;
                }
                self.rds_seen = seen;
                let mut message = RdsMessage::empty(id);
                if let Some(info) = info {
                    message.synced = seen.1;
                    message.pi = info.pi.map(|pi| format!("{pi:04X}"));
                    message.pty = info.pty;
                    message.tp = info.tp;
                    message.ta = info.ta;
                    message.ms = info.music;
                    message.ps.clone_from(&info.ps);
                    message.rt.clone_from(&info.rt);
                }
                message
            }
            None => {
                if !self.rds_clear {
                    return;
                }
                RdsMessage::empty(id)
            }
        };
        self.rds_clear = false;
        self.rds_active = !message.is_empty();
        self.rds_at = Instant::now();
        rds_sink(id, message.encode());
    }

    /// Demodulate one block and emit every completed frame.
    fn process(
        &mut self,
        block: &IqBlock,
        block_start_ns: u64,
        sink: &mut impl FnMut(u32, Vec<u8>),
        rds_sink: &mut impl FnMut(u32, Vec<u8>),
    ) {
        self.process_audio(block, block_start_ns, sink);
        self.publish_rds(rds_sink);
    }

    fn process_audio(
        &mut self,
        block: &IqBlock,
        block_start_ns: u64,
        sink: &mut impl FnMut(u32, Vec<u8>),
    ) {
        let (Some(channel), Some(encoder)) = (self.channel.as_mut(), self.encoder.as_mut()) else {
            return;
        };
        let channels = encoder.channels();
        if channel.output_channels() != channels {
            // The encoder follows on the next `configure`; do not feed it the wrong layout.
            return;
        }
        self.timeline.push_back((self.produced, block_start_ns));
        self.scratch.clear();
        let stats = channel.process(&block.samples, &mut self.scratch);
        self.power_dbfs = stats.power_dbfs;
        self.squelch_open = stats.squelch_open;
        self.stereo_locked = channels == 2 && channel.stereo_locked();
        self.pcm
            .extend(self.scratch.iter().map(|s| s.clamp(-1.0, 1.0)));
        self.produced += (self.scratch.len() / channels) as u64;

        let frame_floats = FRAME_SAMPLES * channels;
        let mut packet = [0u8; MAX_PACKET];
        let mut used = 0;
        while self.pcm.len() - used >= frame_floats {
            let chunk = &self.pcm[used..used + frame_floats];
            used += frame_floats;
            let len = match encoder.encode_float(chunk, &mut packet) {
                Ok(len) => len,
                Err(e) => {
                    tracing::warn!(vfo = self.vfo.id, error = %e, "opus encode failed");
                    self.sample_pos += FRAME_SAMPLES as u64;
                    continue;
                }
            };
            let pos = self.sample_pos;
            let capture_unix_ns = capture_ns(&mut self.timeline, pos);
            let mut flags = 0;
            if self.squelch_open {
                flags |= FLAG_SQUELCH_OPEN;
            }
            if self.stereo_locked {
                flags |= FLAG_STEREO;
            }
            let frame = AudioFrame {
                channels: channels as u8,
                flags,
                sample_rate: OUTPUT_RATE,
                vfo_id: self.vfo.id,
                seq: self.seq,
                sample_pos: pos,
                capture_unix_ns,
                power_dbfs: self.power_dbfs,
                payload: packet[..len].to_vec(),
            };
            self.seq = self.seq.wrapping_add(1);
            self.sample_pos += FRAME_SAMPLES as u64;
            sink(self.vfo.id, frame.encode());
        }
        self.pcm.drain(..used);
    }
}

/// Capture time of the audio sample at index `pos`, interpolated from the block timeline.
fn capture_ns(timeline: &mut VecDeque<(u64, u64)>, pos: u64) -> u64 {
    while timeline.len() > 1 && timeline[1].0 <= pos {
        timeline.pop_front();
    }
    match timeline.front() {
        Some(&(index, ns)) => {
            ns + pos.saturating_sub(index) * 1_000_000_000 / u64::from(OUTPUT_RATE)
        }
        None => now_ns(),
    }
}

/// Make the slots match the registry's VFOs.
fn sync_slots(slots: &mut Vec<Slot>, vfos: &[Vfo]) {
    slots.retain(|s| vfos.iter().any(|v| v.id == s.vfo.id));
    for vfo in vfos {
        match slots.iter_mut().find(|s| s.vfo.id == vfo.id) {
            Some(slot) => slot.vfo = vfo.clone(),
            None => slots.push(Slot::new(vfo.clone())),
        }
    }
}

fn run(
    rx: &Receiver<Arc<IqBlock>>,
    shared: &VfoShared,
    stop: &AtomicBool,
    tap: &IqTap,
    mut sink: impl FnMut(u32, Vec<u8>),
    mut rds_sink: impl FnMut(u32, Vec<u8>),
) {
    let mut slots: Vec<Slot> = Vec::new();
    let mut seen_generation = u64::MAX;
    let mut tuned: Option<(Tuning, u64)> = None;
    let mut settle = 0usize;
    let mut last_report = Instant::now();
    let mut reported_drops = 0;
    while !stop.load(Ordering::Acquire) {
        let block = match rx.recv_timeout(POLL) {
            Ok(block) => block,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let tuning = Tuning {
            center_hz: block.center_hz,
            sample_rate: block.sample_rate,
        };
        let retuned = tuned != Some((tuning, block.tuning_epoch));
        let generation = shared.generation();
        if retuned || generation != seen_generation {
            if generation != seen_generation {
                let (revision, vfos) = shared.snapshot();
                seen_generation = revision;
                sync_slots(&mut slots, &vfos);
            }
            tuned = Some((tuning, block.tuning_epoch));
            if retuned {
                settle = SETTLE_BLOCKS;
            }
            for slot in &mut slots {
                slot.configure(tuning, retuned);
            }
        }
        if settle > 0 {
            settle -= 1;
        } else if !slots.is_empty() {
            let duration_ns =
                block.samples.len() as u64 * 1_000_000_000 / u64::from(block.sample_rate.max(1));
            let block_start_ns = now_ns().saturating_sub(duration_ns);
            for slot in &mut slots {
                slot.process(&block, block_start_ns, &mut sink, &mut rds_sink);
            }
        }
        if last_report.elapsed() >= DROP_REPORT {
            last_report = Instant::now();
            let dropped = tap.dropped();
            if dropped != reported_drops {
                tracing::warn!(total = dropped, "dropped IQ blocks (VFO thread is lagging)");
                reported_drops = dropped;
            }
        }
    }
    tracing::info!("vfo thread exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfo::{VfoCreate, VfoMode};
    use num_complex::Complex32;
    use std::sync::mpsc::channel;
    use superhet_bus::opus::Decoder;
    use superhet_dsp::testsig::{Content, Station};

    const RATE: u32 = 1_024_000;
    const CENTER: u64 = 100_000_000;
    const BLOCK: usize = 4096;

    /// IQ blocks holding a carrier at `offset_hz`, FM-modulated with a 1 kHz tone (3 kHz
    /// deviation).
    fn fm_blocks(offset_hz: f64, count: usize, epoch: u64, center: u64) -> Vec<Arc<IqBlock>> {
        let mut phase = 0.0f64;
        let mut n = 0u64;
        (0..count)
            .map(|_| {
                let samples = (0..BLOCK)
                    .map(|_| {
                        let t = n as f64 / f64::from(RATE);
                        n += 1;
                        let inst =
                            offset_hz + 3_000.0 * (std::f64::consts::TAU * 1_000.0 * t).sin();
                        phase += std::f64::consts::TAU * inst / f64::from(RATE);
                        Complex32::new(0.5 * phase.cos() as f32, 0.5 * phase.sin() as f32)
                    })
                    .collect();
                Arc::new(IqBlock {
                    center_hz: center,
                    sample_rate: RATE,
                    tuning_epoch: epoch,
                    samples,
                })
            })
            .collect()
    }

    fn feed(tap: &IqTap, blocks: &[Arc<IqBlock>]) {
        for block in blocks {
            while !tap.send(block) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn create(shared: &VfoShared, freq_hz: u64, mode: VfoMode) -> u32 {
        shared
            .with(|r| {
                r.create(
                    VfoCreate {
                        freq_hz,
                        mode,
                        bandwidth_hz: None,
                        squelch_dbfs: None,
                        deemphasis: None,
                        cw_pitch_hz: None,
                        stereo: None,
                    },
                    RATE,
                )
            })
            .expect("create")
            .id
    }

    fn collect(rx: &mpsc::Receiver<(u32, Vec<u8>)>, settle: Duration) -> Vec<(u32, AudioFrame)> {
        let mut out = Vec::new();
        while let Ok((id, bytes)) = rx.recv_timeout(settle) {
            out.push((id, AudioFrame::decode(&bytes).expect("decode")));
        }
        out
    }

    /// IQ blocks holding an unmodulated carrier at `offset_hz`.
    fn carrier_blocks(offset_hz: f64, count: usize) -> Vec<Arc<IqBlock>> {
        let mut n = 0u64;
        (0..count)
            .map(|_| {
                let samples = (0..BLOCK)
                    .map(|_| {
                        let ph = std::f64::consts::TAU * offset_hz * n as f64 / f64::from(RATE);
                        n += 1;
                        Complex32::new(0.3 * ph.cos() as f32, 0.3 * ph.sin() as f32)
                    })
                    .collect();
                Arc::new(IqBlock {
                    center_hz: CENTER,
                    sample_rate: RATE,
                    tuning_epoch: 0,
                    samples,
                })
            })
            .collect()
    }

    #[test]
    fn cw_vfo_turns_a_carrier_into_the_pitch_tone() {
        let shared = Arc::new(VfoShared::default());
        let id = create(&shared, CENTER + 50_000, VfoMode::Cw);
        let (tx, rx) = channel();
        let engine = AudioEngine::start(Arc::clone(&shared), move |id, bytes| {
            let _ = tx.send((id, bytes));
        })
        .expect("start");
        feed(&engine.tap(), &carrier_blocks(50_000.0, 300));
        let frames = collect(&rx, Duration::from_millis(300));
        drop(engine);

        assert!(frames.len() >= 50, "got {} frames", frames.len());
        let mut decoder = Decoder::new().expect("decoder");
        let mut pcm_all = Vec::new();
        for (vfo, frame) in &frames {
            assert_eq!(*vfo, id);
            let mut pcm = vec![0f32; FRAME_SAMPLES];
            let n = decoder
                .decode_float(&frame.payload, &mut pcm)
                .expect("opus decode");
            pcm_all.extend_from_slice(&pcm[..n]);
        }
        // Skip the first half (filter / AGC settling): a 700 Hz tone has 1400 zero crossings
        // per second.
        let tail = &pcm_all[pcm_all.len() / 2..];
        let rms = (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(rms > 0.2, "rms {rms}");
        let crossings = tail.windows(2).filter(|w| w[0] * w[1] < 0.0).count() as f32;
        let hz = crossings / 2.0 / (tail.len() as f32 / 48_000.0);
        assert!((650.0..750.0).contains(&hz), "pitch {hz}");
    }

    #[test]
    fn active_vfo_yields_contiguous_decodable_opus_frames() {
        let shared = Arc::new(VfoShared::default());
        let id = create(&shared, CENTER + 50_000, VfoMode::Nfm);
        let (tx, rx) = channel();
        let engine = AudioEngine::start(Arc::clone(&shared), move |id, bytes| {
            let _ = tx.send((id, bytes));
        })
        .expect("start");
        // 300 blocks of 4096 samples at 1.024 MS/s = 1.2 s of signal.
        feed(&engine.tap(), &fm_blocks(50_000.0, 300, 0, CENTER));
        let started = now_ns();
        let frames = collect(&rx, Duration::from_millis(300));
        drop(engine);

        assert!(frames.len() >= 50, "got {} frames", frames.len());
        let mut decoder = Decoder::new().expect("decoder");
        let mut loud = 0;
        for (i, (vfo, frame)) in frames.iter().enumerate() {
            assert_eq!(*vfo, id);
            assert_eq!(frame.vfo_id, id);
            assert_eq!(frame.seq, i as u32);
            assert_eq!(frame.sample_pos, (i * FRAME_SAMPLES) as u64);
            assert_eq!(frame.sample_rate, 48_000);
            assert!(frame.squelch_open());
            assert!(
                frame.power_dbfs > -12.0 && frame.power_dbfs < -4.0,
                "{}",
                frame.power_dbfs
            );
            assert!(
                frame.capture_unix_ns <= started,
                "capture time in the future"
            );
            if i > 0 {
                assert!(frame.capture_unix_ns >= frames[i - 1].1.capture_unix_ns);
            }
            let mut pcm = vec![0f32; FRAME_SAMPLES];
            let n = decoder
                .decode_float(&frame.payload, &mut pcm)
                .expect("opus decode");
            assert_eq!(n, FRAME_SAMPLES);
            if i >= 5 {
                let rms = (pcm.iter().map(|x| x * x).sum::<f32>() / n as f32).sqrt();
                // 3 kHz of 5 kHz NFM reference deviation = 0.6 amplitude → rms ≈ 0.42.
                if rms > 0.25 && rms < 0.6 {
                    loud += 1;
                }
            }
        }
        assert!(
            loud >= frames.len() - 10,
            "{loud} of {} frames carry the tone",
            frames.len()
        );
    }

    /// IQ blocks of a synthetic stereo broadcast station at `offset_hz`.
    fn station_blocks(station: &mut Station, content: Content, count: usize) -> Vec<Arc<IqBlock>> {
        (0..count)
            .map(|_| {
                Arc::new(IqBlock {
                    center_hz: CENTER,
                    sample_rate: RATE,
                    tuning_epoch: 0,
                    samples: (0..BLOCK).map(|_| station.next(content)).collect(),
                })
            })
            .collect()
    }

    fn wfm_vfo(shared: &VfoShared, stereo: Option<bool>) -> u32 {
        shared
            .with(|r| {
                r.create(
                    VfoCreate {
                        freq_hz: CENTER + 100_000,
                        mode: VfoMode::Wfm,
                        bandwidth_hz: None,
                        squelch_dbfs: None,
                        deemphasis: None,
                        cw_pitch_hz: None,
                        stereo,
                    },
                    RATE,
                )
            })
            .expect("create")
            .id
    }

    const BROADCAST: Content = Content {
        left: 0.5,
        right: 0.0,
        pilot: 0.1,
        rds: 0.035,
    };

    #[test]
    fn stereo_wfm_yields_two_channel_frames_that_keep_the_channels_apart() {
        let shared = Arc::new(VfoShared::default());
        let id = wfm_vfo(&shared, None);
        let (tx, rx) = channel();
        let engine = AudioEngine::start(Arc::clone(&shared), move |id, bytes| {
            let _ = tx.send((id, bytes));
        })
        .expect("start");
        let mut station = Station::new(RATE, 100_000.0, "SUPERHET", "Hello");
        // 1.6 s of signal.
        feed(&engine.tap(), &station_blocks(&mut station, BROADCAST, 400));
        let frames = collect(&rx, Duration::from_millis(300));
        drop(engine);

        assert!(frames.len() >= 50, "got {} frames", frames.len());
        let mut decoder = Decoder::with_channels(2).expect("decoder");
        let mut separated = 0;
        for (i, (vfo, frame)) in frames.iter().enumerate() {
            assert_eq!(*vfo, id);
            assert_eq!(frame.channels, 2);
            assert_eq!(frame.sample_pos, (i * FRAME_SAMPLES) as u64);
            let mut pcm = vec![0f32; 2 * FRAME_SAMPLES];
            let n = decoder
                .decode_float(&frame.payload, &mut pcm)
                .expect("opus decode");
            assert_eq!(n, FRAME_SAMPLES);
            if i < frames.len() - 20 {
                continue;
            }
            assert!(frame.stereo(), "pilot not flagged in frame {i}");
            let power = |skip: usize| {
                pcm.iter().skip(skip).step_by(2).map(|x| x * x).sum::<f32>() / n as f32
            };
            let (left, right) = (power(0), power(1));
            if left > 0.01 && right < left / 100.0 {
                separated += 1;
            }
        }
        assert!(
            separated >= 15,
            "{separated} of the last 20 frames are stereo"
        );
    }

    #[test]
    fn stereo_off_gives_mono_frames_without_the_stereo_flag() {
        let shared = Arc::new(VfoShared::default());
        wfm_vfo(&shared, Some(false));
        let (tx, rx) = channel();
        let engine = AudioEngine::start(Arc::clone(&shared), move |id, bytes| {
            let _ = tx.send((id, bytes));
        })
        .expect("start");
        let mut station = Station::new(RATE, 100_000.0, "SUPERHET", "Hello");
        feed(&engine.tap(), &station_blocks(&mut station, BROADCAST, 300));
        let frames = collect(&rx, Duration::from_millis(300));
        assert!(frames.len() >= 40, "got {} frames", frames.len());
        assert!(frames.iter().all(|(_, f)| f.channels == 1 && !f.stereo()));
        let mut decoder = Decoder::new().expect("decoder");
        let mut pcm = vec![0f32; FRAME_SAMPLES];
        let last = &frames.last().expect("frames").1;
        assert_eq!(
            decoder.decode_float(&last.payload, &mut pcm).expect("opus"),
            FRAME_SAMPLES
        );

        // Switching stereo on at runtime changes the stream to two channels.
        shared
            .with(|r| {
                r.update(
                    &crate::vfo::VfoPatch {
                        id: 1,
                        freq_hz: None,
                        mode: None,
                        bandwidth_hz: None,
                        squelch_dbfs: None,
                        deemphasis: None,
                        cw_pitch_hz: None,
                        stereo: Some(Some(true)),
                    },
                    RATE,
                )
            })
            .expect("update");
        feed(&engine.tap(), &station_blocks(&mut station, BROADCAST, 300));
        let frames = collect(&rx, Duration::from_millis(300));
        assert!(
            frames.iter().any(|(_, f)| f.channels == 2),
            "no stereo frames"
        );
        assert!(frames.last().is_some_and(|(_, f)| f.channels == 2));
    }

    #[test]
    fn rds_state_is_published_and_cleared_on_retune() {
        let shared = Arc::new(VfoShared::default());
        let id = wfm_vfo(&shared, None);
        let (tx, _rx) = channel();
        let (rds_tx, rds_rx) = channel();
        let engine = AudioEngine::start_with_rds(
            Arc::clone(&shared),
            move |id, bytes| {
                let _ = tx.send((id, bytes));
            },
            move |id, bytes| {
                let _ = rds_tx.send((id, bytes));
            },
        )
        .expect("start");
        let tap = engine.tap();
        let mut station = Station::new(RATE, 100_000.0, "SUPERHET", "Hello RDS world");
        // 4.5 s of signal.
        feed(&tap, &station_blocks(&mut station, BROADCAST, 1100));
        let mut last = None;
        while let Ok((vfo, bytes)) = rds_rx.recv_timeout(Duration::from_millis(300)) {
            assert_eq!(vfo, id);
            last = Some(RdsMessage::parse(&bytes).expect("valid rds message"));
        }
        let message = last.expect("rds messages");
        assert_eq!(message.vfo, id);
        assert!(message.synced);
        assert_eq!(message.pi.as_deref(), Some("D3C2"));
        assert_eq!(message.pty, Some(10));
        assert_eq!(message.ps.as_deref(), Some("SUPERHET"));
        assert_eq!(message.rt.as_deref(), Some("Hello RDS world"));

        // A retune of the device clears the display.
        let retuned: Vec<_> = (0..20)
            .map(|_| {
                Arc::new(IqBlock {
                    center_hz: CENTER + 300_000,
                    sample_rate: RATE,
                    tuning_epoch: 1,
                    samples: vec![Complex32::new(0.0, 0.0); BLOCK],
                })
            })
            .collect();
        feed(&tap, &retuned);
        let mut after = None;
        while let Ok((_, bytes)) = rds_rx.recv_timeout(Duration::from_millis(300)) {
            after = Some(RdsMessage::parse(&bytes).expect("valid rds message"));
        }
        assert!(after.is_some_and(|m| m.is_empty()), "state not cleared");
    }

    #[test]
    fn paused_vfo_makes_no_frames_and_retune_pauses_and_resumes() {
        let shared = Arc::new(VfoShared::default());
        // 2 MHz away from the centre: outside the 1.024 MHz passband.
        let id = create(&shared, CENTER + 2_000_000, VfoMode::Nfm);
        let (tx, rx) = channel();
        let engine = AudioEngine::start(Arc::clone(&shared), move |id, bytes| {
            let _ = tx.send((id, bytes));
        })
        .expect("start");
        let tap = engine.tap();
        feed(&tap, &fm_blocks(0.0, 60, 0, CENTER));
        assert!(collect(&rx, Duration::from_millis(200)).is_empty());

        // Retune so the VFO is inside the passband (carrier now at +50 kHz).
        feed(&tap, &fm_blocks(50_000.0, 120, 1, CENTER + 1_950_000));
        let frames = collect(&rx, Duration::from_millis(200));
        assert!(frames.len() >= 20, "got {} frames", frames.len());
        assert!(frames.iter().all(|(v, _)| *v == id));
        assert_eq!(frames[0].1.seq, 0);

        // Retune away again: no more frames, counters resume afterwards.
        feed(&tap, &fm_blocks(0.0, 60, 2, CENTER));
        let _ = collect(&rx, Duration::from_millis(200));
        feed(&tap, &fm_blocks(0.0, 60, 3, CENTER));
        assert!(collect(&rx, Duration::from_millis(200)).is_empty());
        feed(&tap, &fm_blocks(50_000.0, 120, 4, CENTER + 1_950_000));
        let again = collect(&rx, Duration::from_millis(200));
        assert!(!again.is_empty());
        assert!(again[0].1.seq > frames.last().expect("frames").1.seq);
        assert!(again[0].1.sample_pos > frames.last().expect("frames").1.sample_pos);
    }

    #[test]
    fn deleted_vfo_stops_and_closed_squelch_is_flagged() {
        let shared = Arc::new(VfoShared::default());
        let id = shared
            .with(|r| {
                r.create(
                    VfoCreate {
                        freq_hz: CENTER + 50_000,
                        mode: VfoMode::Nfm,
                        bandwidth_hz: None,
                        // Signal is about -6 dBFS; this never opens.
                        squelch_dbfs: Some(-3.0),
                        deemphasis: None,
                        cw_pitch_hz: None,
                        stereo: None,
                    },
                    RATE,
                )
            })
            .expect("create")
            .id;
        let (tx, rx) = channel();
        let engine = AudioEngine::start(Arc::clone(&shared), move |id, bytes| {
            let _ = tx.send((id, bytes));
        })
        .expect("start");
        let tap = engine.tap();
        feed(&tap, &fm_blocks(50_000.0, 100, 0, CENTER));
        let frames = collect(&rx, Duration::from_millis(200));
        assert!(frames.len() >= 10);
        assert!(frames.iter().all(|(_, f)| !f.squelch_open()));

        shared.with(|r| r.delete(id)).expect("delete");
        // One block is enough for the thread to notice; frames of that block may still flush.
        feed(&tap, &fm_blocks(50_000.0, 2, 0, CENTER));
        let _ = collect(&rx, Duration::from_millis(200));
        feed(&tap, &fm_blocks(50_000.0, 100, 0, CENTER));
        assert!(collect(&rx, Duration::from_millis(200)).is_empty());
    }
}
