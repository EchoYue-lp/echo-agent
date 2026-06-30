//! QQ Bot WebSocket Gateway — receive messages.
//!
//! Connects to the official QQ WebSocket Gateway, parses events, and forwards them to MessageHandler.
//!
//! QQ Bot WebSocket protocol:
//! 1. After connecting, receive a HELLO event (contains heartbeat_interval)
//! 2. Send IDENTIFY (contains token, intents, etc.)
//! 3. Periodically send HEARTBEAT
//! 4. Receive message events (C2C_MESSAGE_CREATE, GROUP_AT_MESSAGE_CREATE, etc.)

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

/// QQ Bot WebSocket opcodes (Discord Gateway protocol)
const OP_DISPATCH: u32 = 0; // Event message
const OP_HEARTBEAT: u32 = 1; // Heartbeat request (client sends)
const OP_IDENTIFY: u32 = 2; // Authentication (client sends)
#[allow(dead_code)]
const OP_RESUME: u32 = 6; // Resume session (client sends) - used for reconnection
const OP_RECONNECT: u32 = 7; // Reconnect request (server sends)
const OP_HELLO: u32 = 10; // Hello (server sends, contains heartbeat interval)
const OP_HEARTBEAT_ACK: u32 = 11; // Heartbeat acknowledgment (server sends)

/// QQ Bot intents (event subscription bitmask)
/// Reference: https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/interface-framework/event-emit.html
/// Important: GROUP_AND_C2C_EVENT (1<<25) contains QQ direct and group @message events
const INTENT_GUILDS: u32 = 1 << 0; // Guild events (default permission)
const INTENT_GUILD_MEMBERS: u32 = 1 << 1; // Guild member events (default permission)
#[allow(dead_code)]
const INTENT_GUILD_MESSAGES: u32 = 1 << 9; // Guild message events (private-domain bot)
#[allow(dead_code)]
const INTENT_GUILD_MESSAGE_REACTIONS: u32 = 1 << 10; // Guild message reactions
#[allow(dead_code)]
const INTENT_DIRECT_MESSAGE: u32 = 1 << 12; // Guild direct message events
const INTENT_GROUP_AND_C2C_EVENT: u32 = 1 << 25; // QQ direct + group chat events (critical! includes C2C_MESSAGE_CREATE)
#[allow(dead_code)]
const INTENT_PUBLIC_GUILD_MESSAGES: u32 = 1 << 30; // Public guild message events (guild @bot, default permission)

pub(super) async fn connect_to_gateway(
    wss_url: String,
    handler: Arc<dyn MessageHandler>,
    token: String,
) -> std::result::Result<(), ChannelError> {
    let (ws_stream, _) = connect_async(wss_url.clone())
        .await
        .map_err(|e| ChannelError::ConnectionError(format!("WebSocket connect failed: {}", e)))?;

    info!("QQ Gateway: WebSocket connected");

    // Split WebSocket into sender and receiver
    let (ws_sender, mut ws_receiver) = ws_stream.split();

    // Shared state
    let last_seq = Arc::new(Mutex::new(0u64));
    let heartbeat_interval = Arc::new(Mutex::new(Duration::from_secs(30)));
    let identified = Arc::new(Mutex::new(false));

    // Start heartbeat task
    let last_seq_clone = last_seq.clone();
    let heartbeat_interval_clone = heartbeat_interval.clone();
    let identified_clone = identified.clone();
    let ws_sender_clone = Arc::new(Mutex::new(ws_sender));
    let ws_sender_for_heartbeat = ws_sender_clone.clone();

    let heartbeat_task = tokio::spawn(async move {
        // Wait for IDENTIFY to complete
        while !*identified_clone.lock().await {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Start heartbeat loop
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

    // Process message loop
    loop {
        match ws_receiver.next().await {
            Some(Ok(Message::Text(text))) => {
                debug!(
                    "QQ Gateway: received message: {}",
                    if text.chars().count() > 200 {
                        let truncated: String = text.chars().take(200).collect();
                        truncated
                    } else {
                        text.clone()
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
                        // HELLO: get heartbeat interval and send IDENTIFY
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

                        // Send IDENTIFY
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
                        // Event message (op=0 means dispatch event)
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

/// Parse and handle Gateway events (op=0)
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
            // Ignore other events
        }
    }

    Ok(())
}

/// Handle direct message
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

/// Handle group @message
async fn handle_group_at_message(
    handler: Arc<dyn MessageHandler>,
    payload: &serde_json::Value,
) -> std::result::Result<(), ChannelError> {
    let data = &payload["d"];

    let sender_id = data["author"]["id"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    // In QQ Bot v2 API the group chat ID field is group_openid; also compatible with legacy format group.id
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

/// Unified dispatch to Handler and send reply
async fn dispatch_to_handler(
    handler: Arc<dyn MessageHandler>,
    inbound: InboundMessage,
) -> std::result::Result<(), ChannelError> {
    match handler.handle(inbound).await {
        Ok(outbound) => {
            // Safely truncate UTF-8 string using chars()
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

            // Send reply to QQ
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
