//! Feishu HTTP API
//!
//! 1. Get WebSocket connection URL (long connection mode)
//! 2. Get tenant_access_token (for sending messages)
//! 3. Send message API

use echo_core::error::{ChannelError, ReactError, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Feishu API base URL
pub const FEISHU_API_BASE: &str = "https://open.feishu.cn/open-apis";

/// Lark (international) API base URL
pub const LARK_API_BASE: &str = "https://open.larksuite.com/open-apis";

/// Feishu WebSocket endpoint base URL (without /open-apis)
pub const FEISHU_WS_BASE: &str = "https://open.feishu.cn";

/// Lark (international) WebSocket endpoint base URL (without /open-apis)
pub const LARK_WS_BASE: &str = "https://open.larksuite.com";

/// Path to get the WebSocket endpoint
pub const WS_ENDPOINT_PATH: &str = "/callback/ws/endpoint";

// ── HTTP Client ───────────────────────────────────────────────────────────────

/// Create a reqwest HTTP client
pub fn http_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| Client::new())
}

// ── WebSocket Endpoint API ────────────────────────────────────────────────────

/// Endpoint response structure
#[derive(Debug, Deserialize)]
pub struct EndpointResponse {
    pub code: i32,
    pub msg: String,
    #[serde(default)]
    pub data: Option<EndpointData>,
}

/// Endpoint data
#[derive(Debug, Deserialize)]
pub struct EndpointData {
    /// WebSocket connection URL (contains device_id and service_id)
    #[serde(rename = "URL")]
    pub url: String,

    /// Client configuration (optional)
    #[serde(rename = "ClientConfig")]
    pub client_config: Option<ClientConfig>,
}

/// Client configuration delivered by the server
#[derive(Debug, Deserialize)]
pub struct ClientConfig {
    /// Reconnect count
    #[serde(rename = "ReconnectCount")]
    pub reconnect_count: Option<i32>,

    /// Reconnect interval (seconds)
    #[serde(rename = "ReconnectInterval")]
    pub reconnect_interval: Option<i32>,

    /// Initial reconnect random jitter (seconds)
    #[serde(rename = "ReconnectNonce")]
    pub reconnect_nonce: Option<i32>,

    /// Ping interval (seconds)
    #[serde(rename = "PingInterval")]
    pub ping_interval: Option<i32>,
}

/// Get WebSocket connection URL
pub async fn get_ws_endpoint(
    client: &Client,
    domain: &str,
    app_id: &str,
    app_secret: &str,
) -> Result<(String, String, Option<ClientConfig>)> {
    let url = format!("{}{}", domain, WS_ENDPOINT_PATH);

    info!("Feishu: getting WebSocket endpoint from {}", url);

    let body = json!({
        "AppID": app_id,
        "AppSecret": app_secret,
    });

    let res = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("locale", "zh")
        .json(&body)
        .send()
        .await
        .map_err(|e| ChannelError::NetworkError(format!("Failed to request endpoint: {}", e)))?;

    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(ReactError::Channel(Box::new(ChannelError::AuthError(format!(
            "Endpoint request failed (status {}): {}",
            status, text
        )))));
    }

    let resp: EndpointResponse = res.json().await.map_err(|e| {
        ChannelError::NetworkError(format!("Failed to parse endpoint response: {}", e))
    })?;

    // Check error code
    match resp.code {
        0 => {}
        1 => {
            return Err(ReactError::Channel(Box::new(ChannelError::AuthError(
                "System busy, please retry later".to_string(),
            ))));
        }
        514 => {
            return Err(ReactError::Channel(Box::new(ChannelError::AuthError(
                "Authentication failed: invalid app_id or app_secret".to_string(),
            ))));
        }
        1000040350 => {
            return Err(ReactError::Channel(Box::new(ChannelError::AuthError(
                "Exceeded connection limit (max 50 connections per app)".to_string(),
            ))));
        }
        code => {
            return Err(ReactError::Channel(Box::new(ChannelError::AuthError(format!(
                "Endpoint error (code {}): {}",
                code, resp.msg
            )))));
        }
    }

    let data = resp.data.ok_or_else(|| {
        ReactError::Channel(Box::new(ChannelError::AuthError(
            "Endpoint response missing data".to_string(),
        )))
    })?;

    if data.url.is_empty() {
        return Err(ReactError::Channel(Box::new(ChannelError::AuthError(
            "Endpoint URL is empty".to_string(),
        ))));
    }

    // Parse URL to get device_id and service_id
    let parsed_url = url::Url::parse(&data.url)
        .map_err(|e| ChannelError::ConnectionError(format!("Invalid endpoint URL: {}", e)))?;

    let device_id = parsed_url
        .query_pairs()
        .find(|(k, _)| k == "device_id")
        .map(|(_, v)| v.to_string())
        .unwrap_or_default();

    let service_id = parsed_url
        .query_pairs()
        .find(|(k, _)| k == "service_id")
        .map(|(_, v)| v.to_string())
        .unwrap_or_default();

    info!(
        "Feishu: got WebSocket endpoint, device_id={}, service_id={}",
        device_id, service_id
    );

    Ok((data.url, service_id, data.client_config))
}

