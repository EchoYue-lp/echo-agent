//! Feishu Webhook Server
//!
//! HTTP event push mode; requires a public IP.
//!
//! Used to receive event messages pushed by Feishu.

use super::super::super::types::*;
use axum::response::IntoResponse;
use axum::{Router, extract::State, routing::post};
use dashmap::DashMap;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use std::sync::Arc;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

/// Feishu event type
const FEISHU_IM_MESSAGE_RECEIVE: &str = "im.message.receive_v1";

/// TTL for processed event dedup cache
const DEDUP_TTL_SECS: u64 = 300;

type HmacSha256 = Hmac<Sha256>;

/// Verify Feishu Webhook signature (constant-time comparison to prevent timing attacks).
///
/// Feishu signature algorithm:
/// 1. Concatenate `timestamp + "\n" + nonce + "\n" + body`
/// 2. Compute HMAC-SHA256 using signing_key
/// 3. Base64-encode to get the signature
/// 4. Compare `X-Lark-Signature` header with the computed value
fn verify_feishu_signature(
    signing_key: &str,
    timestamp: &str,
    nonce: &str,
    body: &str,
    expected_signature: &str,
) -> bool {
    let msg = format!("{}\n{}\n{}", timestamp, nonce, body);

    let mut mac =
        HmacSha256::new_from_slice(signing_key.as_bytes()).expect("HMAC can take key of any size");
    mac.update(msg.as_bytes());
    let computed = mac.finalize().into_bytes();

    // Base64-encode the computed result
    let computed_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, computed);

    // Constant-time comparison to prevent timing attacks
    if computed_b64.len() != expected_signature.len() {
        return false;
    }
    computed_b64
        .as_bytes()
        .ct_eq(expected_signature.as_bytes())
        .into()
}

/// Webhook state
struct WebhookState {
    handler: Arc<dyn MessageHandler>,
    verification_token: Option<String>,
    /// Signing key (for HMAC signature verification)
    signing_key: Option<String>,
    /// Processed event dedup cache (message_id -> processing time)
    processed_events: DashMap<String, Instant>,
}

/// Handle Feishu event
async fn handle_event(
    State(state): State<Arc<WebhookState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Json(body): axum::extract::Json<serde_json::Value>,
) -> axum::response::Response {
    // 1. challenge 验证
    if let Some(challenge) = body.get("challenge").and_then(|v| v.as_str()) {
        info!("Feishu webhook: responding to challenge verification");
        return axum::Json(json!({ "challenge": challenge })).into_response();
    }

    // 2. Verify verification_token
    if let Some(ref expected_token) = state.verification_token {
        let actual_token = body.get("header").and_then(|h| h["token"].as_str());
        if actual_token != Some(expected_token.as_str()) {
            warn!("Feishu webhook: verification_token mismatch");
            return axum::response::Response::builder()
                .status(axum::http::StatusCode::UNAUTHORIZED)
                .body(axum::body::Body::empty())
                .unwrap()
                .into_response();
        }
    }

    // 3. HMAC signature verification (if signing_key is configured)
    if let Some(ref signing_key) = state.signing_key {
        let timestamp = headers
            .get("X-Lark-Request-Timestamp")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let nonce = headers
            .get("X-Lark-Request-Nonce")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let signature = headers
            .get("X-Lark-Signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Serialize body to JSON string for signature verification
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        if !verify_feishu_signature(signing_key, timestamp, nonce, &body_str, signature) {
            warn!("Feishu webhook: signature verification failed (timing-safe)");
            return axum::response::Response::builder()
                .status(axum::http::StatusCode::UNAUTHORIZED)
                .body(axum::body::Body::empty())
                .unwrap()
                .into_response();
        }

        debug!("Feishu webhook: signature verification passed");
    }

    // 4. Verify event type
    let event_type = body["header"]["event_type"].as_str();
    if event_type != Some(FEISHU_IM_MESSAGE_RECEIVE) {
        debug!("Feishu webhook: ignoring event type: {:?}", event_type);
        return axum::Json(json!({})).into_response();
    }

    // 5. Parse message
    let event = &body["event"];
    let message = &event["message"];

    if message.is_null() {
        return axum::Json(json!({})).into_response();
    }

    let message_id = message["message_id"].as_str().unwrap_or("").to_string();

    // 6. Dedup check (Feishu Webhook retries on timeout)
    if !message_id.is_empty() {
        if state.processed_events.contains_key(&message_id) {
            debug!(
                "Feishu webhook: duplicate event, message_id={}, skipping",
                message_id
            );
            return axum::Json(json!({})).into_response();
        }
        // Mark as processed first, then process async (prevents duplicates during retries)
        state
            .processed_events
            .insert(message_id.clone(), Instant::now());

        // Periodically clean up expired cache entries
        let ttl = Duration::from_secs(DEDUP_TTL_SECS);
        state.processed_events.retain(|_, v| v.elapsed() < ttl);
    }

    let message_type = message["message_type"].as_str().unwrap_or("").to_string();
    let chat_id = message["chat_id"].as_str().unwrap_or("").to_string();
    let chat_type_str = message["chat_type"].as_str().unwrap_or("p2p").to_string();

    // Only handle text messages
    if message_type != "text" {
        debug!(
            "Feishu webhook: ignoring non-text message: {}",
            message_type
        );
        return axum::Json(json!({})).into_response();
    }

    // Parse text content
    let content_str = message["content"].as_str().unwrap_or("{}").to_string();
    let content: serde_json::Value = serde_json::from_str(&content_str).unwrap_or_default();
    let text = content["text"].as_str().unwrap_or("").to_string();

    if text.is_empty() {
        return axum::Json(json!({})).into_response();
    }

    let sender_id = event["sender"]["sender_id"]["open_id"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    // 7. Immediately return 200, then process async (avoids Feishu timeout retries)
    let handler = state.handler.clone();
    tokio::spawn(async move {
        let chat_type = if chat_type_str == "group" {
            ChatType::Group
        } else {
            ChatType::Direct
        };

        let inbound =
            InboundMessage::new("feishu", sender_id, chat_id, chat_type, text, message_id);

        match handler.handle(inbound).await {
            Ok(outbound) => {
                if let Err(e) = handler.reply(outbound).await {
                    warn!("Feishu webhook: failed to send reply: {:?}", e);
                }
            }
            Err(e) => {
                warn!("Feishu webhook: handler error: {:?}", e);
            }
        }
    });

    axum::Json(json!({})).into_response()
}

/// Start Webhook Server
pub(super) async fn run_webhook_server(
    bind_addr: String,
    webhook_path: String,
    handler: Arc<dyn MessageHandler>,
    verification_token: Option<String>,
    signing_key: Option<String>,
) -> Result<(), echo_core::error::ChannelError> {
    let state = Arc::new(WebhookState {
        handler,
        verification_token,
        signing_key,
        processed_events: DashMap::new(),
    });

    let app = Router::new()
        .route(&webhook_path, post(handle_event))
        .with_state(state);

    info!(
        "Feishu webhook server listening on {}{}",
        bind_addr, webhook_path
    );

    let listener = TcpListener::bind(&bind_addr).await.map_err(|e| {
        echo_core::error::ChannelError::ConnectionError(format!(
            "Failed to bind webhook server: {}",
            e
        ))
    })?;

    axum::serve(listener, app).await.map_err(|e| {
        echo_core::error::ChannelError::ConnectionError(format!("Webhook server error: {}", e))
    })?;

    Ok(())
}
