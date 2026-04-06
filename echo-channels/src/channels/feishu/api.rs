//! 飞书 HTTP API —— Token 获取 + 消息发送
//!
//! 官方 API 文档：
//! - Token: POST https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal
//! - 发送消息: POST https://open.feishu.cn/open-apis/im/v1/messages

use crate::types::ChatType;
use echo_core::error::{ChannelError, ReactError, Result};
use reqwest::Client;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

const FEISHU_API_BASE: &str = "https://open.feishu.cn/open-apis";

pub fn reqwest_client() -> Client {
    Client::builder().build().expect("Failed to create HTTP client")
}

// ── Token 管理 ────────────────────────────────────────────────────────────────

/// 飞书 tenant_access_token 管理器
pub(super) struct TokenManager {
    app_id: String,
    app_secret: String,
    token: Arc<Mutex<Option<String>>>,
    expires_at: Arc<AtomicU64>,
    http: reqwest::Client,
}

impl TokenManager {
    pub fn new(app_id: String, app_secret: String) -> Self {
        Self {
            app_id,
            app_secret,
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

    pub async fn get_token(&self) -> Result<String> {
        if let Some(token) = self.get_cached().await {
            return Ok(token);
        }
        self.refresh_token().await
    }

    async fn refresh_token(&self) -> Result<String> {
        info!("Feishu: refreshing tenant_access_token");

        let body = json!({
            "app_id": self.app_id,
            "app_secret": self.app_secret,
        });

        let res = self
            .http
            .post(format!(
                "{}/auth/v3/tenant_access_token/internal",
                FEISHU_API_BASE
            ))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                ChannelError::NetworkError(format!("Feishu token request failed: {}", e))
            })?;

        if !res.status().is_success() {
            return Err(ReactError::Channel(ChannelError::AuthError(format!(
                "Feishu token request failed with status {}",
                res.status()
            ))));
        }

        let json: serde_json::Value = res.json().await.map_err(|e| {
            ChannelError::NetworkError(format!("Feishu token response parse error: {}", e))
        })?;

        let code = json["code"].as_i64().unwrap_or(-1);
        if code != 0 {
            let msg = json["msg"].as_str().unwrap_or("unknown error").to_string();
            return Err(ReactError::Channel(ChannelError::AuthError(format!(
                "Feishu token API error (code {}): {}",
                code, msg
            ))));
        }

        let token = json["tenant_access_token"]
            .as_str()
            .ok_or_else(|| {
                ReactError::Channel(ChannelError::AuthError(
                    "Feishu token response missing tenant_access_token".to_string(),
                ))
            })?
            .to_string();

        let expires_in = json["expire"].as_u64().unwrap_or(7200);
        let refresh_at = (chrono::Utc::now().timestamp() as u64) + expires_in - 300;
        self.expires_at.store(refresh_at, Ordering::Relaxed);

        let mut tok = self.token.lock().await;
        *tok = Some(token.clone());

        info!("Feishu: token refreshed, expires in {}s", expires_in);
        Ok(token)
    }
}

// ── 发送消息 ─────────────────────────────────────────────────────────────────

/// 发送飞书消息
pub async fn send_feishu_message(
    token: &str,
    to: &str,
    chat_type: &ChatType,
    text: &str,
) -> Result<()> {
    let client = reqwest_client();

    let receive_id_type = match chat_type {
        ChatType::Direct => "open_id",
        ChatType::Group => "chat_id",
    };

    let body = json!({
        "receive_id": to,
        "msg_type": "text",
        "content": json!({ "text": text }).to_string(),
    });

    let url = format!(
        "{}/im/v1/messages?receive_id_type={}",
        FEISHU_API_BASE, receive_id_type
    );

    let res = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ChannelError::SendError(format!("Feishu send failed: {}", e)))?;

    let status = res.status();
    if !status.is_success() {
        let error_text = res.text().await.unwrap_or_default();
        warn!(
            "Feishu message send failed (status {}): {}",
            status, error_text
        );
        return Err(ReactError::Channel(ChannelError::SendError(format!(
            "Feishu message send failed (status {}): {}",
            status, error_text
        ))));
    }

    let json: serde_json::Value = res
        .json()
        .await
        .map_err(|e| ChannelError::SendError(format!("Feishu response parse error: {}", e)))?;

    let code = json["code"].as_i64().unwrap_or(-1);
    if code != 0 {
        let msg = json["msg"].as_str().unwrap_or("unknown error").to_string();
        return Err(ReactError::Channel(ChannelError::SendError(format!(
            "Feishu API error (code {}): {}",
            code, msg
        ))));
    }

    debug!("Feishu message sent to {} ({:?})", to, chat_type);
    Ok(())
}
