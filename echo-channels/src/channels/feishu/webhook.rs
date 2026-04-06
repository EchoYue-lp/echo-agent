//! 飞书 Webhook Server —— 接收事件推送
//!
//! 飞书使用 HTTP 事件推送。本模块启动一个轻量 HTTP 服务器接收事件。

use crate::types::*;
use axum::response::IntoResponse;
use axum::{extract::State, routing::post, Router};
use serde_json::json;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

/// 飞书事件推送事件类型
const FEISHU_IM_MESSAGE_RECEIVE: &str = "im.message.receive_v1";

/// Webhook 状态
struct WebhookState {
    handler: Arc<dyn MessageHandler>,
    verification_token: Option<String>,
}

/// 处理飞书事件
async fn handle_event(
    State(state): State<Arc<WebhookState>>,
    axum::extract::Json(body): axum::extract::Json<serde_json::Value>,
) -> axum::response::Response {
    // 1. challenge 验证（飞书配置 webhook URL 时的验证请求）
    if let Some(challenge) = body.get("challenge").and_then(|v| v.as_str()) {
        info!("Feishu: responding to challenge verification");
        return axum::Json(json!({
            "challenge": challenge
        }))
        .into_response();
    }

    // 2. 验证 verification_token（如果配置了）
    if let Some(ref expected_token) = state.verification_token {
        let actual_token = body.get("header").and_then(|h| h["token"].as_str());
        if actual_token != Some(expected_token.as_str()) {
            warn!("Feishu: verification_token mismatch, rejecting event");
            return axum::response::Response::builder()
                .status(axum::http::StatusCode::UNAUTHORIZED)
                .body(axum::body::Body::empty())
                .unwrap()
                .into_response();
        }
    }

    // 3. 验证事件类型
    let header_event_type = body["header"]["event_type"].as_str();
    if header_event_type != Some(FEISHU_IM_MESSAGE_RECEIVE) {
        debug!(
            "Feishu: ignoring event type: {:?}",
            header_event_type
        );
        return axum::Json(json!({})).into_response();
    }

    // 4. 解析事件数据
    let event_data = &body["event"];
    let message = &event_data["message"];

    if message.is_null() {
        return axum::Json(json!({})).into_response();
    }

    let message_type = message["message_type"].as_str().unwrap_or("");
    let message_id = message["message_id"].as_str().unwrap_or("").to_string();
    let chat_id = message["chat_id"].as_str().unwrap_or("").to_string();
    let chat_type_str = message["chat_type"].as_str().unwrap_or("p2p");

    // 只处理文本消息
    if message_type != "text" {
        debug!("Feishu: ignoring non-text message type: {}", message_type);
        return axum::Json(json!({})).into_response();
    }

    // 飞书的 text 字段是 JSON 字符串：{"text":"hello"}
    let text = message["content"]
        .as_str()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v["text"].as_str().map(String::from))
        .unwrap_or_default();

    if text.is_empty() {
        return axum::Json(json!({})).into_response();
    }

    let sender_id = event_data["sender"]["sender_id"]["open_id"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    let chat_type = if chat_type_str == "group" {
        ChatType::Group
    } else {
        ChatType::Direct
    };

    // 5. 构建入站消息并分发
    let inbound = InboundMessage::new(
        "feishu",
        sender_id,
        chat_id,
        chat_type,
        &text,
        message_id,
    );

    match state.handler.handle(inbound.clone()).await {
        Ok(outbound) => {
            info!(
                "Feishu: handler returned outbound to={}, text={}",
                outbound.to,
                if outbound.text.len() > 50 {
                    &outbound.text[..50]
                } else {
                    &outbound.text
                }
            );
            // 通过 handler.reply() 发送（wrapper 会自动路由到 send_tx）
            if let Err(e) = state.handler.reply(outbound).await {
                warn!("Feishu: failed to send reply: {:?}", e);
            }
        }
        Err(e) => {
            warn!("Feishu: handler error: {:?}", e);
        }
    }

    axum::Json(json!({})).into_response()
}

/// 启动飞书 Webhook Server，监听指定地址和路径
pub(super) async fn run_webhook_server(
    bind_addr: String,
    webhook_path: String,
    handler: Arc<dyn MessageHandler>,
    verification_token: Option<String>,
) -> Result<(), ChannelError> {
    let state = Arc::new(WebhookState {
        handler,
        verification_token,
    });

    let app = Router::new()
        .route(&webhook_path, post(handle_event))
        .with_state(state);

    info!("Feishu webhook server listening on {}{}", bind_addr, webhook_path);

    let listener = TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| ChannelError::ConnectionError(format!(
            "Failed to bind webhook server to {}: {}",
            bind_addr, e
        )))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| ChannelError::ConnectionError(format!(
            "Webhook server error: {}",
            e
        )))
}
