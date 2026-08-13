//! QQ Bot HTTP API — Token acquisition + message sending
//!
//! Official API docs:
//! - Token: POST <https://bots.qq.com/app/getAppAccessToken>
//! - Gateway: GET <https://api.sgroup.qq.com/v2/gateway>
//! - Direct message: POST `https://api.sgroup.qq.com/v2/users/{openid}/messages`
//! - Group message: POST `https://api.sgroup.qq.com/v2/groups/{guild_id}/messages`

use super::super::super::types::ChatType;
use echo_core::error::{ChannelError, ReactError, Result};
use reqwest::Client;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

const QQ_API_BASE: &str = "https://api.sgroup.qq.com";
const QQ_TOKEN_URL: &str = "https://bots.qq.com/app/getAppAccessToken";

pub fn reqwest_client() -> Client {
    Client::builder().build().unwrap_or_else(|_| Client::new())
}

// ── Token Management ────────────────────────────────────────────────────────────

/// QQ Token Manager — acquires and caches access_token
pub(super) struct TokenManager {
    app_id: String,
    client_secret: String,
    token: Arc<Mutex<Option<String>>>,
    expires_at: Arc<AtomicU64>,
    /// Prevent concurrent duplicate refresh
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

    /// Get access_token (auto-cache / refresh)
    pub async fn get_token(&self) -> Result<String> {
        if let Some(token) = self.get_cached().await {
            return Ok(token);
        }
        // Lock to prevent concurrent duplicate refresh
        let _lock = self.refresh_lock.lock().await;
        // Double-check after acquiring the lock
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
            .map_err(|e| ChannelError::NetworkError(format!("QQ token request failed: {}", e)))?;

        if !res.status().is_success() {
            return Err(ReactError::Channel(Box::new(ChannelError::AuthError(
                format!("QQ token request failed with status {}", res.status()),
            ))));
        }

        let json: serde_json::Value = res.json().await.map_err(|e| {
            ChannelError::NetworkError(format!("QQ token response parse error: {}", e))
        })?;

        // Redact access_token in debug logs
        let redacted = {
            let j = json.clone();
            if let Some(obj) = j.as_object() {
                let mut redacted_obj = obj.clone();
                if redacted_obj.contains_key("accessToken") {
                    redacted_obj.insert(
                        "accessToken".to_string(),
                        serde_json::Value::String("***REDACTED***".to_string()),
                    );
                }
                if redacted_obj.contains_key("access_token") {
                    redacted_obj.insert(
                        "access_token".to_string(),
                        serde_json::Value::String("***REDACTED***".to_string()),
                    );
                }
                serde_json::Value::Object(redacted_obj)
            } else {
                json.clone()
            }
        };
        debug!("QQ Bot: token response = {:?}", redacted);

        // QQ API may return the field as access_token (snake_case) or accessToken (camelCase)
        let token = json
            .get("accessToken")
            .or_else(|| json.get("access_token"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                // Redact token from error message
                let err_msg = format!("QQ token response missing accessToken, got: {:?}", {
                    let j = json.clone();
                    if let Some(obj) = j.as_object() {
                        let mut redacted_obj = obj.clone();
                        for key in &[
                            "accessToken",
                            "access_token",
                            "clientSecret",
                            "client_secret",
                        ] {
                            redacted_obj.remove(*key);
                        }
                        serde_json::Value::Object(redacted_obj)
                    } else {
                        json.clone()
                    }
                });
                ReactError::Channel(Box::new(ChannelError::AuthError(err_msg)))
            })?
            .to_string();

        // QQ Token validity is typically 7200 seconds; refresh 5 minutes early
        // Note: expires_in may be a string "7200" or a number 7200
        let expires_in = json
            .get("expiresIn")
            .or_else(|| json.get("expires_in"))
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            })
            .unwrap_or(7200);
        let now = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or_default();
        let refresh_at = now.saturating_add(expires_in.saturating_sub(300));
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

/// Get the WebSocket Gateway URL
pub async fn get_gateway_url(client: &reqwest::Client, token: &str) -> Result<String> {
    // QQ Bot API v2 gateway endpoint: try multiple paths
    let endpoints = [
        format!("{}/gateway", QQ_API_BASE),        // no version prefix
        format!("{}/gateway/bot", QQ_API_BASE),    // bot suffix
        format!("{}/v2/gateway", QQ_API_BASE),     // v2 prefix
        format!("{}/v2/gateway/bot", QQ_API_BASE), // v2 + bot
    ];

    for endpoint in &endpoints {
        debug!("QQ Bot: trying gateway endpoint: {}", endpoint);

        let res = client
            .get(endpoint)
            .header("Authorization", format!("QQBot {}", token))
            .send()
            .await
            .map_err(|e| ChannelError::NetworkError(format!("QQ gateway request failed: {}", e)))?;

        let status = res.status();
        if status.is_success() {
            let json: serde_json::Value = res.json().await.map_err(|e| {
                ChannelError::NetworkError(format!("QQ gateway response parse error: {}", e))
            })?;

            debug!("QQ Bot: gateway response = {:?}", json);

            let url = json
                .get("url")
                .or_else(|| json.get("gateway_url"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ReactError::Channel(Box::new(ChannelError::NetworkError(format!(
                        "QQ gateway response missing url, got: {:?}",
                        json
                    ))))
                })?
                .to_string();

            info!("QQ Bot: gateway URL obtained from {}", endpoint);
            return Ok(url);
        }

        let error_body = res.text().await.unwrap_or_default();
        debug!(
            "QQ Gateway: endpoint {} returned status {}, body: {}",
            endpoint, status, error_body
        );
    }

    Err(ReactError::Channel(Box::new(ChannelError::ApiError {
        status: 404,
        message: "QQ gateway: all endpoints failed".to_string(),
    })))
}

// ── Send Message ──────────────────────────────────────────────────────────────

/// Send a QQ message
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
        warn!("QQ message send failed (status {}): {}", status, error_text);
        return Err(ReactError::Channel(Box::new(ChannelError::SendError(
            format!("QQ message send failed (status {}): {}", status, error_text),
        ))));
    }

    debug!("QQ message sent to {} ({:?})", to, chat_type);
    Ok(())
}
