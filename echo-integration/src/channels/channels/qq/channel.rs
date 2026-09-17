//! QQ Bot ChannelPlugin implementation

use super::super::super::types::*;
use super::super::{DeliveryRequest, DeliverySender};
use super::api::*;
use async_trait::async_trait;
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
        let gateway_handle = tokio::spawn(async move {
            let mut reconnect_delay: u64 = 1;
            const MAX_DELAY: u64 = 60;
            // If connection exceeds this threshold it is considered stable; reset backoff delay
            const STABLE_THRESHOLD_SECS: u64 = 60;

            loop {
                let token = match token_manager_clone2.get_token().await {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(
                            "QQ Gateway: failed to get token: {}",
                            crate::redaction::text(&e.to_string())
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                let ws_url = match get_gateway_url(&http_for_gw, &token).await {
                    Ok(u) => u,
                    Err(e) => {
                        warn!(
                            "QQ Gateway: failed to get gateway URL: {}",
                            crate::redaction::text(&e.to_string())
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                info!("QQ Gateway: connecting...");
                let connected_at = std::time::Instant::now();

                match super::gateway::connect_to_gateway(ws_url, wrapper.clone(), token.clone())
                    .await
                {
                    Ok(()) => {
                        warn!(
                            "QQ Gateway: connection closed, reconnecting in {}s...",
                            reconnect_delay
                        );
                    }
                    Err(e) => {
                        warn!(
                            "QQ Gateway: connection error: {:?}, reconnecting in {}s...",
                            crate::redaction::text(&e.to_string()),
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

                tokio::time::sleep(std::time::Duration::from_secs(reconnect_delay)).await;
            }
        });

        self.gateway_handle = Some(gateway_handle);

        info!("QQ Bot channel started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Stopping QQ Bot channel...");

        if let Some(handle) = self.gateway_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        self.send_tx = None;
        if let Some(handle) = self.send_handle.take() {
            handle.abort();
            let _ = handle.await;
        }
        self.token_manager = None;
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
