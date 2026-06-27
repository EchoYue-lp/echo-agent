//! QQ Bot ChannelPlugin implementation

use super::super::super::types::*;
use super::api::*;
use async_trait::async_trait;
use echo_core::error::ChannelError;
use echo_core::error::ReactError;
use echo_core::error::Result;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

// ── Config ────────────────────────────────────────────────────────────────────

/// QQ Bot channel configuration
#[derive(Debug, Clone)]
pub struct QqConfig {
    /// QQ Bot App ID
    pub app_id: String,
    /// QQ Bot Client Secret
    pub client_secret: String,
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
    send_tx: Option<mpsc::Sender<OutboundMessage>>,
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
        info!("Starting QQ Bot channel...");

        // 1. Initialize token manager
        let token_manager = Arc::new(TokenManager::new(
            self.config.app_id.clone(),
            self.config.client_secret.clone(),
        ));
        self.token_manager = Some(token_manager.clone());

        // 2. Start background message sending task
        let (send_tx, mut send_rx) = mpsc::channel::<OutboundMessage>(256);
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
        let _send_task = tokio::spawn(async move {
            loop {
                if let Some(msg) = send_rx.recv().await {
                    let token = match token_manager_clone.get_token().await {
                        Ok(t) => t,
                        Err(e) => {
                            warn!("QQ Bot: failed to get token for sending: {:?}", e);
                            continue;
                        }
                    };
                    if let Err(e) = send_qq_message(
                        &http_for_send,
                        &token,
                        &msg.to,
                        &msg.chat_type,
                        &msg.text,
                        msg.reply_to.as_deref(),
                    )
                    .await
                    {
                        warn!("QQ Bot: failed to send message: {:?}", e);
                    }
                }
            }
        });

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
                        warn!("QQ Gateway: failed to get token: {:?}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                let ws_url = match get_gateway_url(&http_for_gw, &token).await {
                    Ok(u) => u,
                    Err(e) => {
                        warn!("QQ Gateway: failed to get gateway URL: {:?}", e);
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
                            e, reconnect_delay
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
        }

        self.send_tx = None;
        info!("QQ Bot channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if let Some(tx) = &self.send_tx {
            tx.send(msg).await.map_err(|e| {
                ReactError::Channel(Box::new(ChannelError::SendError(format!(
                    "Failed to queue message for sending: {}",
                    e
                ))))
            })
        } else {
            Err(ReactError::Channel(Box::new(ChannelError::SendError(
                "QQ Bot channel not started".to_string(),
            ))))
        }
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
    send_tx: mpsc::Sender<OutboundMessage>,
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
