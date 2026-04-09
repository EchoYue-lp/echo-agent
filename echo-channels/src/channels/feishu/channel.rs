//! Feishu Channel Implementation
//!
//! 支持两种模式：
//! - Long Poll（WebSocket 长连接）：无需公网 IP，纯 Rust 实现
//! - Webhook：需要公网 IP，HTTP 事件推送
//!
//! 消息流程：
//! 1. 用户发送消息 → bot 添加 "OK" 表情反应
//! 2. Bot 回复：发送卡片消息
//! 3. Agent 处理消息并返回结果
//! 4. Bot 更新卡片内容
//! 5. Bot 添加 "DONE" 表情反应

use super::api::{http_client, TokenManager, send_card_message, reply_message, patch_card_message, add_reaction, FEISHU_API_BASE, LARK_API_BASE, FEISHU_WS_BASE, LARK_WS_BASE};
use super::long_poll::WsClient;
use super::webhook;
use crate::types::*;
use async_trait::async_trait;
use echo_core::error::{ChannelError, ReactError, Result};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

// ── Config ────────────────────────────────────────────────────────────────────

/// 飞书通道模式
#[derive(Debug, Clone, Default)]
pub enum FeishuMode {
    /// Webhook 模式：需要公网 IP
    Webhook {
        bind: String,
        path: String,
        verification_token: Option<String>,
    },
    /// 长连接模式：无需公网 IP（默认，推荐）
    #[default]
    LongPoll,
}

/// 飞书通道配置
#[derive(Debug, Clone)]
pub struct FeishuConfig {
    /// 飞书 App ID
    pub app_id: String,
    /// 飞书 App Secret
    pub app_secret: String,
    /// API 基础地址（用于发送消息，带 /open-apis）
    pub api_domain: String,
    /// WebSocket 基础地址（用于获取 endpoint，不带 /open-apis）
    pub ws_domain: String,
    /// 连接模式（默认长连接）
    pub mode: FeishuMode,
}

impl FeishuConfig {
    /// 创建长连接模式配置（国内版飞书）
    pub fn new_long_poll(app_id: String, app_secret: String) -> Self {
        Self {
            app_id,
            app_secret,
            api_domain: FEISHU_API_BASE.to_string(),
            ws_domain: FEISHU_WS_BASE.to_string(),
            mode: FeishuMode::LongPoll,
        }
    }

    /// 创建长连接模式配置（国际版 Lark）
    pub fn new_long_poll_lark(app_id: String, app_secret: String) -> Self {
        Self {
            app_id,
            app_secret,
            api_domain: LARK_API_BASE.to_string(),
            ws_domain: LARK_WS_BASE.to_string(),
            mode: FeishuMode::LongPoll,
        }
    }

    /// 创建 Webhook 模式配置
    pub fn new_webhook(
        app_id: String,
        app_secret: String,
        bind: String,
        path: String,
        verification_token: Option<String>,
    ) -> Self {
        Self {
            app_id,
            app_secret,
            api_domain: FEISHU_API_BASE.to_string(),
            ws_domain: FEISHU_WS_BASE.to_string(),
            mode: FeishuMode::Webhook {
                bind,
                path,
                verification_token,
            },
        }
    }

    /// 自定义 domain
    pub fn with_domain(mut self, api_domain: String, ws_domain: String) -> Self {
        self.api_domain = api_domain;
        self.ws_domain = ws_domain;
        self
    }
}

// ── Channel ───────────────────────────────────────────────────────────────────

