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
    http: reqwest::Client,
}

impl TokenManager {
    pub fn new(app_id: String, client_secret: String) -> Self {
        Self {
            app_id,
            client_secret,
            token: Arc::new(Mutex::new(None)),
            expires_at: Arc::new(AtomicU64::new(0)),
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

        let token = json["accessToken"]
            .as_str()
            .ok_or_else(|| ReactError::Channel(ChannelError::AuthError("QQ token response missing accessToken".to_string())))?
            .to_string();

        // QQ Token 有效期通常 7200 秒，提前 5 分钟刷新
        let expires_in = json["expiresIn"].as_u64().unwrap_or(7200);
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
pub async fn get_gateway_url(token: &str) -> Result<String> {
    let client = reqwest_client();
    let res = client
        .get(format!("{}/v2/gateway", QQ_API_BASE))
        .header("Authorization", format!("QQBot {}", token))
        .send()
        .await
        .map_err(|e| {
            ChannelError::NetworkError(format!("QQ gateway request failed: {}", e))
        })?;

    if !res.status().is_success() {
        return Err(ReactError::Channel(ChannelError::ApiError {
            status: res.status().as_u16(),
            message: "QQ gateway request failed".to_string(),
        }));
    }

    let json: serde_json::Value = res.json().await.map_err(|e| {
        ChannelError::NetworkError(format!("QQ gateway response parse error: {}", e))
    })?;

    let url = json["url"]
        .as_str()
        .ok_or_else(|| {
            ReactError::Channel(ChannelError::NetworkError("QQ gateway response missing url".to_string()))
        })?
        .to_string();

    debug!("QQ Bot: gateway URL obtained");
    Ok(url)
}

// ── 发送消息 ─────────────────────────────────────────────────────────────────

/// 发送 QQ 消息
pub async fn send_qq_message(
    token: &str,
    to: &str,
    chat_type: &ChatType,
    text: &str,
    reply_to: Option<&str>,
) -> Result<()> {
    let client = reqwest_client();
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
