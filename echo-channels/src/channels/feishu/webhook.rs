//! Feishu Webhook Server
//!
//! HTTP 事件推送模式，需要公网 IP。
//!
//! 用于接收飞书推送的事件消息。

use crate::types::*;
use axum::response::IntoResponse;
use axum::{Router, extract::State, routing::post};
use dashmap::DashMap;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

/// 飞书事件类型
const FEISHU_IM_MESSAGE_RECEIVE: &str = "im.message.receive_v1";

/// 已处理事件去重缓存 TTL
const DEDUP_TTL_SECS: u64 = 300;

/// Webhook 状态
struct WebhookState {
    handler: Arc<dyn MessageHandler>,
    verification_token: Option<String>,
    /// 已处理事件去重缓存（message_id -> 处理时间）
    processed_events: DashMap<String, Instant>,
}

/// 处理飞书事件
async fn handle_event(
    State(state): State<Arc<WebhookState>>,
    axum::extract::Json(body): axum::extract::Json<serde_json::Value>,
) -> axum::response::Response {
    // 1. challenge 验证
    if let Some(challenge) = body.get("challenge").and_then(|v| v.as_str()) {
        info!("Feishu webhook: responding to challenge verification");
        return axum::Json(json!({ "challenge": challenge })).into_response();
    }

    // 2. 验证 verification_token
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

    // 3. 验证事件类型
    let event_type = body["header"]["event_type"].as_str();
    if event_type != Some(FEISHU_IM_MESSAGE_RECEIVE) {
        debug!("Feishu webhook: ignoring event type: {:?}", event_type);
        return axum::Json(json!({})).into_response();
    }

    // 4. 解析消息
    let event = &body["event"];
    let message = &event["message"];

    if message.is_null() {
        return axum::Json(json!({})).into_response();
    }

    let message_id = message["message_id"].as_str().unwrap_or("").to_string();

    // 5. 去重检查（飞书 Webhook 超时会重试投递）
    if !message_id.is_empty() {
        if state.processed_events.contains_key(&message_id) {
            debug!(
                "Feishu webhook: duplicate event, message_id={}, skipping",
                message_id
            );
            return axum::Json(json!({})).into_response();
        }
        // 先标记为已处理，再异步处理（防止重试期间重复）
        state
            .processed_events
            .insert(message_id.clone(), Instant::now());

        // 定期清理过期缓存
        let ttl = Duration::from_secs(DEDUP_TTL_SECS);
        state.processed_events.retain(|_, v| v.elapsed() < ttl);
    }

    let message_type = message["message_type"].as_str().unwrap_or("").to_string();
    let chat_id = message["chat_id"].as_str().unwrap_or("").to_string();
    let chat_type_str = message["chat_type"].as_str().unwrap_or("p2p").to_string();

    // 只处理文本消息
    if message_type != "text" {
        debug!(
            "Feishu webhook: ignoring non-text message: {}",
            message_type
        );
        return axum::Json(json!({})).into_response();
    }

    // 解析文本内容
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

    // 6. 立即返回 200，然后异步处理（避免飞书超时重试）
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

/// 启动 Webhook Server
pub(super) async fn run_webhook_server(
    bind_addr: String,
    webhook_path: String,
    handler: Arc<dyn MessageHandler>,
    verification_token: Option<String>,
) -> Result<(), echo_core::error::ChannelError> {
    let state = Arc::new(WebhookState {
        handler,
        verification_token,
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
