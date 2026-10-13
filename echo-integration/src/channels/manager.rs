//! ChannelManager — one lifecycle authority per original IM channel plugin.

use super::types::{ChannelCapabilities, ChannelPlugin, MessageHandler, OutboundMessage};
use async_trait::async_trait;
use echo_core::error::{ChannelError, ReactError, Result};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Identity-bearing lifecycle result for one channel.
pub struct ChannelLifecycleResult {
    pub channel_id: String,
    pub result: Result<()>,
}

fn channel_error(message: String) -> ReactError {
    ReactError::Channel(Box::new(ChannelError::Other(message)))
}

struct ChannelState {
    plugin: Box<dyn ChannelPlugin>,
    handler: Option<Arc<dyn MessageHandler>>,
    stopped_at_epoch: Option<u64>,
}

struct ChannelEntry {
    id: String,
    label: String,
    capabilities: ChannelCapabilities,
    state: RwLock<ChannelState>,
    mutation_epoch: AtomicU64,
    // Diagnostic projection only. All cleanup decisions use state.handler.
    unsettled_handler: AtomicBool,
}

/// Owned access to the same registered plugin and its cleanup responsibility.
///
/// Metadata is the registration-time declaration, fixed for this registration.
/// Operations retain the original instance after the manager borrow ends. A
/// callback may query manager metadata; recursively awaiting another mutation
/// of this same channel is unsupported because lifecycle operations serialize.
#[derive(Clone)]
pub struct ChannelHandle {
    entry: Arc<ChannelEntry>,
}

impl ChannelHandle {
    pub fn id(&self) -> &str {
        &self.entry.id
    }

    pub fn label(&self) -> &str {
        &self.entry.label
    }

    pub fn capabilities(&self) -> &ChannelCapabilities {
        &self.entry.capabilities
    }

    fn invalidate_stopped(&self) {
        // MAX remains permanently conservative, including concurrent invalidation
        // while stop awaits a plugin callback. Never wrap into an old epoch.
        let mut epoch = self.entry.mutation_epoch.load(Ordering::Acquire);
        loop {
            match self.entry.mutation_epoch.compare_exchange_weak(
                epoch,
                epoch.saturating_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => epoch = current,
            }
        }
    }

    async fn start_with(
        &self,
        handler_factory: impl FnOnce() -> Arc<dyn MessageHandler> + Send,
    ) -> Result<()> {
        let mut state = self.entry.state.write().await;
        if state.handler.is_some() {
            return Err(channel_error(format!(
                "Channel '{}' still owns a handler; stop it before restarting",
                self.id()
            )));
        }
        state.stopped_at_epoch = None;
        let handler = handler_factory();
        state.handler = Some(Arc::clone(&handler));
        self.entry.unsettled_handler.store(true, Ordering::Release);
        match state.plugin.start(Arc::clone(&handler)).await {
            Ok(()) => Ok(()),
            Err(start_error) => {
                // Publish responsibility before the first plugin await. Future
                // cancellation leaves that same handler available to stop.
                let epoch = self.entry.mutation_epoch.load(Ordering::Acquire);
                match state.plugin.stop().await {
                    Ok(()) => {
                        state.stopped_at_epoch = Some(epoch);
                        match handler.close().await {
                            Ok(()) => {
                                state.handler = None;
                                self.entry.unsettled_handler.store(false, Ordering::Release);
                                Err(start_error)
                            }
                            Err(close_error) => Err(channel_error(format!(
                                "Channel '{}' start failed: {start_error}; handler close failed: {close_error}",
                                self.id()
                            ))),
                        }
                    }
                    Err(stop_error) => Err(channel_error(format!(
                        "Channel '{}' start failed: {start_error}; rollback stop failed: {stop_error}",
                        self.id()
                    ))),
                }
            }
        }
    }

