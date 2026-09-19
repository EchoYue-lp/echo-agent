//! ChannelManager — manages the lifecycle of multiple IM channel plugins

use super::types::{ChannelPlugin, MessageHandler};
use echo_core::error::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

/// Identity-bearing lifecycle result for one channel.
pub struct ChannelLifecycleResult {
    pub channel_id: String,
    pub result: Result<()>,
}

/// Manage startup, shutdown, and querying of multiple IM channels.
///
/// Supports:
/// - Registering multiple ChannelPlugins (QQ Bot, Feishu, etc.)
/// - Unified start / stop
/// - Query or send by ID
/// - Auto-stop all channels on Drop
pub struct ChannelManager {
    channels: HashMap<String, Box<dyn ChannelPlugin>>,
    handlers: HashMap<String, Arc<dyn MessageHandler>>,
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
            handlers: HashMap::new(),
        }
    }

    /// Register a channel plugin
    pub fn register(&mut self, plugin: Box<dyn ChannelPlugin>) -> Result<()> {
        let id = plugin.id().to_string();
        if self.channels.contains_key(&id) {
            return Err(echo_core::error::ReactError::Channel(Box::new(
                echo_core::error::ChannelError::Other(format!(
                    "Channel '{id}' is already registered"
                )),
            )));
        }
        info!("Registering channel: {}", id);
        self.channels.insert(id, plugin);
        Ok(())
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
    ) -> Vec<ChannelLifecycleResult> {
        info!("Starting all channels (count: {})", self.channels.len());

        let mut results = Vec::with_capacity(self.channels.len());

        for (id, plugin) in self.channels.iter_mut() {
            if self.handlers.contains_key(id) {
                results.push(ChannelLifecycleResult {
                    channel_id: id.clone(),
                    result: Err(echo_core::error::ReactError::Channel(Box::new(
                        echo_core::error::ChannelError::Other(format!(
                            "Channel '{id}' still owns a handler; stop it before restarting"
                        )),
                    ))),
                });
                continue;
            }
            let handler = handler_factory(id);
            self.handlers.insert(id.clone(), Arc::clone(&handler));
            match plugin.start(Arc::clone(&handler)).await {
                Ok(()) => {
                    info!("Channel '{}' started successfully", id);
                    results.push(ChannelLifecycleResult {
                        channel_id: id.clone(),
                        result: Ok(()),
                    });
                }
                Err(e) => {
                    warn!("Failed to start channel '{}': {}", id, e);
                    // A failed start may have retained the handler or opened a
                    // transport. Keep ownership whenever rollback is incomplete.
                    let failure = match plugin.stop().await {
                        Ok(()) => match handler.close().await {
                            Ok(()) => {
                                self.handlers.remove(id);
                                e
                            }
                            Err(close_error) => {
                                warn!(
                                    "Channel '{}' handler close after failed start: {}",
                                    id, close_error
                                );
                                echo_core::error::ReactError::Channel(Box::new(
                                    echo_core::error::ChannelError::Other(format!(
                                        "Channel '{id}' start failed: {e}; handler close failed: {close_error}"
                                    )),
                                ))
                            }
                        },
                        Err(stop_error) => {
                            warn!("Channel '{}' stop after failed start: {}", id, stop_error);
                            echo_core::error::ReactError::Channel(Box::new(
                                echo_core::error::ChannelError::Other(format!(
                                    "Channel '{id}' start failed: {e}; rollback stop failed: {stop_error}"
                                )),
                            ))
                        }
                    };
                    results.push(ChannelLifecycleResult {
                        channel_id: id.clone(),
                        result: Err(failure),
                    });
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
            if let Some(handler) = self.handlers.get(channel_id) {
                handler.close().await?;
                self.handlers.remove(channel_id);
            }
            info!("Channel '{}' stopped", channel_id);
            Ok(())
        } else {
            Err(echo_core::error::ReactError::Channel(Box::new(
                echo_core::error::ChannelError::Other(format!(
                    "Channel '{}' not found",
                    channel_id
                )),
            )))
        }
    }

    /// Stop all registered channels
    pub async fn stop_all(&mut self) -> Result<()> {
        info!("Stopping all channels...");

        let mut failures = Vec::new();
        for id in self.channels.keys().cloned().collect::<Vec<_>>() {
            match self.stop(&id).await {
                Ok(()) => info!("Channel '{}' stopped", id),
                Err(e) => {
                    warn!("Failed to stop channel '{}': {}", id, e);
                    failures.push(format!("{id}: {e}"));
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(echo_core::error::ReactError::Channel(Box::new(
                echo_core::error::ChannelError::Other(format!(
                    "failed to stop channels: {}",
                    failures.join("; ")
                )),
            )))
        }
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

#[cfg(test)]
mod close_tests {
    use super::*;
    use crate::channels::types::{ChannelCapabilities, ChatType, InboundMessage, OutboundMessage};
    use async_trait::async_trait;
    use echo_core::error::{ChannelError, ReactError};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::Notify;

    struct ClosingHandler {
        stopped: Arc<AtomicBool>,
        closes: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl MessageHandler for ClosingHandler {
        async fn handle(&self, _msg: InboundMessage) -> Result<OutboundMessage> {
            Err(ReactError::Other("not used".to_string()))
        }

        async fn reply(&self, _msg: OutboundMessage) -> Result<()> {
            Ok(())
        }

        async fn close(&self) -> Result<()> {
            if !self.stopped.load(Ordering::Acquire) {
                return Err(ReactError::Other("transport still accepting".to_string()));
            }
            let attempt = self.closes.fetch_add(1, Ordering::AcqRel);
            if attempt == 0 {
                Err(ReactError::Other("transient close failure".to_string()))
            } else {
                Ok(())
            }
        }
    }

    struct ClosingPlugin {
        stopped: Arc<AtomicBool>,
        handler: Option<Arc<dyn MessageHandler>>,
    }

    #[async_trait]
    impl ChannelPlugin for ClosingPlugin {
        fn id(&self) -> &str {
            "close-test"
        }

        fn capabilities(&self) -> &ChannelCapabilities {
            static CAPABILITIES: ChannelCapabilities = ChannelCapabilities {
                chat_types: &[ChatType::Direct],
                supports_media: false,
                supports_threads: false,
            };
            &CAPABILITIES
        }

        async fn start(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()> {
            self.stopped.store(false, Ordering::Release);
            self.handler = Some(handler);
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            self.stopped.store(true, Ordering::Release);
            self.handler = None;
            Ok(())
        }

        async fn send(&self, _msg: OutboundMessage) -> Result<()> {
            Err(ReactError::Channel(Box::new(ChannelError::Other(
                "not used".to_string(),
            ))))
        }
    }

    #[tokio::test]
    async fn stop_retains_handler_until_awaited_close_succeeds() -> Result<()> {
        let stopped = Arc::new(AtomicBool::new(false));
        let closes = Arc::new(AtomicUsize::new(0));
        let mut manager = ChannelManager::new();
        manager.register(Box::new(ClosingPlugin {
            stopped: Arc::clone(&stopped),
            handler: None,
        }))?;
        let results = manager
            .start_all(|_| {
                Arc::new(ClosingHandler {
                    stopped: Arc::clone(&stopped),
                    closes: Arc::clone(&closes),
                })
            })
            .await;
        assert_eq!(results.len(), 1);
        assert!(results.iter().all(|result| result.result.is_ok()));

        let first = manager.stop("close-test").await;
        assert!(first.is_err());
        assert_eq!(closes.load(Ordering::Acquire), 1);
        manager.stop("close-test").await?;
        assert_eq!(closes.load(Ordering::Acquire), 2);
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_stop_keeps_handler_for_retry() -> Result<()> {
        struct InterruptedHandler {
            entered: Arc<Notify>,
            closes: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl MessageHandler for InterruptedHandler {
            async fn handle(&self, _msg: InboundMessage) -> Result<OutboundMessage> {
                Err(ReactError::Other("not used".to_string()))
            }
            async fn reply(&self, _msg: OutboundMessage) -> Result<()> {
                Ok(())
            }
            async fn close(&self) -> Result<()> {
                if self.closes.fetch_add(1, Ordering::AcqRel) == 0 {
                    self.entered.notify_one();
                    std::future::pending::<()>().await;
                }
                Ok(())
            }
        }

        let stopped = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(Notify::new());
        let closes = Arc::new(AtomicUsize::new(0));
        let mut manager = ChannelManager::new();
        manager.register(Box::new(ClosingPlugin {
            stopped,
            handler: None,
        }))?;
        let results = manager
            .start_all(|_| {
                Arc::new(InterruptedHandler {
                    entered: Arc::clone(&entered),
                    closes: Arc::clone(&closes),
                })
            })
            .await;
        assert!(results.iter().all(|result| result.result.is_ok()));

        assert!(
            tokio::time::timeout(Duration::from_millis(20), manager.stop("close-test"))
                .await
                .is_err()
        );
        assert_eq!(closes.load(Ordering::Acquire), 1);
        manager.stop("close-test").await?;
        assert_eq!(closes.load(Ordering::Acquire), 2);
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_start_keeps_published_handler_for_stop_all() -> Result<()> {
        struct ParkedStartPlugin {
            started: Arc<Notify>,
            stopped: Arc<AtomicBool>,
            handler: Option<Arc<dyn MessageHandler>>,
        }

        #[async_trait]
        impl ChannelPlugin for ParkedStartPlugin {
            fn id(&self) -> &str {
                "parked-start"
            }
            fn capabilities(&self) -> &ChannelCapabilities {
                static CAPABILITIES: ChannelCapabilities = ChannelCapabilities {
                    chat_types: &[ChatType::Direct],
                    supports_media: false,
                    supports_threads: false,
                };
                &CAPABILITIES
            }
            async fn start(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()> {
                self.handler = Some(handler);
                self.started.notify_one();
                std::future::pending::<Result<()>>().await
            }
            async fn stop(&mut self) -> Result<()> {
                self.stopped.store(true, Ordering::Release);
                self.handler = None;
                Ok(())
            }
            async fn send(&self, _msg: OutboundMessage) -> Result<()> {
                Ok(())
            }
        }

        struct ParkedStartHandler {
            stopped: Arc<AtomicBool>,
            closes: Arc<AtomicUsize>,
        }
        #[async_trait]
        impl MessageHandler for ParkedStartHandler {
            async fn handle(&self, _msg: InboundMessage) -> Result<OutboundMessage> {
                Err(ReactError::Other("not used".to_string()))
            }
            async fn reply(&self, _msg: OutboundMessage) -> Result<()> {
                Ok(())
            }
            async fn close(&self) -> Result<()> {
                if !self.stopped.load(Ordering::Acquire) {
                    return Err(ReactError::Other("transport still running".to_string()));
                }
                self.closes.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
        }

        let started = Arc::new(Notify::new());
        let stopped = Arc::new(AtomicBool::new(false));
        let closes = Arc::new(AtomicUsize::new(0));
        let mut manager = ChannelManager::new();
        manager.register(Box::new(ParkedStartPlugin {
            started: Arc::clone(&started),
            stopped: Arc::clone(&stopped),
            handler: None,
        }))?;
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                manager.start_all(|_| {
                    Arc::new(ParkedStartHandler {
                        stopped: Arc::clone(&stopped),
                        closes: Arc::clone(&closes),
                    })
                })
            )
            .await
            .is_err()
        );
        started.notified().await;
        manager.stop_all().await?;
        assert_eq!(closes.load(Ordering::Acquire), 1);
        Ok(())
    }
}
