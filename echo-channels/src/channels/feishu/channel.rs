//! Feishu ChannelPlugin 实现

use super::api::TokenManager;
use super::webhook;
use super::api::send_feishu_message;
use crate::types::*;
use async_trait::async_trait;
use echo_core::error::ChannelError;
use echo_core::error::ReactError;
use echo_core::error::Result;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

// ── Config ────────────────────────────────────────────────────────────────────

/// 飞书通道配置
#[derive(Debug, Clone)]
pub struct FeishuConfig {
    /// 飞书 App ID
    pub app_id: String,
    /// 飞书 App Secret
    pub app_secret: String,
    /// Webhook 监听地址（如 "0.0.0.0:8080"）
    pub webhook_bind: String,
    /// Webhook 路径（默认 "/webhook"）
    pub webhook_path: String,
    /// 可选：飞书 verification_token（用于验证事件来源）
    pub verification_token: Option<String>,
}

// ── Channel ───────────────────────────────────────────────────────────────────

/// 飞书 IM 通道实现
pub struct FeishuChannel {
    config: FeishuConfig,
    token_manager: Option<Arc<TokenManager>>,
    send_tx: Option<mpsc::Sender<OutboundMessage>>,
    webhook_handle: Option<JoinHandle<()>>,
}

impl FeishuChannel {
    pub fn new(config: FeishuConfig) -> Result<Self> {
        if config.app_id.is_empty() || config.app_secret.is_empty() {
            return Err(ReactError::Channel(ChannelError::InvalidConfig(
                "Feishu requires app_id and app_secret".to_string(),
            )));
        }

        Ok(Self {
            config,
            token_manager: None,
            send_tx: None,
            webhook_handle: None,
        })
    }

    fn webhook_path(&self) -> &str {
        if self.config.webhook_path.is_empty() {
            "/webhook"
        } else {
            &self.config.webhook_path
        }
    }
}

#[async_trait]
impl ChannelPlugin for FeishuChannel {
    fn id(&self) -> &str {
        "feishu"
    }

    fn label(&self) -> &str {
        "Feishu"
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
        info!("Starting Feishu channel...");

        // 1. 初始化 Token 管理器
        let token_manager = Arc::new(TokenManager::new(
            self.config.app_id.clone(),
            self.config.app_secret.clone(),
        ));
        self.token_manager = Some(token_manager.clone());

        // 2. 获取 token 验证配置
        token_manager.get_token().await?;

        // 3. 启动后台消息发送 task
        let (send_tx, mut send_rx) = mpsc::channel::<OutboundMessage>(256);
        let token_manager_clone = token_manager.clone();

        tokio::spawn(async move {
            loop {
                if let Some(msg) = send_rx.recv().await {
                    let token = match token_manager_clone.get_token().await {
                        Ok(t) => t,
                        Err(e) => {
                            warn!("Feishu: failed to get token for sending: {:?}", e);
                            continue;
                        }
                    };
                    if let Err(e) = send_feishu_message(
                        &token,
                        &msg.to,
                        &msg.chat_type,
                        &msg.text,
                    )
                    .await
                    {
                        warn!("Feishu: failed to send message: {:?}", e);
                    }
                }
            }
        });

        // 4. 创建 wrapper handler 用于 webhook —— 处理后自动调用 send
        let send_tx_clone = send_tx.clone();
        self.send_tx = Some(send_tx);

        let wrapper_handler = Arc::new(FeishuMessageHandler {
            inner: handler,
            send_tx: send_tx_clone,
        });

        // 5. 启动 Webhook 服务器
        let bind_addr = self.config.webhook_bind.clone();
        let webhook_path = self.webhook_path().to_string();
        let verification_token = self.config.verification_token.clone();

        let webhook_handle = tokio::spawn(async move {
            if let Err(e) = webhook::run_webhook_server(
                bind_addr,
                webhook_path,
                wrapper_handler,
                verification_token,
            )
            .await
            {
                warn!("Feishu webhook server error: {:?}", e);
            }
        });

        self.webhook_handle = Some(webhook_handle);

        info!("Feishu channel started (webhook on {})", self.config.webhook_bind);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Stopping Feishu channel...");

        if let Some(handle) = self.webhook_handle.take() {
            handle.abort();
        }

        self.send_tx = None;
        info!("Feishu channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if let Some(tx) = &self.send_tx {
            tx.send(msg).await.map_err(|e| {
                ReactError::Channel(ChannelError::SendError(format!(
                    "Failed to queue message for sending: {}",
                    e
                )))
            })
        } else {
            Err(ReactError::Channel(ChannelError::SendError(
                "Feishu channel not started".to_string(),
            )))
        }
    }
}

/// Wrapper: 先调用 inner handler 处理消息，再将结果通过 send_tx 发送
struct FeishuMessageHandler {
    inner: Arc<dyn MessageHandler>,
    send_tx: mpsc::Sender<OutboundMessage>,
}

#[async_trait]
impl MessageHandler for FeishuMessageHandler {
    async fn handle(&self, msg: InboundMessage) -> Result<OutboundMessage> {
        self.inner.handle(msg).await
    }

    async fn reply(&self, msg: OutboundMessage) -> Result<()> {
        self.send_tx.send(msg).await.map_err(|e| {
            ReactError::Channel(ChannelError::SendError(format!(
                "Failed to send reply: {}",
                e
            )))
        })
    }
}
