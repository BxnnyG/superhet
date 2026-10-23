//! Digital signal processing for Superhet: FFT, DDC, demodulators and resampling.
//!
//! - [`spectrum`]: streaming spectrum estimator for the waterfall.
//! - [`zoom`]: zoom spectrum (shift, decimate, FFT) with real resolution gain.
//! - [`channel`]: per-VFO channelizer (NCO, polyphase decimation, channel filter) and WFM / NFM /
//!   AM / USB / LSB / CW demodulators with squelch and AGC, producing 48 kHz audio (WFM
//!   optionally as stereo). Building blocks are in [`nco`], [`fir`], [`demod`] and [`resample`].
//!   Pure DSP: no I/O, no threads.
//! - [`wfm`]: 19 kHz pilot PLL (stereo carrier, lock detection) of the broadcast-FM multiplex.
//! - [`rds`]: RDS receiver (57 kHz subcarrier to PS, RadioText, PI, PTY).
//! - [`tones`]: CTCSS sub-tone and DTMF digit detection on NFM / AM audio.

pub mod channel;
pub mod demod;
pub mod fir;
pub mod nco;
pub mod rds;
pub mod resample;
pub mod spectrum;
#[cfg(any(test, feature = "testsig"))]
pub mod testsig;
pub mod tones;
pub mod wfm;
pub mod zoom;

pub use channel::{
    CW_DEFAULT_PITCH_HZ, CW_PITCH_RANGE_HZ, Channel, ChannelConfig, ChannelStats, Deemphasis,
    DspError, Mode, OUTPUT_RATE, SSB_LOW_CUT_HZ,
};
pub use rds::RdsInfo;
pub use spectrum::{SpectrumConfig, SpectrumError, SpectrumProcessor};
pub use tones::{CTCSS_TONES_HZ, DTMF_TAIL_LEN, ToneDetector, canonical_ctcss};
pub use zoom::{ZoomConfig, ZoomError, ZoomSpectrum, shift_for_span};

#[cfg(test)]
mod wfm_tests;
