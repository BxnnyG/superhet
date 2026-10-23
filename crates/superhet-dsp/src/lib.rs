//! Digital signal processing for Superhet: FFT, DDC, demodulators and resampling.
//!
//! - [`spectrum`]: streaming spectrum estimator for the waterfall.
//! - [`zoom`]: zoom spectrum (shift, decimate, FFT) with real resolution gain.
//! - [`channel`]: per-VFO channelizer (NCO, polyphase decimation, channel filter) and WFM / NFM /
//!   AM / USB / LSB / CW demodulators with squelch and AGC, producing 48 kHz mono audio. Building
//!   blocks are in [`nco`], [`fir`], [`demod`] and [`resample`]. Pure DSP: no I/O, no threads.

pub mod channel;
pub mod demod;
pub mod fir;
pub mod nco;
pub mod resample;
pub mod spectrum;
pub mod zoom;

pub use channel::{
    CW_DEFAULT_PITCH_HZ, CW_PITCH_RANGE_HZ, Channel, ChannelConfig, ChannelStats, Deemphasis,
    DspError, Mode, OUTPUT_RATE, SSB_LOW_CUT_HZ,
};
pub use spectrum::{SpectrumConfig, SpectrumError, SpectrumProcessor};
pub use zoom::{ZoomConfig, ZoomError, ZoomSpectrum, shift_for_span};