// ── Token Management ────────────────────────────────────────────────────────────

/// tenant_access_token manager
pub struct TokenManager {
    app_id: String,
    app_secret: String,
    domain: String,
    token: Arc<Mutex<Option<String>>>,
    expires_at: Arc<AtomicU64>,
    /// Prevent concurrent duplicate refresh
    refresh_lock: Arc<Mutex<()>>,
    http: Client,
}

impl TokenManager {
    pub fn new(app_id: String, app_secret: String, domain: String) -> Self {
        Self {
            app_id,
            app_secret,
            domain,
            token: Arc::new(Mutex::new(None)),
            expires_at: Arc::new(AtomicU64::new(0)),
            refresh_lock: Arc::new(Mutex::new(())),
            http: http_client(),
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
        // Lock to prevent concurrent duplicate refresh
        let _lock = self.refresh_lock.lock().await;
        // Double-check after acquiring lock; another coroutine may have already refreshed
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

        let url = format!("{}{}", self.domain, "/auth/v3/tenant_access_token/internal");

        let res = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelError::NetworkError(format!("Token request failed: {}", e)))?;

        if !res.status().is_success() {
            return Err(ReactError::Channel(Box::new(ChannelError::AuthError(format!(
                "Token request failed with status {}",
                res.status()
            )))));
        }

        let json: serde_json::Value = res.json().await.map_err(|e| {
            ChannelError::NetworkError(format!("Token response parse error: {}", e))
        })?;

        let code = json["code"].as_i64().unwrap_or(-1);
        if code != 0 {
            let msg = json["msg"].as_str().unwrap_or("unknown error").to_string();
            return Err(ReactError::Channel(Box::new(ChannelError::AuthError(format!(
                "Token API error (code {}): {}",
                code, msg
            )))));
        }

        let token = json["tenant_access_token"]
            .as_str()
            .ok_or_else(|| {
                ReactError::Channel(Box::new(ChannelError::AuthError(
                    "Token response missing tenant_access_token".to_string(),
                )))
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

// ── Send Message ──────────────────────────────────────────────────────────────

/// Send a Feishu message
pub async fn send_message(
    client: &Client,
    domain: &str,
    token: &str,
    receive_id: &str,
    receive_id_type: &str,
    msg_type: &str,
    content: &str,
) -> Result<String> {
    let url = format!(
        "{}{}?receive_id_type={}",
        domain, "/im/v1/messages", receive_id_type
    );

    let body = json!({
        "receive_id": receive_id,
        "msg_type": msg_type,
        "content": content,
    });

    debug!(
        "Feishu: sending message to {} (type={})",
        receive_id, msg_type
    );

    let res = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ChannelError::SendError(format!("Send request failed: {}", e)))?;

    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        warn!("Feishu: send message failed (status {}): {}", status, text);
        return Err(ReactError::Channel(Box::new(ChannelError::SendError(format!(
            "Send message failed (status {}): {}",
            status, text
        )))));
    }

    let json: serde_json::Value = res
        .json()
        .await
        .map_err(|e| ChannelError::SendError(format!("Send response parse error: {}", e)))?;

    let code = json["code"].as_i64().unwrap_or(-1);
    if code != 0 {
        let msg = json["msg"].as_str().unwrap_or("unknown error").to_string();
        return Err(ReactError::Channel(Box::new(ChannelError::SendError(format!(
            "Send API error (code {}): {}",
            code, msg
        )))));
    }

    // Return message ID
    let message_id = json["data"]["message_id"]
        .as_str()
        .unwrap_or("")
        .to_string();

    debug!("Feishu: message sent, message_id={}", message_id);
    Ok(message_id)
}

/// Send a text message
pub async fn send_text_message(
    client: &Client,
    domain: &str,
    token: &str,
    receive_id: &str,
    receive_id_type: &str,
    text: &str,
) -> Result<String> {
    let content = json!({ "text": text }).to_string();
    send_message(
        client,
        domain,
        token,
        receive_id,
        receive_id_type,
        "text",
        &content,
    )
    .await
}

/// Send a card message
pub async fn send_card_message(
    client: &Client,
    domain: &str,
    token: &str,
    receive_id: &str,
    receive_id_type: &str,
    card_content: &str,
) -> Result<String> {
    send_message(
        client,
        domain,
        token,
        receive_id,
        receive_id_type,
        "interactive",
        card_content,
    )
    .await
}

/// Reply to a message (reply in thread)
pub async fn reply_message(
    client: &Client,
    domain: &str,
    token: &str,
    message_id: &str,
    msg_type: &str,
    content: &str,
) -> Result<String> {
    let url = format!("{}/im/v1/messages/{}/reply", domain, message_id);

    let body = json!({
        "msg_type": msg_type,
        "content": content,
    });

    debug!("Feishu: replying to message {}", message_id);

    let res = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ChannelError::SendError(format!("Reply request failed: {}", e)))?;

    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(ReactError::Channel(Box::new(ChannelError::SendError(format!(
            "Reply failed (status {}): {}",
            status, text
        )))));
    }

