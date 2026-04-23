//! QQ Bot WebSocket Gateway —— 接收消息
//!
//! 连接到 QQ 官方 WebSocket Gateway，解析事件并转发到 MessageHandler。
//!
//! QQ Bot WebSocket 协议:
//! 1. 连接后收到 HELLO 事件（包含 heartbeat_interval）
//! 2. 发送 IDENTIFY（包含 token、intents 等）
//! 3. 定期发送 HEARTBEAT
//! 4. 接收消息事件（C2C_MESSAGE_CREATE, GROUP_AT_MESSAGE_CREATE 等）

use super::super::super::types::*;
use echo_core::error::ChannelError;
use futures::SinkExt;
use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

/// QQ Bot WebSocket opcodes (Discord Gateway 协议)
const OP_DISPATCH: u32 = 0; // 事件消息
const OP_HEARTBEAT: u32 = 1; // 心跳请求（客户端发送）
const OP_IDENTIFY: u32 = 2; // 鉴权（客户端发送）
#[allow(dead_code)]
const OP_RESUME: u32 = 6; // 恢复会话（客户端发送）- 用于断线重连
const OP_RECONNECT: u32 = 7; // 重连请求（服务器发送）
const OP_HELLO: u32 = 10; // Hello（服务器发送，包含心跳间隔）
const OP_HEARTBEAT_ACK: u32 = 11; // 心跳确认（服务器发送）

/// QQ Bot intents（事件订阅位掩码）
/// 参考: https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/interface-framework/event-emit.html
/// 重要：GROUP_AND_C2C_EVENT (1<<25) 包含 QQ 单聊和群聊 @消息事件
const INTENT_GUILDS: u32 = 1 << 0; // 频道事件（默认权限）
const INTENT_GUILD_MEMBERS: u32 = 1 << 1; // 频道成员事件（默认权限）
#[allow(dead_code)]
const INTENT_GUILD_MESSAGES: u32 = 1 << 9; // 频道消息事件（私域机器人）
#[allow(dead_code)]
const INTENT_GUILD_MESSAGE_REACTIONS: u32 = 1 << 10; // 频道消息表情表态
#[allow(dead_code)]
const INTENT_DIRECT_MESSAGE: u32 = 1 << 12; // 频道私信事件
const INTENT_GROUP_AND_C2C_EVENT: u32 = 1 << 25; // QQ 单聊+群聊事件（关键！包含 C2C_MESSAGE_CREATE）
#[allow(dead_code)]
const INTENT_PUBLIC_GUILD_MESSAGES: u32 = 1 << 30; // 公域消息事件（频道 @机器人，默认权限）

/// 启动 QQ Gateway 连接 —— 无限循环带指数退避重连
#[allow(dead_code)]
pub(super) async fn run_gateway_loop(
    wss_url: String,
    handler: Arc<dyn MessageHandler>,
    token: String,
) {
    let mut reconnect_delay: u64 = 1;
    let max_delay: u64 = 60;
    const STABLE_THRESHOLD_SECS: u64 = 60;

    loop {
        info!("QQ Gateway: connecting to gateway...");
        let connected_at = std::time::Instant::now();

        match connect_to_gateway(wss_url.clone(), handler.clone(), token.clone()).await {
            Ok(()) => {
                warn!(
                    "QQ Gateway: connection closed, reconnecting in {}s...",
                    reconnect_delay
                );
            }
            Err(e) => {
                error!("QQ Gateway: connection error: {}", e);
                warn!("QQ Gateway: reconnecting in {}s...", reconnect_delay);
            }
        }

        // 连接稳定则重置退避延迟
        if connected_at.elapsed().as_secs() >= STABLE_THRESHOLD_SECS {
            reconnect_delay = 1;
        } else {
            reconnect_delay = (reconnect_delay * 2).min(max_delay);
        }

        tokio::time::sleep(Duration::from_secs(reconnect_delay)).await;
    }
}

