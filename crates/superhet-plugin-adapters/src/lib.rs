//! Adapters between external decoder programs and the Superhet plugin contract
//! (`docs/plugin-contract.md`, simple transport).
//!
//! Each first-party decoder plugin is a manifest plus one call of the `superhet-plugin-adapter`
//! binary. The adapter starts the real tool (multimon-ng, direwolf, rtl_433) as a child process
//! that inherits the plugin's `stdin` (the samples fed by the host), reads what the tool prints
//! and writes contract events (JSON Lines) to `stdout`. The parsers are plain functions from one
//! output line to an event, so they are unit-tested without the tools; the golden tests in
//! `tests/golden.rs` run the whole chain on committed SigMF recordings.

pub mod aprs;
pub mod dab;
pub mod event;
pub mod pager;
pub mod rtl433;
pub mod run;
