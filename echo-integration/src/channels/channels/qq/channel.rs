//! QQ Bot ChannelPlugin implementation

use super::super::super::types::*;
use super::super::{DeliveryRequest, DeliverySender};
use super::api::*;
use async_trait::async_trait;
use echo_core::agent::CancellationToken;
use echo_core::error::ChannelError;
use echo_core::error::ReactError;
use echo_core::error::Result;
use std::fmt;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

// ── Config ────────────────────────────────────────────────────────────────────

/// QQ Bot channel configuration
#[derive(Clone)]
pub struct QqConfig {
    /// QQ Bot App ID
    pub app_id: String,
    /// QQ Bot Client Secret
    pub client_secret: String,
}

impl fmt::Debug for QqConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QqConfig")
            .field("app_id", &self.app_id)
            .field("client_secret", &crate::redaction::REDACTED)
            .finish()
    }
}

impl QqConfig {
    pub fn new(app_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            client_secret: client_secret.into(),
        }
    }
}

// ── Channel ───────────────────────────────────────────────────────────────────

/// QQ Bot IM channel implementation
pub struct QqChannel {
    config: QqConfig,
    /// Shared HTTP client (connection pool reuse)
    http: reqwest::Client,
    token_manager: Option<Arc<TokenManager>>,
    send_tx: Option<DeliverySender>,
    send_handle: Option<JoinHandle<()>>,
    gateway_handle: Option<JoinHandle<()>>,
    gateway_cancel: CancellationToken,
    transport: TransportStatus,
    shutdown_failed: bool,
}

impl QqChannel {
    pub fn new(config: QqConfig) -> Result<Self> {
        if config.app_id.is_empty() || config.client_secret.is_empty() {
            return Err(ReactError::Channel(Box::new(ChannelError::InvalidConfig(
                "QQ Bot requires app_id and client_secret".to_string(),
            ))));
        }

        Ok(Self {
            config,
            http: reqwest_client(),
            token_manager: None,
            send_tx: None,
            send_handle: None,
            gateway_handle: None,
            gateway_cancel: CancellationToken::new(),
            transport: TransportStatus::new(ChannelTransportPhase::Stopped),
            shutdown_failed: false,
        })
    }
}

#[async_trait]
impl ChannelPlugin for QqChannel {
    fn id(&self) -> &str {
        "qqbot"
    }

    fn label(&self) -> &str {
        "QQ Bot"
    }

    fn capabilities(&self) -> &ChannelCapabilities {
        static CAPS: std::sync::OnceLock<ChannelCapabilities> = std::sync::OnceLock::new();
        CAPS.get_or_init(|| ChannelCapabilities {
            chat_types: &[ChatType::Direct, ChatType::Group],
            supports_media: false,
            supports_threads: false,
        })
    }