pub(super) async fn connect_to_gateway(
    wss_url: String,
    handler: Arc<dyn MessageHandler>,
    token: String,
) -> std::result::Result<(), ChannelError> {
    let (ws_stream, _) = connect_async(wss_url.clone())
        .await
        .map_err(|e| ChannelError::ConnectionError(format!("WebSocket connect failed: {}", e)))?;

    info!("QQ Gateway: WebSocket connected");

    // 分离 WebSocket 为 sender 和 receiver
    let (ws_sender, mut ws_receiver) = ws_stream.split();

    // 共享状态
    let last_seq = Arc::new(Mutex::new(0u64));
    let heartbeat_interval = Arc::new(Mutex::new(Duration::from_secs(30)));
    let identified = Arc::new(Mutex::new(false));

    // 启动心跳任务
    let last_seq_clone = last_seq.clone();
    let heartbeat_interval_clone = heartbeat_interval.clone();
    let identified_clone = identified.clone();
    let ws_sender_clone = Arc::new(Mutex::new(ws_sender));
    let ws_sender_for_heartbeat = ws_sender_clone.clone();

    let heartbeat_task = tokio::spawn(async move {
        // 等待 IDENTIFY 完成
        while !*identified_clone.lock().await {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // 开始心跳循环
        let interval = *heartbeat_interval_clone.lock().await;
        let mut timer = tokio::time::interval(interval);

        loop {
            timer.tick().await;

            let seq = *last_seq_clone.lock().await;
            let heartbeat = serde_json::json!({
                "op": OP_HEARTBEAT,
                "d": seq
            });

            let mut sender = ws_sender_for_heartbeat.lock().await;
            if let Err(e) = sender.send(Message::Text(heartbeat.to_string())).await {
                warn!("QQ Gateway: heartbeat failed: {}", e);
                break;
            }
            debug!("QQ Gateway: heartbeat sent (seq={})", seq);
        }
    });

    // 处理消息循环
    loop {
        match ws_receiver.next().await {
            Some(Ok(Message::Text(text))) => {
                debug!(
                    "QQ Gateway: received message: {}",
                    if text.len() > 200 {
                        &text[..200]
                    } else {
                        &text
                    }
                );

                let payload: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
                    ChannelError::Other(format!("QQ Gateway: failed to parse event: {}", e))
                })?;

                let op = payload["op"].as_u64().unwrap_or(0);
                {
                    let mut seq = last_seq.lock().await;
                    *seq = payload["s"].as_u64().unwrap_or(*seq);
                }

                match op as u32 {
                    OP_HELLO => {
                        // HELLO: 获取心跳间隔并发送 IDENTIFY
                        let interval_ms =
                            payload["d"]["heartbeat_interval"].as_u64().unwrap_or(30000);

                        info!(
                            "QQ Gateway: HELLO received, heartbeat_interval={}ms",
                            interval_ms
                        );

                        {
                            let mut interval = heartbeat_interval.lock().await;
                            *interval = Duration::from_millis(interval_ms);
                        }

                        // 发送 IDENTIFY
                        let identify = serde_json::json!({
                            "op": OP_IDENTIFY,
                            "d": {
                                "token": format!("QQBot {}", token),
                                "intents": INTENT_GUILDS | INTENT_GUILD_MEMBERS | INTENT_GROUP_AND_C2C_EVENT,
                                "shard": [0, 1],
                            }
                        });

                        {
                            let mut sender = ws_sender_clone.lock().await;
                            sender
                                .send(Message::Text(identify.to_string()))
                                .await
                                .map_err(|e| {
                                    ChannelError::ConnectionError(format!(
                                        "Failed to send IDENTIFY: {}",
                                        e
                                    ))
                                })?;
                        }

                        info!("QQ Gateway: IDENTIFY sent");
                        {
                            let mut id = identified.lock().await;
                            *id = true;
                        }
                    }
                    OP_HEARTBEAT_ACK => {
                        debug!("QQ Gateway: heartbeat ACK received");
                    }
                    OP_RECONNECT => {
                        warn!("QQ Gateway: RECONNECT requested");
                        heartbeat_task.abort();
                        return Err(ChannelError::ConnectionError(
                            "Server requested reconnect".to_string(),
                        ));
                    }
                    OP_DISPATCH => {
                        // 事件消息（op=0 表示 dispatch event）
                        if *identified.lock().await
                            && let Err(e) = handle_gateway_event(handler.clone(), &payload).await
                        {
                            warn!("QQ Gateway: failed to handle event: {:?}", e);
                        }
                    }
                    _ => {
                        debug!("QQ Gateway: unknown op code: {}", op);
                    }
                }
            }
            Some(Ok(Message::Ping(payload))) => {
                debug!("QQ Gateway: received PING, sending PONG");
                let mut sender = ws_sender_clone.lock().await;
                if let Err(e) = sender.send(Message::Pong(payload)).await {
                    warn!("QQ Gateway: failed to send PONG: {}", e);
                }
            }
            Some(Ok(Message::Close(_))) => {
                info!("QQ Gateway: received close frame");
                heartbeat_task.abort();
                return Ok(());
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => {
                warn!("QQ Gateway: WebSocket read error: {}", e);
                heartbeat_task.abort();
                return Err(ChannelError::ConnectionError(format!(
                    "WebSocket read error: {}",
                    e
                )));
            }
            None => {
                info!("QQ Gateway: WebSocket stream ended");
                heartbeat_task.abort();
                return Ok(());
            }
        }
    }
}

/// 解析并处理 Gateway 事件（op=0）
async fn handle_gateway_event(
    handler: Arc<dyn MessageHandler>,
    payload: &serde_json::Value,
) -> std::result::Result<(), ChannelError> {
    let event_type = payload["t"].as_str().unwrap_or("");

    debug!("QQ Gateway: event_type={}", event_type);

    match event_type {
        "READY" => {
            info!("QQ Gateway: READY event received, session established");
        }
        "RESUMED" => {
            info!("QQ Gateway: RESUMED event received");
        }
        "C2C_MESSAGE_CREATE" | "C2C_MESSAGE_CREATE_WITH_INTENT" => {
            handle_c2c_message(handler, payload).await?;
        }
        "GROUP_AT_MESSAGE_CREATE" | "AT_MESSAGE_CREATE" => {
            handle_group_at_message(handler, payload).await?;
        }
        _ => {
            // 忽略其他事件
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
    // QQ Bot v2 API 中群聊 ID 字段为 group_openid，兼容旧格式 group.id
    let group_id = data["group_openid"]
        .as_str()
        .or_else(|| data["group"]["id"].as_str())
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
            // 使用 chars() 安全截断 UTF-8 字符串
            let text_preview: String = outbound.text.chars().take(50).collect();
            info!(
                "Handler returned outbound: to={}, text={}",
                outbound.to,
                if outbound.text.len() > text_preview.len() {
                    format!("{}...", text_preview)
                } else {
                    text_preview
                }
            );

            // 发送回复到 QQ
            if let Err(e) = handler.reply(outbound).await {
                warn!("Failed to send reply: {:?}", e);
            }

            Ok(())
        }
        Err(e) => {
            warn!("Handler error: {:?}", e);
            Err(ChannelError::Other(format!("Handler error: {:?}", e)))
        }
    }
}