/// 飞书 IM 通道实现
pub struct FeishuChannel {
    config: FeishuConfig,
    token_manager: Option<Arc<TokenManager>>,
    http: reqwest::Client,
    send_tx: Option<mpsc::Sender<OutboundMessage>>,
    task_handle: Option<JoinHandle<()>>,
    /// 消息 ID -> (卡片消息 ID, 创建时间)（用于更新运行中的卡片，带 TTL 防泄漏）
    running_cards: Arc<dashmap::DashMap<String, (String, std::time::Instant)>>,
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
            http: http_client(),
            send_tx: None,
            task_handle: None,
            running_cards: Arc::new(dashmap::DashMap::new()),
        })
    }

    /// 构建卡片内容（Markdown 格式）
    fn build_card_content(text: &str) -> String {
        let card = serde_json::json!({
            "config": {
                "wide_screen_mode": true,
                "update_multi": true
            },
            "elements": [
                {
                    "tag": "markdown",
                    "content": text
                }
            ]
        });
        card.to_string()
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
            supports_threads: true,
        })
    }

    async fn start(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()> {
        info!("Starting Feishu channel...");

        // 1. 初始化 Token 管理器
        let token_manager = Arc::new(TokenManager::new(
            self.config.app_id.clone(),
            self.config.app_secret.clone(),
            self.config.api_domain.clone(),
        ));
        self.token_manager = Some(token_manager.clone());

        // 验证配置（获取 token）
        token_manager.get_token().await?;

        // 2. 启动消息发送任务
        let (send_tx, mut send_rx) = mpsc::channel::<OutboundMessage>(256);
        self.send_tx = Some(send_tx.clone());

        let http = self.http.clone();
        let api_domain = self.config.api_domain.clone();
        let token_mgr = token_manager.clone();
        let running_cards = self.running_cards.clone();

        tokio::spawn(async move {
            while let Some(msg) = send_rx.recv().await {
                let token = match token_mgr.get_token().await {
                    Ok(t) => t,
                    Err(e) => {
                        warn!("Feishu: failed to get token: {:?}", e);
                        continue;
                    }
                };

                if let Err(e) = send_feishu_message_internal(
                    &http,
                    &api_domain,
                    &token,
                    msg,
                    running_cards.clone(),
                ).await {
                    warn!("Feishu: failed to send message: {:?}", e);
                }
            }
        });

        // 3. 创建 wrapper handler
        let wrapper_handler = Arc::new(FeishuMessageHandler {
            inner: handler,
            send_tx: send_tx.clone(),
            http: self.http.clone(),
            api_domain: self.config.api_domain.clone(),
            token_manager: token_manager.clone(),
            running_cards: self.running_cards.clone(),
        });

        // 4. 根据模式启动不同的消息接收服务
        match &self.config.mode {
            FeishuMode::LongPoll => {
                let config = super::long_poll::WsClientConfig::new(
                    self.config.app_id.clone(),
                    self.config.app_secret.clone(),
                    self.config.ws_domain.clone(),
                );

                let task_handle = tokio::spawn(async move {
                    let mut client = WsClient::new(config);
                    // WsClient::run() 内部已有完整的重连逻辑，无需外层 loop
                    if let Err(e) = client.run(wrapper_handler.clone()).await {
                        warn!("Feishu WebSocket: fatal error: {:?}", e);
                    }
                });
                self.task_handle = Some(task_handle);
                info!("Feishu channel started (long poll mode)");
            }
            FeishuMode::Webhook { bind, path, verification_token } => {
                let bind_addr = bind.clone();
                let webhook_path = path.clone();
                let token = verification_token.clone();

                let task_handle = tokio::spawn(async move {
                    if let Err(e) = webhook::run_webhook_server(
                        bind_addr,
                        webhook_path,
                        wrapper_handler,
                        token,
                    ).await {
                        warn!("Feishu webhook server error: {:?}", e);
                    }
                });
                self.task_handle = Some(task_handle);
                info!("Feishu channel started (webhook on {})", bind);
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Stopping Feishu channel...");

        if let Some(handle) = self.task_handle.take() {
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
                    "Failed to queue message: {}",
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

/// 内部发送消息函数
async fn send_feishu_message_internal(
    http: &reqwest::Client,
    api_domain: &str,
    token: &str,
    msg: OutboundMessage,
    running_cards: Arc<dashmap::DashMap<String, (String, std::time::Instant)>>,
) -> Result<()> {
    let receive_id_type = match msg.chat_type {
        ChatType::Direct => "open_id",
        ChatType::Group => "chat_id",
    };

    let card_content = FeishuChannel::build_card_content(&msg.text);

    // 清理超过 1 小时的过期卡片缓存
    let card_ttl = std::time::Duration::from_secs(3600);
    running_cards.retain(|_, (_, created)| created.elapsed() < card_ttl);

    // 检查是否有回复目标（thread_ts）
    if let Some(ref reply_to) = msg.reply_to {
        // 检查是否有运行中的卡片
        if let Some(entry) = running_cards.get(reply_to) {
            let (card_id, _) = entry.value();
            patch_card_message(http, api_domain, token, card_id, &card_content).await?;
            debug!("Feishu: updated card {} for reply_to {}", card_id, reply_to);
        } else {
            // 发送新的卡片回复
            let card_msg_id = reply_message(http, api_domain, token, reply_to, "interactive", &card_content).await?;
            running_cards.insert(reply_to.clone(), (card_msg_id, std::time::Instant::now()));
            debug!("Feishu: sent card reply to {}", reply_to);
        }
    } else {
        // 发送新消息
        send_card_message(http, api_domain, token, &msg.to, receive_id_type, &card_content).await?;
        debug!("Feishu: sent new card message to {}", msg.to);
    }

    Ok(())
}

/// Wrapper Handler：处理消息并自动发送回复
struct FeishuMessageHandler {
    inner: Arc<dyn MessageHandler>,
    send_tx: mpsc::Sender<OutboundMessage>,
    http: reqwest::Client,
    api_domain: String,
    token_manager: Arc<TokenManager>,
    #[allow(dead_code)]
    running_cards: Arc<dashmap::DashMap<String, (String, std::time::Instant)>>,
}

#[async_trait]
impl MessageHandler for FeishuMessageHandler {
    async fn handle(&self, msg: InboundMessage) -> Result<OutboundMessage> {
        // 先添加 OK 表情反应
        let token = self.token_manager.get_token().await?;
        if let Err(e) = add_reaction(&self.http, &self.api_domain, &token, &msg.message_id, "OK").await {
            warn!("Feishu: failed to add OK reaction: {:?}", e);
        }

        // 调用 inner handler
        self.inner.handle(msg).await
    }

    async fn reply(&self, msg: OutboundMessage) -> Result<()> {
        // 发送消息前，先发送一个 "Working on it..." 卡片（如果不是最终回复）
        // 这里简化处理，直接发送

        // 通过 send_tx 发送（会被后台任务处理）
        self.send_tx.send(msg).await.map_err(|e| {
            ReactError::Channel(ChannelError::SendError(format!(
                "Failed to send reply: {}",
                e
            )))
        })
    }
}