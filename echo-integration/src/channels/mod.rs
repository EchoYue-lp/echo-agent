//! # echo_channels
//!
//! IM channel plugin system — unified ChannelPlugin interface + concrete platform implementations.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use echo_integration::channels::prelude::*;
//! use async_trait::async_trait;
//! use echo_core::error::Result;
//! use std::sync::Arc;
//!
//! struct DummyHandler;
//!
//! #[async_trait]
//! impl MessageHandler for DummyHandler {
//!     async fn handle(&self, msg: InboundMessage) -> Result<OutboundMessage> {
//!         Ok(OutboundMessage::new(
//!             msg.channel_id,
//!             msg.chat_id,
//!             msg.chat_type,
//!             "ok",
//!         ))
//!     }
//!
//!     async fn reply(&self, _msg: OutboundMessage) -> Result<()> {
//!         Ok(())
//!     }
//! }
//!
//! # async fn example() -> Result<()> {
//! let mut manager = ChannelManager::new();
//!
//! // Register QQ Bot
//! let qq_config = QqConfig {
//!     app_id: "your-app-id".into(),
//!     client_secret: "your-secret".into(),
//! };
//! manager.register(Box::new(QqChannel::new(qq_config)?));
//!
//! // Register Feishu (long-poll mode, no public IP required)
//! let feishu_config = FeishuConfig::new_long_poll(
//!     "your-feishu-app-id".into(),
//!     "your-feishu-secret".into(),
//! );
//! manager.register(Box::new(FeishuChannel::new(feishu_config)?));
//!
//! // Start all channels
//! let handler_factory = |_channel_id: &str| -> Arc<dyn MessageHandler> {
//!     Arc::new(DummyHandler)
//! };
//! for result in manager.start_all(handler_factory).await {
//!     result?;
//! }
//! # Ok(())
//! # }
//! ```

#[allow(clippy::module_inception)]
pub mod channels;
pub mod manager;
pub mod session;
pub mod types;

pub use channels::feishu::channel::{FeishuChannel, FeishuConfig};
pub use channels::qq::channel::{QqChannel, QqConfig};
pub use manager::ChannelManager;
pub use session::{
    SessionConfig, SessionEndInfo, SessionEndReason, SessionFactory, SessionHandler,
};
pub use types::*;

pub mod prelude {
    pub use super::ChannelPlugin;
    pub use super::channels::feishu::channel::{FeishuChannel, FeishuConfig};
    pub use super::channels::qq::channel::{QqChannel, QqConfig};
    pub use super::manager::ChannelManager;
    pub use super::session::{
        SessionConfig, SessionEndInfo, SessionEndReason, SessionFactory, SessionHandler,
    };
    pub use super::types::*;
}