    async fn start(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()> {
        if self.gateway_handle.is_some() || self.send_handle.is_some() {
            return Err(ReactError::Channel(Box::new(ChannelError::Other(
                "QQ Bot channel is already started".to_string(),
            ))));
        }
        self.gateway_cancel = CancellationToken::new();
        self.transport = TransportStatus::new(ChannelTransportPhase::Connecting);
        info!("Starting QQ Bot channel...");

        // 1. Initialize token manager
        let token_manager = Arc::new(TokenManager::new(
            self.config.app_id.clone(),
            self.config.client_secret.clone(),
        ));
        self.token_manager = Some(token_manager.clone());

        // 2. Start background message sending task
        let (send_tx, mut send_rx) = mpsc::channel::<DeliveryRequest>(256);
        let token_manager_clone = token_manager.clone();

        // 3. Create wrapper handler — calls inner handler then auto-sends reply
        let send_tx_clone = send_tx.clone();
        self.send_tx = Some(send_tx);
        let wrapper = Arc::new(QqMessageHandler {
            inner: handler,
            send_tx: send_tx_clone,
        });

        // 4. Start message sending task
        let http_for_send = self.http.clone();
        let token_manager_clone2 = token_manager.clone();
        let send_handle = tokio::spawn(async move {
            while let Some(request) = send_rx.recv().await {
                let DeliveryRequest {
                    message,
                    receipt,
                    delivery_permit,
                } = request;
                let result = async {
                    let token = token_manager_clone.get_token().await?;
                    send_qq_message(
                        &http_for_send,
                        &token,
                        &message.to,
                        &message.chat_type,
                        &message.text,
                        message.reply_to.as_deref(),
                    )
                    .await
                }
                .await;
                drop(delivery_permit);
                let _ = receipt.send(result);
            }
        });
        self.send_handle = Some(send_handle);

        // 5. Start Gateway connection loop (with exponential backoff reconnect)
        let http_for_gw = self.http.clone();
        let cancellation = self.gateway_cancel.clone();
        let transport = self.transport.clone();
        let client_secret = self.config.client_secret.clone();
        let gateway_handle = tokio::spawn(async move {
            let mut observation = transport.task_guard();
            let mut reconnect_delay: u64 = 1;
            const MAX_DELAY: u64 = 60;
            // If connection exceeds this threshold it is considered stable; reset backoff delay
            const STABLE_THRESHOLD_SECS: u64 = 60;

            loop {
                if cancellation.is_cancelled() {
                    break;
                }
                transport.publish(ChannelTransportPhase::Connecting, None, None);
                let token_result = tokio::select! {
                    biased;
                    () = cancellation.cancelled() => break,
                    result = token_manager_clone2.get_token() => result,
                };
                let token = match token_result {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(
                            "QQ Gateway: failed to get token: {}",
                            crate::redaction::text_with_secrets(
                                &e.to_string(),
                                [client_secret.as_str()]
                            )
                        );
                        transport.publish(
                            ChannelTransportPhase::Disconnected,
                            None,
                            Some(crate::redaction::text_with_secrets(
                                &e.to_string(),
                                [client_secret.as_str()],
                            )),
                        );
                        tokio::select! {
                            biased;
                            () = cancellation.cancelled() => break,
                            () = tokio::time::sleep(std::time::Duration::from_secs(5)) => {},
                        }
                        continue;
                    }
                };

                let url_result = tokio::select! {
                    biased;
                    () = cancellation.cancelled() => break,
                    result = get_gateway_url(&http_for_gw, &token) => result,
                };
                let ws_url = match url_result {
                    Ok(u) => u,
                    Err(e) => {
                        warn!(
                            "QQ Gateway: failed to get gateway URL: {}",
                            crate::redaction::text_with_secrets(
                                &e.to_string(),
                                [client_secret.as_str(), token.as_str()]
                            )
                        );
                        transport.publish(
                            ChannelTransportPhase::Disconnected,
                            None,
                            Some(crate::redaction::text_with_secrets(
                                &e.to_string(),
                                [client_secret.as_str(), token.as_str()],
                            )),
                        );
                        tokio::select! {
                            biased;
                            () = cancellation.cancelled() => break,
                            () = tokio::time::sleep(std::time::Duration::from_secs(5)) => {},
                        }
                        continue;
                    }
                };

                info!("QQ Gateway: connecting...");
                let connected_at = std::time::Instant::now();

                match super::gateway::connect_to_gateway(
                    ws_url,
                    wrapper.clone(),
                    token.clone(),
                    cancellation.clone(),
                    transport.clone(),
                )
                .await
                {
                    Ok(()) => {
                        if cancellation.is_cancelled() {
                            break;
                        }
                        transport.publish(ChannelTransportPhase::Disconnected, None, None);
                        warn!(
                            "QQ Gateway: connection closed, reconnecting in {}s...",
                            reconnect_delay
                        );
                    }
                    Err(e) => {
                        transport.publish(
                            ChannelTransportPhase::Disconnected,
                            None,
                            Some(crate::redaction::text_with_secrets(
                                &e.to_string(),
                                [client_secret.as_str(), token.as_str()],
                            )),
                        );
                        warn!(
                            "QQ Gateway: connection error: {:?}, reconnecting in {}s...",
                            crate::redaction::text_with_secrets(
                                &e.to_string(),
                                [client_secret.as_str(), token.as_str()]
                            ),
                            reconnect_delay
                        );
                    }
                }

                // If connection ran stably beyond threshold, reset backoff delay
                if connected_at.elapsed().as_secs() >= STABLE_THRESHOLD_SECS {
                    reconnect_delay = 1;
                } else {
                    reconnect_delay = (reconnect_delay * 2).min(MAX_DELAY);
                }

                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => break,
                    () = tokio::time::sleep(std::time::Duration::from_secs(reconnect_delay)) => {},
                }
            }
            observation.finish();
        });