    /// Start using the original plugin, retaining handler responsibility on failure or cancellation.
    pub fn start(&self, handler: Arc<dyn MessageHandler>) -> BoxFuture<'static, Result<()>> {
        let channel = self.clone();
        Box::pin(async move { channel.start_with(|| handler).await })
    }

    /// Await transport stop and handler close. Retry does not repeat a confirmed stop.
    pub fn stop(&self) -> BoxFuture<'static, Result<()>> {
        let channel = self.clone();
        Box::pin(async move {
            let mut state = channel.entry.state.write().await;
            let epoch = channel.entry.mutation_epoch.load(Ordering::Acquire);
            if epoch == u64::MAX || state.stopped_at_epoch != Some(epoch) {
                state.plugin.stop().await?;
                state.stopped_at_epoch = Some(epoch);
            }
            if let Some(handler) = &state.handler {
                handler.close().await?;
                state.handler = None;
                channel
                    .entry
                    .unsettled_handler
                    .store(false, Ordering::Release);
            }
            Ok(())
        })
    }

    pub fn send(&self, message: OutboundMessage) -> BoxFuture<'static, Result<()>> {
        let channel = self.clone();
        Box::pin(async move { channel.entry.state.read().await.plugin.send(message).await })
    }

    pub fn health_check(&self) -> BoxFuture<'static, Result<()>> {
        let channel = self.clone();
        Box::pin(async move { channel.entry.state.read().await.plugin.health_check().await })
    }
}

#[async_trait]
impl ChannelPlugin for ChannelHandle {
    fn id(&self) -> &str {
        self.id()
    }
    fn label(&self) -> &str {
        self.label()
    }
    fn capabilities(&self) -> &ChannelCapabilities {
        self.capabilities()
    }
    async fn start(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()> {
        ChannelHandle::start(self, handler).await
    }
    async fn stop(&mut self) -> Result<()> {
        ChannelHandle::stop(self).await
    }
    async fn send(&self, message: OutboundMessage) -> Result<()> {
        ChannelHandle::send(self, message).await
    }
    async fn health_check(&self) -> Result<()> {
        ChannelHandle::health_check(self).await
    }
}

/// Registry and lifecycle access for IM channel plugins.
///
/// `id`, `label` and `capabilities` are fixed registration declarations. The old
/// borrowed accessors delegate to the same entry as owned access; they do not
/// expose later dynamic changes to a third-party plugin's metadata.
/// Obtain an owned operation under an external manager lock, release that lock,
/// then await. Explicit awaited shutdown is required; dropping is not shutdown.
pub struct ChannelManager {
    channels: HashMap<String, ChannelHandle>,
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

    /// Register the original plugin with its fixed metadata declaration.
    pub fn register(&mut self, plugin: Box<dyn ChannelPlugin>) -> Result<()> {
        let id = plugin.id().to_string();
        if self.channels.contains_key(&id) {
            return Err(channel_error(format!(
                "Channel '{id}' is already registered"
            )));
        }
        let capabilities = plugin.capabilities();
        let capabilities = ChannelCapabilities {
            chat_types: capabilities.chat_types,
            supports_media: capabilities.supports_media,
            supports_threads: capabilities.supports_threads,
        };
        let entry = Arc::new(ChannelEntry {
            id: id.clone(),
            label: plugin.label().to_string(),
            capabilities,
            state: RwLock::new(ChannelState {
                plugin,
                handler: None,
                stopped_at_epoch: None,
            }),
            mutation_epoch: AtomicU64::new(0),
            unsettled_handler: AtomicBool::new(false),
        });
        info!("Registering channel: {}", id);
        self.channels.insert(id, ChannelHandle { entry });
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.channels.len()
    }
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// Retain the original entry independently of the manager borrow.
    pub fn get_owned(&self, id: &str) -> Option<ChannelHandle> {
        self.channels.get(id).cloned()
    }

    fn snapshot(&self) -> Vec<ChannelHandle> {
        self.channels.values().cloned().collect()
    }

    async fn start_entries(
        entries: Vec<ChannelHandle>,
        handler_factory: impl Fn(&str) -> Arc<dyn MessageHandler> + Sync,
    ) -> Vec<ChannelLifecycleResult> {
        let mut results = Vec::with_capacity(entries.len());
        for channel in entries {
            let result = channel.start_with(|| handler_factory(channel.id())).await;
            results.push(ChannelLifecycleResult {
                channel_id: channel.id().to_string(),
                result,
            });
        }
        results
    }

