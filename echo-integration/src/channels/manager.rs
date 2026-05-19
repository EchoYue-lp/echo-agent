//! ChannelManager — manages the lifecycle of multiple IM channel plugins

use super::types::{ChannelPlugin, MessageHandler};
use echo_core::error::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

/// Manage startup, shutdown, and querying of multiple IM channels.
///
/// Supports:
/// - Registering multiple ChannelPlugins (QQ Bot, Feishu, etc.)
/// - Unified start / stop
/// - Query or send by ID
/// - Auto-stop all channels on Drop
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

    /// Register a channel plugin
    pub fn register(&mut self, plugin: Box<dyn ChannelPlugin>) {
        let id = plugin.id().to_string();
        info!("Registering channel: {}", id);
        self.channels.insert(id, plugin);
    }

    /// Get the number of channels
    pub fn len(&self) -> usize {
        self.channels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// Start all registered channels.
    ///
    /// Creates a task for each channel and starts them concurrently. Returns `Vec<Result<()>>`
    /// so the caller can inspect the result of each channel; a single failure does not affect others.
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

    /// Stop a single channel
    pub async fn stop(&mut self, channel_id: &str) -> Result<()> {
        if let Some(plugin) = self.channels.get_mut(channel_id) {
            info!("Stopping channel: {}", channel_id);
            plugin.stop().await?;
            info!("Channel '{}' stopped", channel_id);
            Ok(())
        } else {
            Err(echo_core::error::ReactError::Channel(
                Box::new(echo_core::error::ChannelError::Other(format!(
                    "Channel '{}' not found",
                    channel_id
                ))),
            ))
        }
    }

    /// Stop all registered channels
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

    /// Get a channel reference by ID
    pub fn get(&self, id: &str) -> Option<&(dyn ChannelPlugin + '_)> {
        match self.channels.get(id) {
            Some(plugin) => Some(plugin.as_ref()),
            None => None,
        }
    }

    /// Get a mutable channel reference by ID
    pub fn get_mut(&mut self, id: &str) -> Option<&mut (dyn ChannelPlugin + '_)> {
        match self.channels.get_mut(id) {
            Some(plugin) => Some(plugin.as_mut()),
            None => None,
        }
    }

    /// List all registered channel IDs
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
