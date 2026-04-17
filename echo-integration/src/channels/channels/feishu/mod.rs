//! Feishu/Lark Channel Module
//!
//! 支持两种接收模式：
//! - WebSocket 长连接（Long Poll）：无需公网 IP，纯 Rust 实现
//! - Webhook：需要公网 IP，HTTP 事件推送

pub mod api;
pub mod channel;
pub mod long_poll;
pub mod proto;
pub mod webhook;

pub use channel::{FeishuChannel, FeishuConfig, FeishuMode};
