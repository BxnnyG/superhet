//! Scanner (`docs/scanner.md`, ADR-0025): scans frequency ranges and channel lists, stops on a
//! signal and resumes after a hang time.
//!
//! Detection uses the FFT of the spectrum pipeline: every channel of the current window is
//! measured on the same frame, so a scan inside the visible span needs no retune. Windows that
//! do not fit are visited by retuning the device. While a scan runs the scanner is a holder of
//! the tuning lock (nobody else retunes, an exclusive plugin claim stops the scan). When the
//! scanner stops on a channel it moves one VFO there.

pub mod machine;
pub mod plan;
pub mod runtime;
pub mod wire;

pub use runtime::{HitSink, ScanProbe, Scanner};