    /// Capture current registrations now; invoke the factory only when polled.
    /// Channels registered later are considered by the next batch.
    pub fn start_all_owned(
        &self,
        handler_factory: impl Fn(&str) -> Arc<dyn MessageHandler> + Send + Sync + 'static,
    ) -> BoxFuture<'static, Vec<ChannelLifecycleResult>> {
        let entries = self.snapshot();
        Box::pin(Self::start_entries(entries, handler_factory))
    }

    /// Start the current channels sequentially and retain per-channel results.
    pub async fn start_all(
        &mut self,
        handler_factory: impl Fn(&str) -> Arc<dyn MessageHandler> + Sync,
    ) -> Vec<ChannelLifecycleResult> {
        Self::start_entries(self.snapshot(), &handler_factory).await
    }

    pub fn stop_owned(&self, channel_id: &str) -> BoxFuture<'static, Result<()>> {
        let channel = self.get_owned(channel_id);
        let id = channel_id.to_string();
        Box::pin(async move {
            channel
                .ok_or_else(|| channel_error(format!("Channel '{id}' not found")))?
                .stop()
                .await
        })
    }

    pub async fn stop(&mut self, channel_id: &str) -> Result<()> {
        self.stop_owned(channel_id).await
    }

    /// Capture current registrations now and attempt every channel on poll.
    pub fn stop_all_owned(&self) -> BoxFuture<'static, Result<()>> {
        let entries = self.snapshot();
        Box::pin(async move {
            let mut failures = Vec::new();
            for channel in entries {
                if let Err(error) = channel.stop().await {
                    warn!("Failed to stop channel '{}': {}", channel.id(), error);
                    failures.push(format!("{}: {error}", channel.id()));
                }
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(channel_error(format!(
                    "failed to stop channels: {}",
                    failures.join("; ")
                )))
            }
        })
    }

    pub async fn stop_all(&mut self) -> Result<()> {
        self.stop_all_owned().await
    }

    pub fn get(&self, id: &str) -> Option<&(dyn ChannelPlugin + '_)> {
        self.channels
            .get(id)
            .map(|channel| channel as &dyn ChannelPlugin)
    }

    /// Mutable borrowed access conservatively invalidates a prior stop fact.
    /// Availability does not change when an owned handle is also retained.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut (dyn ChannelPlugin + '_)> {
        match self.channels.get_mut(id) {
            Some(channel) => {
                channel.invalidate_stopped();
                Some(channel as &mut dyn ChannelPlugin)
            }
            None => None,
        }
    }

    pub fn channel_ids(&self) -> Vec<&str> {
        self.channels.keys().map(String::as_str).collect()
    }
}

impl Drop for ChannelManager {
    fn drop(&mut self) {
        let unsettled = self
            .channels
            .values()
            .filter(|channel| channel.entry.unsettled_handler.load(Ordering::Acquire))
            .count();
        if unsettled != 0 {
            info!(
                "ChannelManager dropped with {} unsettled handlers remaining; await stop_all() for shutdown",
                unsettled
            );
        }
    }
}

