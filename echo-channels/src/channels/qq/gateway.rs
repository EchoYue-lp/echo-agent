//! QQ Bot WebSocket Gateway —— 接收消息
//!
//! 连接到 QQ 官方 WebSocket Gateway，解析事件并转发到 MessageHandler。

use crate::types::*;
use echo_core::error::ChannelError;
use futures::SinkExt;
use futures::StreamExt;
use std::sync::Arc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

/// 启动 QQ Gateway 连接 —— 无限循环带重连
#[allow(dead_code)]
pub(super) async fn run_gateway_loop(
    wss_url: String,
    handler: Arc<dyn MessageHandler>,
    token: String,
) {
    let mut reconnect_delay: u64 = 1;
    let max_delay: u64 = 60;

    loop {
        info!("QQ Gateway: connecting to gateway...");

        match connect_to_gateway(wss_url.clone(), handler.clone(), token.clone()).await {
            Ok(()) => {
                warn!("QQ Gateway: connection closed, reconnecting in {}s...", reconnect_delay);
            }
            Err(e) => {
                error!("QQ Gateway: connection error: {}", e);
                warn!("QQ Gateway: reconnecting in {}s...", reconnect_delay);
            }
        }

        reconnect_delay = (reconnect_delay * 2).min(max_delay);

        tokio::time::sleep(std::time::Duration::from_secs(reconnect_delay)).await;
    }
}

pub(super) async fn connect_to_gateway(
    wss_url: String,
    handler: Arc<dyn MessageHandler>,
    _token: String,
) -> std::result::Result<(), ChannelError> {
    let (mut ws_stream, _) = connect_async(wss_url.clone())
        .await
        .map_err(|e| ChannelError::ConnectionError(format!("WebSocket connect failed: {}", e)))?;

    info!("QQ Gateway: WebSocket connected");

    loop {
        match ws_stream.next().await {
            Some(Ok(msg)) => match msg {
                Message::Text(text) => {
                    if let Err(e) = handle_gateway_message(handler.clone(), &text).await {
                        warn!("QQ Gateway: failed to handle message: {:?}", e);
                    }
                }
                Message::Ping(payload) => {
                    debug!("QQ Gateway: received PING, sending PONG");
                    if let Err(e) = ws_stream.send(Message::Pong(payload)).await {
                        warn!("QQ Gateway: failed to send PONG: {}", e);
                    }
                }
                Message::Pong(_) => {
                    debug!("QQ Gateway: received PONG");
                }
                Message::Close(_) => {
                    info!("QQ Gateway: received close frame");
                    return Ok(());
                }
                _ => {}
            },
            Some(Err(e)) => {
                warn!("QQ Gateway: WebSocket read error: {}", e);
                return Err(ChannelError::ConnectionError(format!(
                    "WebSocket read error: {}",
                    e
                )));
            }
            None => {
                info!("QQ Gateway: WebSocket stream ended");
                return Ok(());
            }
        }
    }
}

/// 解析并处理 Gateway 事件
async fn handle_gateway_message(
    handler: Arc<dyn MessageHandler>,
    text: &str,
) -> std::result::Result<(), ChannelError> {
    let payload: serde_json::Value = serde_json::from_str(text).map_err(|e| {
        ChannelError::Other(format!("QQ Gateway: failed to parse event: {}", e))
    })?;

    let event_type = payload["t"]
        .as_str()
        .ok_or_else(|| ChannelError::Other("QQ Gateway: missing event type".to_string()))?;

    debug!("QQ Gateway: event_type={}", event_type);

    match event_type {
        "C2C_MESSAGE_CREATE" | "C2C_MESSAGE_CREATE_WITH_INTENT" => {
            handle_c2c_message(handler, &payload).await?;
        }
        "GROUP_AT_MESSAGE_CREATE" | "AT_MESSAGE_CREATE" => {
            handle_group_at_message(handler, &payload).await?;
        }
        _ => {
            // 忽略不需要的事件（如心跳、群公告等）
        }
    }

    Ok(())
}

/// 处理私聊消息
async fn handle_c2c_message(
    handler: Arc<dyn MessageHandler>,
    payload: &serde_json::Value,
) -> std::result::Result<(), ChannelError> {
    let data = &payload["d"];

    let sender_id = data["author"]["id"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let text = data["content"].as_str().unwrap_or("").to_string();
    let message_id = data["id"].as_str().unwrap_or("").to_string();

    if text.is_empty() {
        return Ok(());
    }

    let inbound = InboundMessage::new(
        "qqbot",
        &sender_id,
        &sender_id,
        ChatType::Direct,
        &text,
        &message_id,
    );

    dispatch_to_handler(handler, inbound).await
}

/// 处理群聊 @消息
async fn handle_group_at_message(
    handler: Arc<dyn MessageHandler>,
    payload: &serde_json::Value,
) -> std::result::Result<(), ChannelError> {
    let data = &payload["d"];

    let sender_id = data["author"]["id"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let group_id = data["group"]["id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let text = data["content"].as_str().unwrap_or("").to_string();
    let message_id = data["id"].as_str().unwrap_or("").to_string();

    if text.is_empty() {
        return Ok(());
    }

    let inbound = InboundMessage::new(
        "qqbot",
        &sender_id,
        &group_id,
        ChatType::Group,
        &text,
        &message_id,
    );

    dispatch_to_handler(handler, inbound).await
}

/// 统一分发到 Handler 并发送回复
async fn dispatch_to_handler(
    handler: Arc<dyn MessageHandler>,
    inbound: InboundMessage,
) -> std::result::Result<(), ChannelError> {
    match handler.handle(inbound).await {
        Ok(outbound) => {
            // 这里实际发送由 ChannelPlugin 负责，但 gateway 层也可以回调回去
            // 目前先返回 outbound，由上层处理
            info!(
                "Handler returned outbound: to={}, text={}",
                outbound.to,
                if outbound.text.len() > 50 {
                    &outbound.text[..50]
                } else {
                    &outbound.text
                }
            );
            Ok(())
        }
        Err(e) => {
            warn!("Handler error: {:?}", e);
            Err(ChannelError::Other(format!("Handler error: {:?}", e)))
        }
    }
}
