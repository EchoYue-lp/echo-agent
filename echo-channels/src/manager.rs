//! ChannelManager —— 管理多个 IM 通道插件的生命周期

use crate::types::{ChannelPlugin, MessageHandler};
use echo_core::error::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

/// 管理多个 IM 通道的启动、停止、查询
///
/// 支持：
/// - 注册多个 ChannelPlugin（QQ Bot、飞书等）
/// - 统一启动 / 停止
/// - 按 ID 查询或发送
pub struct ChannelManager {
    channels: HashMap<String, Box<dyn ChannelPlugin>>,
}

impl Default for ChannelManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelManager {
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
        }
    }

    /// 注册一个通道插件
    pub fn register(&mut self, plugin: Box<dyn ChannelPlugin>) {
        let id = plugin.id().to_string();
        info!("Registering channel: {}", id);
        self.channels.insert(id, plugin);
    }

    /// 获取通道数量
    pub fn len(&self) -> usize {
        self.channels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// 启动所有已注册的通道
    ///
    /// 为每个通道创建一个 task 并发启动，失败时记录警告但不停止其他通道。
    pub async fn start_all(
        &mut self,
        handler_factory: impl Fn(&str) -> Arc<dyn MessageHandler> + Sync,
    ) -> Result<()> {
        info!("Starting all channels (count: {})", self.channels.len());

        for (id, plugin) in self.channels.iter_mut() {
            let handler = handler_factory(id);
            match plugin.start(handler).await {
                Ok(()) => info!("Channel '{}' started successfully", id),
                Err(e) => warn!("Failed to start channel '{}': {}", id, e),
            }
        }

        Ok(())
    }

    /// 停止所有已注册的通道
    pub async fn stop_all(&mut self) -> Result<()> {
        info!("Stopping all channels...");

        for (id, plugin) in self.channels.iter_mut() {
            match plugin.stop().await {
                Ok(()) => info!("Channel '{}' stopped", id),
                Err(e) => warn!("Failed to stop channel '{}': {}", id, e),
            }
        }

        Ok(())
    }

    /// 获取通道引用（通过 ID）
    pub fn get(&self, id: &str) -> Option<&Box<dyn ChannelPlugin>> {
        self.channels.get(id)
    }

    /// 列出所有已注册的通道 ID
    pub fn channel_ids(&self) -> Vec<&str> {
        self.channels.keys().map(|k| k.as_str()).collect()
    }
}
