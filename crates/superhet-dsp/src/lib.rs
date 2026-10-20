//! Digital signal processing for Superhet: FFT, DDC, demodulators and resampling.

pub mod spectrum;

pub use spectrum::{SpectrumConfig, SpectrumError, SpectrumProcessor};
