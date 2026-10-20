//! File replay backend: raw `cu8` (RTL-SDR interleaved unsigned 8-bit) or `cf32`
//! (interleaved little-endian f32) IQ, looped endlessly and paced to real time.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use num_complex::Complex32;

use crate::{Device, Error, IqBlock, IqStream};

/// Default samples per emitted block.
const DEFAULT_BLOCK_LEN: usize = 16_384;
const DEFAULT_RATE: u32 = 2_048_000;

/// On-disk sample format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// Interleaved unsigned 8-bit I/Q (RTL-SDR native, 127.5 = zero).
    Cu8,
    /// Interleaved little-endian 32-bit float I/Q.
    Cf32,
}

impl SampleFormat {
    fn bytes_per_sample(self) -> usize {
        match self {
            Self::Cu8 => 2,
            Self::Cf32 => 8,
        }
    }
}

/// Configuration of a [`FileDevice`].
#[derive(Debug, Clone)]
pub struct FileConfig {
    /// Path of the IQ file.
    pub path: PathBuf,
    /// Sample format of the file.
    pub format: SampleFormat,
    /// Sample rate in Hz (also the replay pacing).
    pub sample_rate: u32,
    /// Reported centre frequency in Hz.
    pub center_hz: u64,
    /// Samples per emitted block.
    pub block_len: usize,
}

impl FileConfig {
    /// Parse `<path>[?format=cu8|cf32&rate=<Hz>&freq=<Hz>&block=<samples>]`.
    /// Without `format`, it is derived from the extension (`.cf32`/`.fc32` → cf32, else cu8).
    ///
    /// # Errors
    /// Fails on unknown keys or unparsable values.
    pub fn parse(rest: &str) -> Result<Self, Error> {
        let (path, query) = match rest.rsplit_once('?') {
            Some((p, q)) => (p, q),
            None => (rest, ""),
        };
        if path.is_empty() {
            return Err(Error::Config("file device needs a path".into()));
        }
        let mut cfg = Self {
            path: PathBuf::from(path),
            format: match path.rsplit_once('.').map(|(_, e)| e) {
                Some("cf32" | "fc32") => SampleFormat::Cf32,
                _ => SampleFormat::Cu8,
            },
            sample_rate: DEFAULT_RATE,
            center_hz: 0,
            block_len: DEFAULT_BLOCK_LEN,
        };
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| Error::Config(format!("malformed parameter {pair:?}")))?;
            let bad = |_| Error::Config(format!("invalid value for {key}: {value:?}"));
            match key {
                "format" => {
                    cfg.format = match value {
                        "cu8" => SampleFormat::Cu8,
                        "cf32" => SampleFormat::Cf32,
                        _ => return Err(Error::Config(format!("unknown format {value:?}"))),
                    }
                }
                "rate" => cfg.sample_rate = value.parse().map_err(bad)?,
                "freq" => cfg.center_hz = value.parse().map_err(bad)?,
                "block" => cfg.block_len = value.parse().map_err(bad)?,
                _ => return Err(Error::Config(format!("unknown parameter {key:?}"))),
            }
        }
        if cfg.sample_rate == 0 || cfg.block_len == 0 {
            return Err(Error::Config("rate and block must be non-zero".into()));
        }
        Ok(cfg)
    }
}

/// Tuning shared between the control side and the producer thread.
#[derive(Debug)]
struct Shared {
    center_hz: AtomicU64,
    sample_rate: AtomicU32,
    stop: AtomicBool,
}

/// File replay device.
#[derive(Debug)]
pub struct FileDevice {
    config: FileConfig,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl FileDevice {
    /// Open the file and validate that it holds at least one sample.
    ///
    /// # Errors
    /// Fails if the file is unreadable or shorter than one sample.
    pub fn open(config: FileConfig) -> Result<Self, Error> {
        let len = std::fs::metadata(&config.path)?.len();
        if len < config.format.bytes_per_sample() as u64 {
            return Err(Error::Config(format!(
                "{} holds no complete sample",
                config.path.display()
            )));
        }
        let shared = Arc::new(Shared {
            center_hz: AtomicU64::new(config.center_hz),
            sample_rate: AtomicU32::new(config.sample_rate),
            stop: AtomicBool::new(false),
        });
        Ok(Self {
            config,
            shared,
            worker: None,
        })
    }
}

impl Device for FileDevice {
    fn name(&self) -> &str {
        "file"
    }

