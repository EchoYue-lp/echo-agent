//! QQ Bot HTTP API —— Token 获取 + 消息发送
//!
//! 官方 API 文档：
//! - Token: POST https://bots.qq.com/app/getAppAccessToken
//! - Gateway: GET https://api.sgroup.qq.com/v2/gateway
//! - 私聊: POST https://api.sgroup.qq.com/v2/users/{openid}/messages
//! - 群聊: POST https://api.sgroup.qq.com/v2/groups/{guild_id}/messages

use crate::types::ChatType;
use echo_core::error::{ChannelError, ReactError, Result};
use reqwest::Client;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

const QQ_API_BASE: &str = "https://api.sgroup.qq.com";
const QQ_TOKEN_URL: &str = "https://bots.qq.com/app/getAppAccessToken";

pub fn reqwest_client() -> Client {
    Client::builder().build().expect("Failed to create HTTP client")
}

// ── Token 管理 ────────────────────────────────────────────────────────────────

/// QQ Token 管理器 —— 获取并缓存 access_token
pub(super) struct TokenManager {
    app_id: String,
    client_secret: String,
    token: Arc<Mutex<Option<String>>>,
    expires_at: Arc<AtomicU64>,
    /// 防止并发重复刷新
    refresh_lock: Arc<Mutex<()>>,
    http: reqwest::Client,
}

impl TokenManager {
    pub fn new(app_id: String, client_secret: String) -> Self {
        Self {
            app_id,
            client_secret,
            token: Arc::new(Mutex::new(None)),
            expires_at: Arc::new(AtomicU64::new(0)),
            refresh_lock: Arc::new(Mutex::new(())),
            http: reqwest_client(),
        }
    }

    async fn is_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp() as u64;
        self.expires_at.load(Ordering::Relaxed) <= now
    }

    async fn get_cached(&self) -> Option<String> {
        if self.is_expired().await {
            return None;
        }
        self.token.lock().await.clone()
    }

    /// 获取 access_token（自动缓存 / 刷新）
    pub async fn get_token(&self) -> Result<String> {
        if let Some(token) = self.get_cached().await {
            return Ok(token);
        }
        // 加锁防止并发重复刷新
        let _lock = self.refresh_lock.lock().await;
        // 拿到锁后再检查一次
        if let Some(token) = self.get_cached().await {
            return Ok(token);
        }
        self.refresh_token().await
    }

    async fn refresh_token(&self) -> Result<String> {
        info!("QQ Bot: refreshing access_token");

        let body = json!({
            "appId": self.app_id,
            "clientSecret": self.client_secret,
        });

        let res = self
            .http
            .post(QQ_TOKEN_URL)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                ChannelError::NetworkError(format!("QQ token request failed: {}", e))
            })?;

        if !res.status().is_success() {
            return Err(ReactError::Channel(ChannelError::AuthError(format!(
                "QQ token request failed with status {}",
                res.status()
            ))));
        }

        let json: serde_json::Value = res.json().await.map_err(|e| {
            ChannelError::NetworkError(format!("QQ token response parse error: {}", e))
        })?;

        debug!("QQ Bot: token response = {:?}", json);

        // QQ API 返回字段可能是 access_token (snake_case) 或 accessToken (camelCase)
        let token = json.get("accessToken")
            .or_else(|| json.get("access_token"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ReactError::Channel(ChannelError::AuthError(
                format!("QQ token response missing accessToken, got: {:?}", json)
            )))?
            .to_string();

        // QQ Token 有效期通常 7200 秒，提前 5 分钟刷新
        // 注意：expires_in 可能是字符串 "7200" 或数字 7200
        let expires_in = json.get("expiresIn")
            .or_else(|| json.get("expires_in"))
            .and_then(|v| {
                v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            })
            .unwrap_or(7200);
        let refresh_at = (chrono::Utc::now().timestamp() as u64) + expires_in - 300;
        self.expires_at.store(refresh_at, Ordering::Relaxed);

        let mut tok = self.token.lock().await;
        *tok = Some(token.clone());

        info!("QQ Bot: token refreshed, expires in {}s", expires_in);
        Ok(token)
    }

    #[allow(dead_code)]
    pub(crate) fn auth_header(&self, token: &str) -> String {
        format!("QQBot {}", token)
    }
}

// ── Gateway URL ───────────────────────────────────────────────────────────────

/// 获取 WebSocket Gateway URL
pub async fn get_gateway_url(client: &reqwest::Client, token: &str) -> Result<String> {
    // QQ Bot API v2 gateway endpoint 尝试多个路径
    let endpoints = [
        format!("{}/gateway", QQ_API_BASE),       // 无版本号
        format!("{}/gateway/bot", QQ_API_BASE),   // bot suffix
        format!("{}/v2/gateway", QQ_API_BASE),    // v2 前缀
        format!("{}/v2/gateway/bot", QQ_API_BASE), // v2 + bot
    ];

    for endpoint in &endpoints {
        debug!("QQ Bot: trying gateway endpoint: {}", endpoint);

        let res = client
            .get(endpoint)
            .header("Authorization", format!("QQBot {}", token))
            .send()
            .await
            .map_err(|e| {
                ChannelError::NetworkError(format!("QQ gateway request failed: {}", e))
            })?;

        let status = res.status();
        if status.is_success() {
            let json: serde_json::Value = res.json().await.map_err(|e| {
                ChannelError::NetworkError(format!("QQ gateway response parse error: {}", e))
            })?;

            debug!("QQ Bot: gateway response = {:?}", json);

            let url = json.get("url")
                .or_else(|| json.get("gateway_url"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ReactError::Channel(ChannelError::NetworkError(
                        format!("QQ gateway response missing url, got: {:?}", json)
                    ))
                })?
                .to_string();

            info!("QQ Bot: gateway URL obtained from {}", endpoint);
            return Ok(url);
        }

        let error_body = res.text().await.unwrap_or_default();
        debug!("QQ Gateway: endpoint {} returned status {}, body: {}", endpoint, status, error_body);
    }

    Err(ReactError::Channel(ChannelError::ApiError {
        status: 404,
        message: "QQ gateway: all endpoints failed".to_string(),
    }))
}

// ── 发送消息 ─────────────────────────────────────────────────────────────────

/// 发送 QQ 消息
pub async fn send_qq_message(
    client: &reqwest::Client,
    token: &str,
    to: &str,
    chat_type: &ChatType,
    text: &str,
    reply_to: Option<&str>,
) -> Result<()> {
    let auth = format!("QQBot {}", token);

    let body = if let Some(msg_id) = reply_to {
        json!({
            "content": text,
            "msg_type": 0,
            "msg_id": msg_id
        })
    } else {
        json!({
            "content": text,
            "msg_type": 0
        })
    };

    let url = match chat_type {
        ChatType::Direct => {
            format!("{}/v2/users/{}/messages", QQ_API_BASE, to)
        }
        ChatType::Group => {
            format!("{}/v2/groups/{}/messages", QQ_API_BASE, to)
        }
    };

    let res = client
        .post(&url)
        .header("Authorization", &auth)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ChannelError::SendError(format!("QQ send failed: {}", e)))?;

    let status = res.status();
    if !status.is_success() {
        let error_text = res.text().await.unwrap_or_default();
        warn!(
            "QQ message send failed (status {}): {}",
            status, error_text
        );
        return Err(ReactError::Channel(ChannelError::SendError(format!(
            "QQ message send failed (status {}): {}",
            status, error_text
        ))));
    }

    debug!("QQ message sent to {} ({:?})", to, chat_type);
    Ok(())
}
