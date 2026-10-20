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

use superhet_bus::audio::{AudioFrame, FLAG_SQUELCH_OPEN, FRAME_SAMPLES};
use superhet_bus::opus::Encoder;
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
/// Opus bitrate for WFM in bits per second.
const BITRATE_WFM: i32 = 64_000;
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
    /// Start the thread. `sink` receives `(vfo_id, encoded AudioFrame)`; it runs on the engine
    /// thread and must never block.
    ///
    /// # Errors
    /// Fails if the thread cannot be spawned.
    pub fn start(
        vfos: Arc<VfoShared>,
        sink: impl FnMut(u32, Vec<u8>) + Send + 'static,
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
                .spawn(move || run(&rx, &vfos, &stop, &tap, sink))?
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

fn bitrate_for(mode: Mode) -> i32 {
    match mode {
        Mode::Wfm { .. } => BITRATE_WFM,
        Mode::Nfm { .. } | Mode::Am => BITRATE_NARROW,
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
    /// Audio not yet forming a full frame.
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
        }
    }

    /// Drop buffered audio and its timing; the counters keep running.
    fn discard_audio(&mut self) {
        self.pcm.clear();
        self.produced = self.sample_pos;
        self.timeline.clear();
    }

    fn deactivate(&mut self) {
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
        self.ensure_encoder(bitrate_for(cfg.mode));
    }

    fn ensure_encoder(&mut self, bitrate: i32) {
        match &mut self.encoder {
            Some(encoder) => {
                if bitrate != self.bitrate && encoder.set_bitrate(bitrate).is_ok() {
                    self.bitrate = bitrate;
                }
            }
            None => match Encoder::new(bitrate) {
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

    /// Demodulate one block and emit every completed frame.
    fn process(
        &mut self,
        block: &IqBlock,
        block_start_ns: u64,
        sink: &mut impl FnMut(u32, Vec<u8>),
    ) {
        let (Some(channel), Some(encoder)) = (self.channel.as_mut(), self.encoder.as_mut()) else {
            return;
        };
        self.timeline.push_back((self.produced, block_start_ns));
        self.scratch.clear();
        let stats = channel.process(&block.samples, &mut self.scratch);
        self.power_dbfs = stats.power_dbfs;
        self.squelch_open = stats.squelch_open;
        self.pcm
            .extend(self.scratch.iter().map(|s| s.clamp(-1.0, 1.0)));
        self.produced += self.scratch.len() as u64;

        let mut packet = [0u8; MAX_PACKET];
        let mut used = 0;
        while self.pcm.len() - used >= FRAME_SAMPLES {
            let chunk = &self.pcm[used..used + FRAME_SAMPLES];
            used += FRAME_SAMPLES;
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
            let frame = AudioFrame {
                channels: 1,
                flags: if self.squelch_open {
                    FLAG_SQUELCH_OPEN
                } else {
                    0
                },
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
                slot.process(&block, block_start_ns, &mut sink);
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
