//! # echo-channels
//!
//! IM 通道插件系统 —— 统一的 ChannelPlugin 接口 + 具体平台实现。
//!
//! ## 快速开始
//!
//! ```rust,no_run
//! use echo_channels::prelude::*;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut manager = ChannelManager::new();
//!
//! // 注册 QQ Bot
//! let qq_config = QqConfig {
//!     app_id: "your-app-id".into(),
//!     client_secret: "your-secret".into(),
//! };
//! manager.register(Box::new(QqChannel::new(qq_config)?));
//!
//! // 注册飞书
//! let feishu_config = FeishuConfig {
//!     app_id: "your-feishu-app-id".into(),
//!     app_secret: "your-feishu-secret".into(),
//!     webhook_port: 8080,
//!     webhook_path: "/feishu/webhook".into(),
//! };
//! manager.register(Box::new(FeishuChannel::new(feishu_config)?));
//!
//! // 启动所有通道
//! let ctx = ChannelContext::new(handler);
//! manager.start_all(ctx).await?;
//! # Ok(())
//! # }
//! ```

pub mod channels;
pub mod manager;
pub mod types;

pub use channels::qq::channel::{QqChannel, QqConfig};
pub use channels::feishu::channel::{FeishuChannel, FeishuConfig};
pub use manager::ChannelManager;
pub use types::*;

pub mod prelude {
    pub use crate::channels::qq::channel::{QqChannel, QqConfig};
    pub use crate::channels::feishu::channel::{FeishuChannel, FeishuConfig};
    pub use crate::manager::ChannelManager;
    pub use crate::types::*;
    pub use crate::ChannelPlugin;
}
