//! Feishu/Lark Channel Module
//!
//! Supports two receive modes:
//! - WebSocket long connection (Long Poll): no public IP required, pure Rust implementation
//! - Webhook: requires public IP, HTTP event push

pub mod api;
pub mod channel;
pub mod long_poll;
pub mod proto;
pub mod webhook;

pub use channel::{FeishuChannel, FeishuConfig, FeishuMode};
