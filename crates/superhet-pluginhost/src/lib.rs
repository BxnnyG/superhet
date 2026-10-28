//! Superhet plugin host library: plugin registry, instance supervisor, plugin input from the
//! device daemons, instance persistence and the bus control API.
//! The binary (`main.rs`) wires them to NATS and serves `/healthz` on port 8075.

pub mod ctl;
mod framed_session;
pub mod inputs;
pub mod metrics;
pub mod outputs;
pub mod registry;
pub mod store;
pub mod supervisor;