impl Drop for ChannelEntry {
    fn drop(&mut self) {
        if self.state.get_mut().handler.is_some() {
            warn!(
                "Channel '{}' final owner dropped with an unsettled handler; shutdown was not awaited",
                self.id
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
        stops: Arc<AtomicUsize>,
        fail_repeated_stop: bool,
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
            let attempt = self.stops.fetch_add(1, Ordering::AcqRel);
            if self.fail_repeated_stop && attempt > 0 {
                return Err(ReactError::Other("transport already stopped".to_string()));
            }
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
        let stops = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let mut manager = ChannelManager::new();
        manager.register(Box::new(ClosingPlugin {
            stopped: Arc::clone(&stopped),
            stops: Arc::clone(&stops),
            fail_repeated_stop: true,
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
        assert_eq!(stops.load(Ordering::Acquire), 1);
        assert!(
            manager
                .channels
                .values()
                .all(|channel| !channel.entry.unsettled_handler.load(Ordering::Acquire))
        );
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
        let stops = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Notify::new());
        let closes = Arc::new(AtomicUsize::new(0));
        let mut manager = ChannelManager::new();
        manager.register(Box::new(ClosingPlugin {
            stopped,
            stops: Arc::clone(&stops),
            fail_repeated_stop: true,
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
        assert_eq!(stops.load(Ordering::Acquire), 1);
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

#[cfg(test)]
mod owned_tests {
    use super::*;
    use crate::channels::types::{ChatType, InboundMessage};
    use std::sync::Weak;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;
    use tokio::sync::{Mutex, Notify};

    const CAPS: ChannelCapabilities = ChannelCapabilities {
        chat_types: &[ChatType::Direct],
        supports_media: false,
        supports_threads: false,
    };

    struct Probe {
        manager: Weak<Mutex<ChannelManager>>,
        calls: Mutex<Vec<&'static str>>,
        starts: AtomicUsize,
        stops: AtomicUsize,
        closes: AtomicUsize,
        fail_start: bool,
        fail_first_stop: bool,
        start_gate: Option<(Arc<Notify>, Arc<Notify>)>,
        stop_gate: Option<(Arc<Notify>, Arc<Notify>)>,
    }

    impl Probe {
        fn new(manager: &Arc<Mutex<ChannelManager>>) -> Arc<Self> {
            Arc::new(Self {
                manager: Arc::downgrade(manager),
                calls: Mutex::new(Vec::new()),
                starts: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
                closes: AtomicUsize::new(0),
                fail_start: false,
                fail_first_stop: false,
                start_gate: None,
                stop_gate: None,
            })
        }
        async fn callback(&self, operation: &'static str) -> Result<()> {
            if let Some(manager) = self.manager.upgrade() {
                let guard = manager.lock().await;
                assert!(!guard.channel_ids().is_empty());
                for id in guard.channel_ids() {
                    let plugin = guard
                        .get(id)
                        .ok_or_else(|| channel_error("missing registered plugin".into()))?;
                    assert!(!plugin.label().is_empty());
                    assert_eq!(plugin.capabilities().chat_types, &[ChatType::Direct]);
                }
            }
            self.calls.lock().await.push(operation);
            Ok(())
        }
    }

    struct ProbePlugin {
        id: &'static str,
        probe: Arc<Probe>,
    }
    #[async_trait]
    impl ChannelPlugin for ProbePlugin {
        fn id(&self) -> &str {
            self.id
        }
        fn capabilities(&self) -> &ChannelCapabilities {
            &CAPS
        }
        async fn start(&mut self, _handler: Arc<dyn MessageHandler>) -> Result<()> {
            self.probe.starts.fetch_add(1, Ordering::AcqRel);
            self.probe.callback("start").await?;
            if let Some((entered, release)) = &self.probe.start_gate {
                entered.notify_one();
                release.notified().await;
            }
            if self.probe.fail_start {
                Err(channel_error("probe start failure".into()))
            } else {
                Ok(())
            }
        }
        async fn stop(&mut self) -> Result<()> {
            let attempt = self.probe.stops.fetch_add(1, Ordering::AcqRel);
            self.probe.callback("stop").await?;
            if let Some((entered, release)) = &self.probe.stop_gate {
                entered.notify_one();
                release.notified().await;
            }
            if self.probe.fail_first_stop && attempt == 0 {
                Err(channel_error("probe stop failure".into()))
            } else {
                Ok(())
            }
        }
        async fn send(&self, _message: OutboundMessage) -> Result<()> {
            self.probe.callback("send").await
        }
        async fn health_check(&self) -> Result<()> {
            self.probe.callback("health").await
        }
    }
    struct ProbeHandler(Arc<Probe>);
    #[async_trait]
    impl MessageHandler for ProbeHandler {
        async fn handle(&self, _message: InboundMessage) -> Result<OutboundMessage> {
            Err(channel_error("unused handle".into()))
        }
        async fn reply(&self, _message: OutboundMessage) -> Result<()> {
            Ok(())
        }
        async fn close(&self) -> Result<()> {
            self.0.closes.fetch_add(1, Ordering::AcqRel);
            self.0.callback("close").await
        }
    }

    async fn registered(
        manager: &Arc<Mutex<ChannelManager>>,
        id: &'static str,
        probe: Arc<Probe>,
    ) -> Result<ChannelHandle> {
        let mut guard = manager.lock().await;
        guard.register(Box::new(ProbePlugin { id, probe }))?;
        guard
            .get_owned(id)
            .ok_or_else(|| channel_error("registration missing".into()))
    }

    #[tokio::test]
    async fn owned_callbacks_reenter_manager_metadata_on_every_async_path() -> Result<()> {
        let manager = Arc::new(Mutex::new(ChannelManager::new()));
        let probe = Probe::new(&manager);
        let channel = registered(&manager, "reentrant", Arc::clone(&probe)).await?;
        let start = {
            let guard = manager.lock().await;
            let probe = Arc::clone(&probe);
            guard.start_all_owned(move |_| Arc::new(ProbeHandler(Arc::clone(&probe))))
        };
        let sequence = async {
            assert!(
                start
                    .await
                    .into_iter()
                    .all(|row| row.channel_id == "reentrant" && row.result.is_ok())
            );
            channel.health_check().await?;
            channel
                .send(OutboundMessage::new(
                    "reentrant",
                    "chat",
                    ChatType::Direct,
                    "你好🌍",
                ))
                .await?;
            let stop = manager.lock().await.stop_owned("reentrant");
            stop.await
        };
        tokio::time::timeout(Duration::from_secs(2), sequence)
            .await
            .map_err(|_| channel_error("callback could not reenter manager metadata".into()))??;
        assert_eq!(
            *probe.calls.lock().await,
            vec!["start", "health", "send", "stop", "close"]
        );
        assert_eq!(probe.stops.load(Ordering::Acquire), 1);
        assert_eq!(probe.closes.load(Ordering::Acquire), 1);
        assert_eq!(manager.lock().await.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn owned_batch_snapshots_synchronously_and_unpolled_drop_has_no_side_effects()
    -> Result<()> {
        let manager = Arc::new(Mutex::new(ChannelManager::new()));
        let a = Probe::new(&manager);
        let b = Probe::new(&manager);
        registered(&manager, "a", Arc::clone(&a)).await?;
        let factories = Arc::new(AtomicUsize::new(0));
        let make_batch = || {
            let a = Arc::clone(&a);
            let b = Arc::clone(&b);
            let factories = Arc::clone(&factories);
            move |id: &str| -> Arc<dyn MessageHandler> {
                factories.fetch_add(1, Ordering::AcqRel);
                Arc::new(ProbeHandler(Arc::clone(if id == "a" { &a } else { &b })))
            }
        };
        let dropped = manager.lock().await.start_all_owned(make_batch());
        drop(dropped);
        assert_eq!(factories.load(Ordering::Acquire), 0);
        assert_eq!(a.starts.load(Ordering::Acquire), 0);
        let first = manager.lock().await.start_all_owned(make_batch());
        registered(&manager, "b", Arc::clone(&b)).await?;
        let rows = first.await;
        assert_eq!(rows.len(), 1);
        assert!(
            rows.into_iter()
                .all(|row| row.channel_id == "a" && row.result.is_ok())
        );
        assert_eq!(b.starts.load(Ordering::Acquire), 0);
        let next = manager.lock().await.start_all_owned(make_batch());
        let rows = next.await;
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .any(|row| row.channel_id == "a" && row.result.is_err())
        );
        assert!(
            rows.iter()
                .any(|row| row.channel_id == "b" && row.result.is_ok())
        );
        assert_eq!(factories.load(Ordering::Acquire), 2);
        let shutdown = manager.lock().await.stop_all_owned();
        shutdown.await?;
        Ok(())
    }

    #[tokio::test]
    async fn owned_stop_batch_excludes_later_registration_and_drop_does_not_stop() -> Result<()> {
        let manager = Arc::new(Mutex::new(ChannelManager::new()));
        let a = Probe::new(&manager);
        let b = Probe::new(&manager);
        registered(&manager, "a", Arc::clone(&a)).await?;
        let unpolled = manager.lock().await.stop_all_owned();
        drop(unpolled);
        assert_eq!(a.stops.load(Ordering::Acquire), 0);
        let first = manager.lock().await.stop_all_owned();
        registered(&manager, "b", Arc::clone(&b)).await?;
        first.await?;
        assert_eq!(a.stops.load(Ordering::Acquire), 1);
        assert_eq!(b.stops.load(Ordering::Acquire), 0);
        let next = manager.lock().await.stop_all_owned();
        next.await?;
        assert_eq!(a.stops.load(Ordering::Acquire), 1);
        assert_eq!(b.stops.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[tokio::test]
    async fn parked_channel_does_not_block_other_entry_or_registry_and_stop_serializes()
    -> Result<()> {
        let manager = Arc::new(Mutex::new(ChannelManager::new()));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let mut a = Probe::new(&manager);
        Arc::get_mut(&mut a)
            .ok_or_else(|| channel_error("probe unexpectedly shared".into()))?
            .start_gate = Some((Arc::clone(&entered), Arc::clone(&release)));
        let a_handle = registered(&manager, "a", Arc::clone(&a)).await?;
        let b = Probe::new(&manager);
        let b_handle = registered(&manager, "b", Arc::clone(&b)).await?;
        let task = tokio::spawn(a_handle.start(Arc::new(ProbeHandler(Arc::clone(&a)))));
        entered.notified().await;
        assert!(a_handle.entry.state.try_write().is_err());
        let stop = tokio::spawn(a_handle.stop());
        tokio::time::timeout(Duration::from_secs(2), b_handle.health_check())
            .await
            .map_err(|_| channel_error("other entry blocked by parked start".into()))??;
        assert_eq!(a.stops.load(Ordering::Acquire), 0);
        assert_eq!(manager.lock().await.channel_ids().len(), 2);
        release.notify_one();
        task.await
            .map_err(|error| channel_error(error.to_string()))??;
        stop.await
            .map_err(|error| channel_error(error.to_string()))??;
        assert_eq!(a.stops.load(Ordering::Acquire), 1);
        assert_eq!(a.closes.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[tokio::test]
    async fn failed_start_and_rollback_retain_original_handler_until_retry() -> Result<()> {
        let manager = Arc::new(Mutex::new(ChannelManager::new()));
        let mut probe = Probe::new(&manager);
        let state = Arc::get_mut(&mut probe).ok_or_else(|| channel_error("probe shared".into()))?;
        state.fail_start = true;
        state.fail_first_stop = true;
        let channel = registered(&manager, "rollback", Arc::clone(&probe)).await?;
        assert!(
            channel
                .start(Arc::new(ProbeHandler(Arc::clone(&probe))))
                .await
                .is_err()
        );
        assert_eq!(probe.closes.load(Ordering::Acquire), 0);
        assert!(
            channel
                .start(Arc::new(ProbeHandler(Arc::clone(&probe))))
                .await
                .is_err()
        );
        assert_eq!(probe.starts.load(Ordering::Acquire), 1);
        channel.stop().await?;
        assert_eq!(probe.stops.load(Ordering::Acquire), 2);
        assert_eq!(probe.closes.load(Ordering::Acquire), 1);
        assert!(!channel.entry.unsettled_handler.load(Ordering::Acquire));
        Ok(())
    }

    #[tokio::test]
    async fn owned_handle_survives_manager_and_mutable_borrow_invalidates_prior_stop() -> Result<()>
    {
        let mut manager = ChannelManager::new();
        let outside = Arc::new(Mutex::new(ChannelManager::new()));
        let probe = Probe::new(&outside);
        manager.register(Box::new(ProbePlugin {
            id: "retained",
            probe: Arc::clone(&probe),
        }))?;
        let channel = manager
            .get_owned("retained")
            .ok_or_else(|| channel_error("missing retained channel".into()))?;
        // No actual registry is retained by the probe, so callbacks do not reenter an empty manager.
        drop(outside);
        channel
            .start(Arc::new(ProbeHandler(Arc::clone(&probe))))
            .await?;
        channel.stop().await?;
        channel.stop().await?;
        assert_eq!(probe.stops.load(Ordering::Acquire), 1);
        assert!(manager.get_mut("retained").is_some());
        channel.stop().await?;
        assert_eq!(probe.stops.load(Ordering::Acquire), 2);
        let health = channel.health_check();
        drop(manager);
        health.await?;
        channel
            .start(Arc::new(ProbeHandler(Arc::clone(&probe))))
            .await?;
        channel.stop().await?;
        assert_eq!(probe.starts.load(Ordering::Acquire), 2);
        assert_eq!(probe.closes.load(Ordering::Acquire), 2);
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_mutable_borrow_cannot_be_cleared_by_inflight_stop_and_epoch_saturates()
    -> Result<()> {
        let manager = Arc::new(Mutex::new(ChannelManager::new()));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let mut probe = Probe::new(&manager);
        Arc::get_mut(&mut probe)
            .ok_or_else(|| channel_error("probe shared".into()))?
            .stop_gate = Some((Arc::clone(&entered), Arc::clone(&release)));
        let channel = registered(&manager, "epoch", Arc::clone(&probe)).await?;
        let first = tokio::spawn(channel.stop());
        entered.notified().await;
        assert!(manager.lock().await.get_mut("epoch").is_some());
        release.notify_one();
        first
            .await
            .map_err(|error| channel_error(error.to_string()))??;
        let second = tokio::spawn(channel.stop());
        entered.notified().await;
        release.notify_one();
        second
            .await
            .map_err(|error| channel_error(error.to_string()))??;
        assert_eq!(probe.stops.load(Ordering::Acquire), 2);
        channel
            .entry
            .mutation_epoch
            .store(u64::MAX, Ordering::Release);
        assert!(manager.lock().await.get_mut("epoch").is_some());
        assert_eq!(
            channel.entry.mutation_epoch.load(Ordering::Acquire),
            u64::MAX
        );
        for _ in 0..2 {
            let task = tokio::spawn(channel.stop());
            entered.notified().await;
            release.notify_one();
            task.await
                .map_err(|error| channel_error(error.to_string()))??;
        }
        assert_eq!(probe.stops.load(Ordering::Acquire), 4);
        Ok(())
    }

    #[tokio::test]
    async fn registration_metadata_is_fixed_while_async_calls_reach_original_plugin() -> Result<()>
    {
        struct DynamicDeclaration {
            started: bool,
        }
        #[async_trait]
        impl ChannelPlugin for DynamicDeclaration {
            fn id(&self) -> &str {
                "declared"
            }
            fn label(&self) -> &str {
                if self.started {
                    "changed"
                } else {
                    "registered"
                }
            }
            fn capabilities(&self) -> &ChannelCapabilities {
                static CHANGED: ChannelCapabilities = ChannelCapabilities {
                    chat_types: &[ChatType::Group],
                    supports_media: true,
                    supports_threads: true,
                };
                if self.started { &CHANGED } else { &CAPS }
            }
            async fn start(&mut self, _handler: Arc<dyn MessageHandler>) -> Result<()> {
                self.started = true;
                Ok(())
            }
            async fn stop(&mut self) -> Result<()> {
                Ok(())
            }
            async fn send(&self, _message: OutboundMessage) -> Result<()> {
                if self.started {
                    Ok(())
                } else {
                    Err(channel_error("original instance not started".into()))
                }
            }
        }
        let outside = Arc::new(Mutex::new(ChannelManager::new()));
        let probe = Probe::new(&outside);
        drop(outside);
        let mut manager = ChannelManager::new();
        manager.register(Box::new(DynamicDeclaration { started: false }))?;
        let channel = manager
            .get_owned("declared")
            .ok_or_else(|| channel_error("missing declaration".into()))?;
        let borrowed = manager
            .get_mut("declared")
            .ok_or_else(|| channel_error("missing mutable declaration".into()))?;
        borrowed.start(Arc::new(ProbeHandler(probe))).await?;
        assert_eq!(borrowed.label(), "registered");
        assert_eq!(borrowed.capabilities().chat_types, &[ChatType::Direct]);
        assert!(!borrowed.capabilities().supports_media);
        assert_eq!(channel.label(), "registered");
        channel
            .send(OutboundMessage::new(
                "declared",
                "chat",
                ChatType::Direct,
                "original",
            ))
            .await?;
        channel.stop().await?;
        Ok(())
    }
}
