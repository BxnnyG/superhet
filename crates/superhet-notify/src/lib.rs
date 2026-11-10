//! Notifications (ADR-0027): which decoder events match a rule, what the message says, and how
//! it reaches a webhook, a mailbox or a browser. Pure logic and senders; storage lives in
//! `superhet-db::notifications`, orchestration in `superhet-worker`, the admin API in
//! `superhet-api`.

pub mod egress;
pub mod email;
pub mod message;
pub mod push;
pub mod retry;
pub mod rule;
pub mod ssrf;
pub mod target;
pub mod template;
pub mod webhook;

pub use egress::{Egress, SendError};
pub use message::Message;
