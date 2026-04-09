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
//! // 注册飞书（长连接模式，无需公网 IP）
//! let feishu_config = FeishuConfig::new_long_poll(
//!     "your-feishu-app-id".into(),
//!     "your-feishu-secret".into(),
//! );
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
pub mod session;
pub mod types;

pub use channels::feishu::channel::{FeishuChannel, FeishuConfig};
pub use channels::qq::channel::{QqChannel, QqConfig};
pub use manager::ChannelManager;
pub use session::{SessionConfig, SessionFactory, SessionHandler};
pub use types::*;

pub mod prelude {
    pub use crate::ChannelPlugin;
    pub use crate::channels::feishu::channel::{FeishuChannel, FeishuConfig};
    pub use crate::channels::qq::channel::{QqChannel, QqConfig};
    pub use crate::manager::ChannelManager;
    pub use crate::session::{SessionConfig, SessionFactory, SessionHandler};
    pub use crate::types::*;
}
