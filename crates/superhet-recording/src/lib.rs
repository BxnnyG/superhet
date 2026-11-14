//! Recording building blocks shared by the API service (ADR-0028): Opus packets in Ogg without
//! re-encoding, SigMF metadata for IQ, the per-VFO replay ring buffer, the squelch gate, quota
//! planning and file-name safety. Everything here is pure (no I/O besides what the callers pass
//! in), so every rule is unit-tested.

pub mod files;
pub mod gate;
pub mod ogg;
pub mod quota;
pub mod ring;
pub mod sigmf;
