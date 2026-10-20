//! Digital signal processing for Superhet: FFT, DDC, demodulators and resampling.
//!
//! - [`spectrum`]: streaming spectrum estimator for the waterfall.
//! - [`channel`]: per-VFO channelizer (NCO, polyphase decimation, channel filter) and WFM / NFM /
//!   AM demodulators with squelch and AGC, producing 48 kHz mono audio. Building blocks are in
//!   [`nco`], [`fir`], [`demod`] and [`resample`]. Pure DSP: no I/O, no threads.

pub mod channel;
pub mod demod;
pub mod fir;
pub mod nco;
pub mod resample;
pub mod spectrum;

pub use channel::{Channel, ChannelConfig, ChannelStats, Deemphasis, DspError, Mode, OUTPUT_RATE};
pub use spectrum::{SpectrumConfig, SpectrumError, SpectrumProcessor};