    let json: serde_json::Value = res
        .json()
        .await
        .map_err(|e| ChannelError::SendError(format!("Reply response parse error: {}", e)))?;

    let code = json["code"].as_i64().unwrap_or(-1);
    if code != 0 {
        let msg = json["msg"].as_str().unwrap_or("unknown error").to_string();
        return Err(ReactError::Channel(Box::new(ChannelError::SendError(format!(
            "Reply API error (code {}): {}",
            code, msg
        )))));
    }

    let reply_id = json["data"]["message_id"]
        .as_str()
        .unwrap_or("")
        .to_string();

    Ok(reply_id)
}

/// Update a card message
pub async fn patch_card_message(
    client: &Client,
    domain: &str,
    token: &str,
    message_id: &str,
    card_content: &str,
) -> Result<()> {
    let url = format!("{}{}{}", domain, "/im/v1/messages/", message_id);

    let body = json!({
        "content": card_content,
    });

    let res = client
        .patch(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ChannelError::SendError(format!("Patch request failed: {}", e)))?;

    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(ReactError::Channel(Box::new(ChannelError::SendError(format!(
            "Patch failed (status {}): {}",
            status, text
        )))));
    }

    let json: serde_json::Value = res
        .json()
        .await
        .map_err(|e| ChannelError::SendError(format!("Patch response parse error: {}", e)))?;

    let code = json["code"].as_i64().unwrap_or(-1);
    if code != 0 {
        let msg = json["msg"].as_str().unwrap_or("unknown error").to_string();
        return Err(ReactError::Channel(Box::new(ChannelError::SendError(format!(
            "Patch API error (code {}): {}",
            code, msg
        )))));
    }

    Ok(())
}

/// Add an emoji reaction to a message
pub async fn add_reaction(
    client: &Client,
    domain: &str,
    token: &str,
    message_id: &str,
    emoji_type: &str,
) -> Result<()> {
    let url = format!(
        "{}{}{}{}",
        domain, "/im/v1/messages/", message_id, "/reactions"
    );

    let body = json!({
        "reaction_type": {
            "emoji_type": emoji_type
        }
    });

    let res = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ChannelError::SendError(format!("Reaction request failed: {}", e)))?;

    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        warn!("Feishu: add reaction failed: {}", text);
        // Do not return an error; reaction failure does not affect the main flow
    }

    Ok(())
}