        self.gateway_handle = Some(gateway_handle);

        info!("QQ Bot channel started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Stopping QQ Bot channel...");

        self.gateway_cancel.cancel();
        // Keep the original handles in the adapter across a cancelled stop
        // waiter. Its heartbeat is driven in the same original gateway task.
        if let Some(handle) = self.gateway_handle.as_mut() {
            self.shutdown_failed |= handle.await.is_err();
            self.gateway_handle = None;
        }
        self.send_tx = None;
        if let Some(handle) = self.send_handle.as_mut() {
            handle.abort();
            if let Err(error) = handle.await {
                self.shutdown_failed |= !error.is_cancelled();
            }
            self.send_handle = None;
        }
        self.token_manager = None;
        if std::mem::take(&mut self.shutdown_failed) {
            self.transport.publish(
                ChannelTransportPhase::Failed,
                None,
                Some("QQ transport task failed during shutdown".into()),
            );
            return Err(ReactError::Channel(Box::new(
                ChannelError::ConnectionError("QQ transport task failed during shutdown".into()),
            )));
        }
        self.transport
            .publish(ChannelTransportPhase::Stopped, None, None);
        info!("QQ Bot channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        let _delivery_permit = msg.begin_delivery()?;
        let token_manager = self.token_manager.as_ref().ok_or_else(|| {
            ReactError::Channel(Box::new(ChannelError::SendError(
                "QQ Bot channel not started".to_string(),
            )))
        })?;
        let token = token_manager.get_token().await?;
        send_qq_message(
            &self.http,
            &token,
            &msg.to,
            &msg.chat_type,
            &msg.text,
            msg.reply_to.as_deref(),
        )
        .await
    }

    fn transport_status(&self) -> Option<tokio::sync::watch::Receiver<ChannelTransportSnapshot>> {
        Some(self.transport.subscribe())
    }

    async fn health_check(&self) -> Result<()> {
        if self
            .gateway_handle
            .as_ref()
            .is_some_and(|h| h.is_finished())
        {
            return Err(ReactError::Channel(Box::new(
                ChannelError::ConnectionError("QQ Bot gateway task has terminated".to_string()),
            )));
        }
        if let Some(ref tm) = self.token_manager {
            tm.get_token().await?;
        }
        Ok(())
    }
}

/// Wrapper: calls inner handler then automatically sends reply via send_tx
struct QqMessageHandler {
    inner: Arc<dyn MessageHandler>,
    send_tx: DeliverySender,
}

#[async_trait]
impl MessageHandler for QqMessageHandler {
    async fn handle(&self, msg: InboundMessage) -> Result<OutboundMessage> {
        // 消费 inner 的流式分段,逐 chunk 经 send_tx 投递(真流式);返回空 text 占位
        // (gateway 会再调 reply,reply 对空 text no-op 防双发,见 super::reply_with_empty_guard)。
        super::super::dispatch_stream_to_send_tx(&self.inner, &self.send_tx, msg).await
    }

