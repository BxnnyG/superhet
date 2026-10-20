//! Device abstraction layer for Superhet (RTL-SDR first; Soapy and rtl_tcp later).
//!
//! A [`Device`] owns one SDR. Streaming is pull-free: [`Device::start`] returns an
//! [`IqStream`] whose bounded channel is fed from a dedicated OS thread. The producer
//! never blocks on a slow consumer; blocks that do not fit are dropped and counted.
//! A future librtlsdr backend implements the same trait.

mod file;
mod spec;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;

pub use file::{FileConfig, FileDevice, SampleFormat};
pub use num_complex::Complex32;
pub use spec::open_from_spec;

/// Errors raised by device backends.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// I/O failure while opening or reading a device.
    #[error("device i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The device specification or a parameter is invalid.
    #[error("invalid device configuration: {0}")]
    Config(String),
    /// The operation is not valid in the current state (e.g. already streaming).
    #[error("invalid device state: {0}")]
    State(&'static str),
}

/// A block of complex baseband samples with the tuning it was captured at.
#[derive(Debug, Clone)]
pub struct IqBlock {
    /// Centre frequency in Hz at capture time.
    pub center_hz: u64,
    /// Sample rate in Hz at capture time.
    pub sample_rate: u32,
    /// Samples, normalised to roughly [-1.0, 1.0] (full scale = 1.0).
    pub samples: Vec<Complex32>,
}

/// Receiving end of a running device stream.
#[derive(Debug)]
pub struct IqStream {
    /// Blocks in capture order. The channel closes when the device stops.
    pub blocks: Receiver<IqBlock>,
    dropped: Arc<AtomicU64>,
}

impl IqStream {
    /// Create a stream from its parts (for backend implementations).
    pub fn new(blocks: Receiver<IqBlock>, dropped: Arc<AtomicU64>) -> Self {
        Self { blocks, dropped }
    }

    /// Number of blocks the producer had to drop because the consumer lagged.
    pub fn dropped_blocks(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// An SDR device (or a replacement such as a file replay).
pub trait Device: Send {
    /// Human-readable backend name, e.g. `file`.
    fn name(&self) -> &str;
    /// Current centre frequency in Hz.
    fn center_freq(&self) -> u64;
    /// Current sample rate in Hz.
    fn sample_rate(&self) -> u32;
    /// Retune; may be called while streaming.
    ///
    /// # Errors
    /// Fails if the device rejects the frequency.
    fn set_center_freq(&mut self, hz: u64) -> Result<(), Error>;
    /// Change the sample rate; may be called while streaming.
    ///
    /// # Errors
    /// Fails if the device rejects the rate.
    fn set_sample_rate(&mut self, hz: u32) -> Result<(), Error>;
    /// Start streaming. `queue_blocks` bounds the channel between device and consumer.
    ///
    /// # Errors
    /// Fails if the device is already streaming or cannot be started.
    fn start(&mut self, queue_blocks: usize) -> Result<IqStream, Error>;
    /// Stop streaming and release the producer thread. Idempotent.
    fn stop(&mut self);
}
