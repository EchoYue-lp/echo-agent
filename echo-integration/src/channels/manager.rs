//! ChannelManager —— 管理多个 IM 通道插件的生命周期

use super::types::{ChannelPlugin, MessageHandler};
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
/// - Drop 时自动停止所有通道
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
    /// 为每个通道创建一个 task 并发启动。返回 `Vec<Result<()>>`，
    /// 调用者可以查看每个通道的启动结果，单个失败不影响其他通道。
    pub async fn start_all(
        &mut self,
        handler_factory: impl Fn(&str) -> Arc<dyn MessageHandler> + Sync,
    ) -> Vec<Result<()>> {
        info!("Starting all channels (count: {})", self.channels.len());

        let mut results = Vec::with_capacity(self.channels.len());

        for (id, plugin) in self.channels.iter_mut() {
            let handler = handler_factory(id);
            match plugin.start(handler).await {
                Ok(()) => {
                    info!("Channel '{}' started successfully", id);
                    results.push(Ok(()));
                }
                Err(e) => {
                    warn!("Failed to start channel '{}': {}", id, e);
                    results.push(Err(e));
                }
            }
        }

        results
    }

    /// 停止单个通道
    pub async fn stop(&mut self, channel_id: &str) -> Result<()> {
        if let Some(plugin) = self.channels.get_mut(channel_id) {
            info!("Stopping channel: {}", channel_id);
            plugin.stop().await?;
            info!("Channel '{}' stopped", channel_id);
            Ok(())
        } else {
            Err(echo_core::error::ReactError::Channel(
                echo_core::error::ChannelError::Other(format!(
                    "Channel '{}' not found",
                    channel_id
                )),
            ))
        }
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
    pub fn get(&self, id: &str) -> Option<&(dyn ChannelPlugin + '_)> {
        match self.channels.get(id) {
            Some(plugin) => Some(plugin.as_ref()),
            None => None,
        }
    }

    /// 获取通道可变引用（通过 ID）
    pub fn get_mut(&mut self, id: &str) -> Option<&mut (dyn ChannelPlugin + '_)> {
        match self.channels.get_mut(id) {
            Some(plugin) => Some(plugin.as_mut()),
            None => None,
        }
    }

    /// 列出所有已注册的通道 ID
    pub fn channel_ids(&self) -> Vec<&str> {
        self.channels.keys().map(|k| k.as_str()).collect()
    }
}

impl Drop for ChannelManager {
    fn drop(&mut self) {
        if !self.channels.is_empty() {
            info!(
                "ChannelManager dropped with {} channels remaining, \
                 consider calling stop_all() before drop",
                self.channels.len()
            );
        }
    }
}