    async fn reply(&self, msg: OutboundMessage) -> Result<()> {
        super::super::reply_with_empty_guard(&self.send_tx, msg).await
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use crate::channels::types::ChannelDeliveryFence;

    #[tokio::test]
    async fn stopped_health_is_not_readiness_and_dropping_observer_has_no_side_effect() -> Result<()>
    {
        let mut channel = QqChannel::new(QqConfig::new("fixture-app", "fixture-secret"))?;
        let receiver = channel
            .transport_status()
            .ok_or_else(|| ReactError::Other("missing built-in observation".into()))?;
        assert_eq!(receiver.borrow().phase, ChannelTransportPhase::Stopped);
        drop(receiver);
        channel.health_check().await?;
        assert_eq!(
            channel
                .transport_status()
                .ok_or_else(|| ReactError::Other("missing observation".into()))?
                .borrow()
                .phase,
            ChannelTransportPhase::Stopped
        );
        channel.stop().await?;
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_stop_waiter_retains_the_original_transport_join()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut channel = QqChannel::new(QqConfig::new("fixture-app", "fixture-secret"))?;
        let release = Arc::new(tokio::sync::Notify::new());
        let task_release = release.clone();
        channel.gateway_handle = Some(tokio::spawn(async move {
            task_release.notified().await;
        }));
        let cancellation = channel.gateway_cancel.clone();
        let channel = Arc::new(tokio::sync::Mutex::new(channel));
        let stopping = channel.clone();
        let stop = tokio::spawn(async move { stopping.lock().await.stop().await });
        tokio::time::timeout(std::time::Duration::from_secs(2), cancellation.cancelled()).await?;
        stop.abort();
        let stopped_waiter = stop.await;
        assert!(stopped_waiter.is_err());
        assert!(channel.lock().await.gateway_handle.is_some());
        release.notify_one();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            channel.lock().await.stop(),
        )
        .await??;
        let locked = channel.lock().await;
        assert!(locked.gateway_handle.is_none());
        assert_eq!(
            locked.transport.subscribe().borrow().phase,
            ChannelTransportPhase::Stopped
        );
        Ok(())
    }

    #[tokio::test]
    async fn completed_abnormal_join_is_removed_and_stop_retry_does_not_repoll_it() -> Result<()> {
        let mut channel = QqChannel::new(QqConfig::new("fixture-app", "fixture-secret"))?;
        let task = tokio::spawn(std::future::pending::<()>());
        task.abort();
        channel.gateway_handle = Some(task);
        assert!(channel.stop().await.is_err());
        assert!(channel.gateway_handle.is_none());
        assert_eq!(
            channel.transport.subscribe().borrow().phase,
            ChannelTransportPhase::Failed
        );
        // This second physical stop has no old JoinHandle to poll again.
        channel.stop().await?;
        assert_eq!(
            channel.transport.subscribe().borrow().phase,
            ChannelTransportPhase::Stopped
        );
        Ok(())
    }

    #[test]
    fn debug_does_not_expose_client_secret() {
        let config = QqConfig::new("app-id", "qq-client-secret");
        let debug = format!("{config:?}");
        assert!(debug.contains("app-id"));
        assert!(
            !debug.contains("qq-client-secret"),
            "debug leaked client secret: {debug}"
        );
    }

    #[tokio::test]
    async fn direct_send_rejects_retired_generation_before_channel_state() -> Result<()> {
        let channel = QqChannel::new(QqConfig::new("app-id", "client-secret"))?;
        let fence = ChannelDeliveryFence::new("retired-qq".to_string());
        fence.retire();
        let result = channel
            .send(
                OutboundMessage::new("qqbot", "recipient", ChatType::Direct, "stale")
                    .with_delivery_fence(fence),
            )
            .await;
        if !matches!(
            result,
            Err(ReactError::Channel(ref error))
                if matches!(
                    error.as_ref(),
                    ChannelError::StaleDelivery { incarnation_id }
                        if incarnation_id == "retired-qq"
                )
        ) {
            return Err(ReactError::Other(
                "QQ direct send did not reject the retired generation first".to_string(),
            ));
        }
        Ok(())
    }
}
