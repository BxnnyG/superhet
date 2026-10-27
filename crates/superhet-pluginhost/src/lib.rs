//! Superhet plugin host library: plugin registry, instance supervisor and the bus control API.
//! The binary (`main.rs`) wires them to NATS and serves `/healthz` on port 8075.

pub mod ctl;
pub mod registry;
pub mod supervisor;