    fn center_freq(&self) -> u64 {
        self.shared.center_hz.load(Ordering::Relaxed)
    }

    fn sample_rate(&self) -> u32 {
        self.shared.sample_rate.load(Ordering::Relaxed)
    }

    fn set_center_freq(&mut self, hz: u64) -> Result<(), Error> {
        self.shared.center_hz.store(hz, Ordering::Relaxed);
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<(), Error> {
        if hz == 0 {
            return Err(Error::Config("sample rate must be non-zero".into()));
        }
        self.shared.sample_rate.store(hz, Ordering::Relaxed);
        Ok(())
    }

    fn start(&mut self, queue_blocks: usize) -> Result<IqStream, Error> {
        if self.worker.is_some() {
            return Err(Error::State("already streaming"));
        }
        let file = File::open(&self.config.path)?;
        self.shared.stop.store(false, Ordering::Relaxed);
        let (tx, rx) = sync_channel(queue_blocks.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        let producer = Producer {
            reader: BufReader::new(file),
            format: self.config.format,
            block_len: self.config.block_len,
            shared: Arc::clone(&self.shared),
            dropped: Arc::clone(&dropped),
        };
        let handle = std::thread::Builder::new()
            .name("file-device".into())
            .spawn(move || producer.run(&tx))?;
        self.worker = Some(handle);
        Ok(IqStream::new(rx, dropped))
    }

    fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.worker.take()
            && handle.join().is_err()
        {
            tracing::warn!("file device thread panicked");
        }
    }
}

impl Drop for FileDevice {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Producer {
    reader: BufReader<File>,
    format: SampleFormat,
    block_len: usize,
    shared: Arc<Shared>,
    dropped: Arc<AtomicU64>,
}

impl Producer {
    fn run(mut self, tx: &std::sync::mpsc::SyncSender<IqBlock>) {
        let mut raw = vec![0u8; self.block_len * self.format.bytes_per_sample()];
        let mut anchor = Instant::now();
        let mut emitted: u64 = 0;
        let mut rate = self.shared.sample_rate.load(Ordering::Relaxed);
        while !self.shared.stop.load(Ordering::Relaxed) {
            let new_rate = self.shared.sample_rate.load(Ordering::Relaxed);
            if new_rate != rate {
                rate = new_rate;
                anchor = Instant::now();
                emitted = 0;
            }
            let samples = match self.read_block(&mut raw) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "file device read failed; stopping");
                    return;
                }
            };
            emitted += samples.len() as u64;
            let block = IqBlock {
                center_hz: self.shared.center_hz.load(Ordering::Relaxed),
                sample_rate: rate,
                samples,
            };
            match tx.try_send(block) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
            // Pace to real time: the block ends at emitted / rate seconds after the anchor.
            let due = anchor + Duration::from_secs_f64(emitted as f64 / f64::from(rate));
            self.sleep_until(due);
        }
    }

    /// Sleep in short slices so `stop()` stays responsive.
    fn sleep_until(&self, due: Instant) {
        loop {
            if self.shared.stop.load(Ordering::Relaxed) {
                return;
            }
            let now = Instant::now();
            if now >= due {
                return;
            }
            std::thread::sleep((due - now).min(Duration::from_millis(20)));
        }
    }

    /// Read exactly one block of samples, wrapping around at end of file.
    fn read_block(&mut self, raw: &mut [u8]) -> std::io::Result<Vec<Complex32>> {
        let bps = self.format.bytes_per_sample();
        let mut filled = 0;
        let mut rewound_empty = false;
        while filled < raw.len() {
            let n = self.reader.read(&mut raw[filled..])?;
            if n == 0 {
                // End of file: discard a trailing partial sample and loop.
                filled -= filled % bps;
                if filled == 0 && rewound_empty {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "no samples after rewind",
                    ));
                }
                rewound_empty = filled == 0;
                self.reader.seek(SeekFrom::Start(0))?;
                // Keep what we already have; continue filling after the rewind.
                continue;
            }
            rewound_empty = false;
            filled += n;
        }
        Ok(decode(self.format, raw))
    }
}

