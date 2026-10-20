//! Device → FFT → quantised spectrum frames, on a dedicated OS thread.
//!
//! The IQ path never blocks: the device thread drops blocks when this thread lags, and the
//! frame sink is a plain callback that must itself never block (see `publish`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use superhet_bus::spectrum::SpectrumFrame;
use superhet_device::{Device, IqStream};
use superhet_dsp::{SpectrumConfig, SpectrumError, SpectrumProcessor};

use crate::config::SpectrumSettings;

/// Blocks buffered between device thread and DSP thread.
const QUEUE_BLOCKS: usize = 16;
/// Interval for reporting dropped IQ blocks.
const DROP_REPORT: Duration = Duration::from_secs(5);

/// Pipeline start-up failure.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// Device error.
    #[error(transparent)]
    Device(#[from] superhet_device::Error),
    /// Invalid spectrum settings.
    #[error("invalid spectrum settings: {0}")]
    Spectrum(#[from] SpectrumError),
    /// Thread creation failed.
    #[error("cannot spawn dsp thread: {0}")]
    Spawn(#[from] std::io::Error),
}

/// A running pipeline; dropping it stops the device and joins the DSP thread.
pub struct Pipeline {
    device: Box<dyn Device>,
    thread: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

/// Clears the running flag when the DSP thread ends, even by panic.
struct RunningGuard(Arc<AtomicBool>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Pipeline {
    /// Flag that is true while the device stream is being processed.
    #[must_use]
    pub fn running_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.running)
    }

    /// Start streaming from `device` and call `sink` with each encoded [`SpectrumFrame`].
    ///
    /// # Errors
    /// Fails if the settings are invalid or the device cannot start.
    pub fn start(
        mut device: Box<dyn Device>,
        settings: &SpectrumSettings,
        mut sink: impl FnMut(Vec<u8>) + Send + 'static,
    ) -> Result<Self, PipelineError> {
        let mut processor = SpectrumProcessor::new(SpectrumConfig {
            fft_size: settings.fft_size,
            bins: settings.bins,
            fps: settings.fps,
            sample_rate: device.sample_rate(),
        })?;
        let (min_db, max_db) = (settings.min_db, settings.max_db);
        let stream: IqStream = device.start(QUEUE_BLOCKS)?;
        let running = Arc::new(AtomicBool::new(true));
        let guard = RunningGuard(Arc::clone(&running));
        let thread = std::thread::Builder::new()
            .name("sdrd-dsp".into())
            .spawn(move || {
                let _guard = guard;
                let mut tuning = (0u64, 0u32);
                let mut last_report = std::time::Instant::now();
                let mut reported_drops = 0;
                while let Ok(block) = stream.blocks.recv() {
                    if tuning != (block.center_hz, block.sample_rate) {
                        tuning = (block.center_hz, block.sample_rate);
                        if processor.set_sample_rate(block.sample_rate).is_err() {
                            continue;
                        }
                    }
                    for frame in processor.push(&block.samples) {
                        let quantised = SpectrumFrame::quantize(
                            block.center_hz,
                            block.sample_rate,
                            now_us(),
                            min_db,
                            max_db,
                            &frame,
                        );
                        sink(quantised.encode());
                    }
                    if last_report.elapsed() >= DROP_REPORT {
                        last_report = std::time::Instant::now();
                        let dropped = stream.dropped_blocks();
                        if dropped != reported_drops {
                            tracing::warn!(
                                total = dropped,
                                "dropped IQ blocks (DSP thread is lagging)"
                            );
                            reported_drops = dropped;
                        }
                    }
                }
                tracing::info!("iq stream closed; dsp thread exiting");
            })?;
        Ok(Self {
            device,
            thread: Some(thread),
            running,
        })
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.device.stop();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("dsp thread panicked");
        }
    }
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc::channel;

    /// Write a cu8 file with a complex tone at `offset_hz`.
    fn write_tone(offset_hz: f64, rate: u32, samples: usize) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("superhet-sdrd-tone-{}.cu8", std::process::id()));
        let mut bytes = Vec::with_capacity(samples * 2);
        for i in 0..samples {
            let ph = std::f64::consts::TAU * offset_hz * i as f64 / f64::from(rate);
            bytes.push((127.5 + 127.0 * ph.cos()).round() as u8);
            bytes.push((127.5 + 127.0 * ph.sin()).round() as u8);
        }
        std::fs::File::create(&path)
            .and_then(|mut f| f.write_all(&bytes))
            .expect("write fixture");
        path
    }

    #[test]
    fn file_device_produces_spectrum_frames_with_tone_in_expected_bin() {
        let rate = 1_024_000;
        // 50 kHz offset, 1024 bins over 1.024 MHz = 1 kHz per bin → bin 512 + 50.
        let path = write_tone(50_000.0, rate, 8 * 4096);
        let spec = format!(
            "file:{}?format=cu8&rate={rate}&freq=433920000&block=4096",
            path.display()
        );
        let device = superhet_device::open_from_spec(&spec).expect("device");
        let settings = SpectrumSettings {
            fft_size: 4096,
            bins: 1024,
            fps: 50.0,
            min_db: -120.0,
            max_db: 0.0,
        };
        let (tx, rx) = channel();
        let pipeline = Pipeline::start(device, &settings, move |frame| {
            let _ = tx.send(frame);
        })
        .expect("start");
        let bytes = rx.recv_timeout(Duration::from_secs(5)).expect("frame");
        drop(pipeline);
        let _ = std::fs::remove_file(path);

        let frame = SpectrumFrame::decode(&bytes).expect("decode");
        assert_eq!(frame.center_hz, 433_920_000);
        assert_eq!(frame.sample_rate, rate);
        assert_eq!(frame.bins.len(), 1024);
        let peak = frame
            .bins
            .iter()
            .enumerate()
            .max_by_key(|(_, q)| **q)
            .map(|(i, _)| i)
            .expect("bins");
        assert_eq!(peak, 512 + 50);
        assert!(frame.db(peak).expect("db") > -3.0);
    }
}
