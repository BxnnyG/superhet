//! Device abstraction layer for Superhet (RTL-SDR first; Soapy and rtl_tcp later).
//!
//! A [`Device`] owns one SDR. Streaming is pull-free: [`Device::start`] returns an
//! [`IqStream`] whose bounded channel is fed from a dedicated OS thread. The producer
//! never blocks on a slow consumer; blocks that do not fit are dropped and counted.
//! Backends: [`FileDevice`] (replay) and, with the `rtlsdr` feature, `RtlSdrDevice`.

mod file;
#[cfg(feature = "rtlsdr")]
mod rtlsdr;
mod rtlsdr_config;
mod spec;
mod tuning;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;

pub use file::{FileConfig, FileDevice, SampleFormat};
pub use num_complex::Complex32;
#[cfg(feature = "rtlsdr")]
pub use rtlsdr::RtlSdrDevice;
pub use rtlsdr_config::{DeviceSelector, Gain, RtlSdrConfig};
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
    /// The backend does not support the requested setting.
    #[error("unsupported by this device: {0}")]
    Unsupported(&'static str),
}

/// A block of complex baseband samples with the tuning it was captured at.
#[derive(Debug, Clone)]
pub struct IqBlock {
    /// Centre frequency in Hz at capture time.
    pub center_hz: u64,
    /// Sample rate in Hz at capture time.
    pub sample_rate: u32,
    /// Tuning generation at capture time; incremented by every applied retune (centre, rate,
    /// gain or ppm change). Consumers reset averaging state when it changes.
    pub tuning_epoch: u64,
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

/// A runtime change of the device settings. `None` leaves a setting untouched.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TuneRequest {
    /// New centre frequency in Hz.
    pub center_hz: Option<u64>,
    /// New sample rate in Hz.
    pub sample_rate: Option<u32>,
    /// New tuner gain.
    pub gain: Option<Gain>,
    /// New crystal correction in ppm.
    pub ppm: Option<i32>,
    /// Bias tee on or off.
    pub bias_tee: Option<bool>,
}

impl TuneRequest {
    /// Whether the request changes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Fold a later request into this one: for every field the later value wins.
    pub fn merge(&mut self, later: &Self) {
        self.center_hz = later.center_hz.or(self.center_hz);
        self.sample_rate = later.sample_rate.or(self.sample_rate);
        self.gain = later.gain.or(self.gain);
        self.ppm = later.ppm.or(self.ppm);
        self.bias_tee = later.bias_tee.or(self.bias_tee);
    }
}

/// Snapshot of the settings a device is actually running with.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceState {
    /// Backend name, e.g. `rtlsdr` or `file`.
    pub driver: &'static str,
    /// Tuner chip, if known.
    pub tuner: Option<String>,
    /// Centre frequency in Hz.
    pub center_hz: u64,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Gain as applied (a manual value is the one the tuner snapped to); `None` if the device
    /// has no gain control.
    pub gain: Option<Gain>,
    /// Crystal correction in ppm.
    pub ppm: i32,
    /// Bias tee state.
    pub bias_tee: bool,
    /// Tuning generation, see [`IqBlock::tuning_epoch`].
    pub tuning_epoch: u64,
    /// Selectable manual gains in dB (empty if not applicable).
    pub gains_db: Vec<f32>,
}

/// Thread-safe control handle of a device; stays valid while the device streams.
///
/// Calls are serialised internally. Hardware settings are changed while the read thread keeps
/// running; blocks affected by the change are marked through [`IqBlock::tuning_epoch`].
pub trait DeviceControl: Send + Sync {
    /// Current settings.
    fn state(&self) -> DeviceState;
    /// Apply the requested changes after validating all of them first, and return the
    /// resulting settings. A request that only repeats current values changes nothing.
    ///
    /// # Errors
    /// [`Error::Config`] for out-of-range values, [`Error::Unsupported`] for settings the
    /// backend lacks, otherwise a device error. On a device error, changes made before the
    /// failure stay applied (see [`DeviceControl::state`]).
    fn apply(&self, request: &TuneRequest) -> Result<DeviceState, Error>;
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
    /// Set the tuner gain.
    ///
    /// # Errors
    /// [`Error::Unsupported`] unless the backend has a gain control.
    fn set_gain(&mut self, gain: Gain) -> Result<(), Error> {
        let _ = gain;
        Err(Error::Unsupported("gain"))
    }
    /// Set the crystal correction in ppm.
    ///
    /// # Errors
    /// [`Error::Unsupported`] unless the backend supports it.
    fn set_ppm(&mut self, ppm: i32) -> Result<(), Error> {
        let _ = ppm;
        Err(Error::Unsupported("ppm correction"))
    }
    /// Switch the bias tee.
    ///
    /// # Errors
    /// [`Error::Unsupported`] unless the backend supports it.
    fn set_bias_tee(&mut self, on: bool) -> Result<(), Error> {
        let _ = on;
        Err(Error::Unsupported("bias tee"))
    }
    /// Cloneable handle for changing settings from other threads, also while streaming.
    fn control(&self) -> Arc<dyn DeviceControl>;
    /// Start streaming. `queue_blocks` bounds the channel between device and consumer.
    ///
    /// # Errors
    /// Fails if the device is already streaming or cannot be started.
    fn start(&mut self, queue_blocks: usize) -> Result<IqStream, Error>;
    /// Stop streaming and release the producer thread. Idempotent.
    fn stop(&mut self);
}