fn decode(format: SampleFormat, raw: &[u8]) -> Vec<Complex32> {
    match format {
        SampleFormat::Cu8 => raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                Complex32::new(
                    (f32::from(c[0]) - 127.5) / 127.5,
                    (f32::from(c[1]) - 127.5) / 127.5,
                )
            })
            .collect(),
        SampleFormat::Cf32 => raw
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| {
                Complex32::new(
                    f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                    f32::from_le_bytes([c[4], c[5], c[6], c[7]]),
                )
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::AtomicU32;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "superhet-device-test-{}-{}-{name}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        File::create(&path)
            .and_then(|mut f| f.write_all(bytes))
            .expect("write fixture");
        path
    }

    #[test]
    fn parses_spec() {
        let c =
            FileConfig::parse("/tmp/a.cf32?rate=1000000&freq=433920000&block=512").expect("valid");
        assert_eq!(c.format, SampleFormat::Cf32);
        assert_eq!(c.sample_rate, 1_000_000);
        assert_eq!(c.center_hz, 433_920_000);
        assert_eq!(c.block_len, 512);
        let c = FileConfig::parse("/tmp/a.bin?format=cu8").expect("valid");
        assert_eq!(c.format, SampleFormat::Cu8);
        assert!(FileConfig::parse("/tmp/a?bogus=1").is_err());
        assert!(FileConfig::parse("/tmp/a?rate=x").is_err());
        assert!(FileConfig::parse("?rate=1").is_err());
    }

    #[test]
    fn decodes_cu8_and_loops() {
        // 3 samples; block of 5 forces a wrap. Trailing odd byte must be ignored.
        let path = temp_file("loop.cu8", &[255, 0, 0, 255, 128, 127, 99]);
        let cfg = FileConfig::parse(&format!(
            "{}?format=cu8&rate=10000000&block=5&freq=7",
            path.display()
        ))
        .expect("valid");
        let mut dev = FileDevice::open(cfg).expect("open");
        let stream = dev.start(4).expect("start");
        let block = stream
            .blocks
            .recv_timeout(Duration::from_secs(2))
            .expect("block");
        assert_eq!(block.center_hz, 7);
        assert_eq!(block.sample_rate, 10_000_000);
        assert_eq!(block.samples.len(), 5);
        assert!((block.samples[0].re - 1.0).abs() < 1e-6);
        assert!((block.samples[0].im + 1.0).abs() < 1e-6);
        assert!((block.samples[1].re + 1.0).abs() < 1e-6);
        // Sample 3 is the wrap back to sample 0.
        assert_eq!(block.samples[3], block.samples[0]);
        dev.stop();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn decodes_cf32() {
        let mut bytes = Vec::new();
        for v in [0.5f32, -0.25, 1.0, 0.0] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let path = temp_file("a.cf32", &bytes);
        let cfg =
            FileConfig::parse(&format!("{}?rate=10000000&block=2", path.display())).expect("valid");
        let mut dev = FileDevice::open(cfg).expect("open");
        let stream = dev.start(2).expect("start");
        let block = stream
            .blocks
            .recv_timeout(Duration::from_secs(2))
            .expect("block");
        assert_eq!(
            block.samples,
            vec![Complex32::new(0.5, -0.25), Complex32::new(1.0, 0.0)]
        );
        dev.stop();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn paces_to_real_time_and_drops_when_consumer_lags() {
        let path = temp_file("pace.cu8", &vec![128u8; 4096]);
        // 20 ms blocks at 100 kS/s.
        let cfg = FileConfig::parse(&format!("{}?rate=100000&block=2000", path.display()))
            .expect("valid");
        let mut dev = FileDevice::open(cfg).expect("open");
        let stream = dev.start(1).expect("start");
        std::thread::sleep(Duration::from_millis(300));
        // Consumer did not read: producer must have dropped blocks without blocking.
        assert!(stream.dropped_blocks() >= 5, "{}", stream.dropped_blocks());
        assert!(stream.dropped_blocks() <= 20, "{}", stream.dropped_blocks());
        assert!(dev.start(1).is_err());
        dev.stop();
        dev.stop();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_empty_file() {
        let path = temp_file("empty.cu8", &[]);
        let cfg = FileConfig::parse(&path.display().to_string()).expect("valid");
        assert!(FileDevice::open(cfg).is_err());
        let _ = std::fs::remove_file(path);
    }
}
