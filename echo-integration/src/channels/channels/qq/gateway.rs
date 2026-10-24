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
use crate::redaction::text_with_secrets;
use echo_core::agent::CancellationToken;
use echo_core::error::ChannelError;
use futures::SinkExt;
use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

/// QQ Bot WebSocket opcodes (Discord Gateway protocol)
const OP_DISPATCH: u32 = 0; // Event message
const OP_HEARTBEAT: u32 = 1; // Heartbeat request (client sends)
const OP_IDENTIFY: u32 = 2; // Authentication (client sends)
const OP_RECONNECT: u32 = 7; // Reconnect request (server sends)
const OP_HELLO: u32 = 10; // Hello (server sends, contains heartbeat interval)
const OP_HEARTBEAT_ACK: u32 = 11; // Heartbeat acknowledgment (server sends)

/// QQ Bot intents (event subscription bitmask)
/// Reference: https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/interface-framework/event-emit.html
/// Important: GROUP_AND_C2C_EVENT (1<<25) contains QQ direct and group @message events
const INTENT_GUILDS: u32 = 1 << 0; // Guild events (default permission)
const INTENT_GUILD_MEMBERS: u32 = 1 << 1; // Guild member events (default permission)
const INTENT_GROUP_AND_C2C_EVENT: u32 = 1 << 25; // QQ direct + group chat events (critical! includes C2C_MESSAGE_CREATE)

pub(super) async fn connect_to_gateway(
    wss_url: String,
    handler: Arc<dyn MessageHandler>,
    token: String,
    cancellation: CancellationToken,
    transport: TransportStatus,
) -> std::result::Result<(), ChannelError> {
    let connected = tokio::select! {
        biased;
        () = cancellation.cancelled() => return Ok(()),
        result = connect_async(wss_url.clone()) => result,
    };
    let (ws_stream, _) = connected.map_err(|e| {
        ChannelError::ConnectionError(format!(
            "WebSocket connect failed: {}",
            text_with_secrets(&e.to_string(), [token.as_str()])
        ))
    })?;

    info!("QQ Gateway: WebSocket connected");

    // Split WebSocket into sender and receiver
    let (ws_sender, mut ws_receiver) = ws_stream.split();

    // Shared state
    let last_seq = Arc::new(Mutex::new(0u64));
    let heartbeat_interval = Arc::new(Mutex::new(Duration::from_secs(30)));
    let identified = Arc::new(Mutex::new(false));

    // Drive heartbeat within this same transport task.
    let last_seq_clone = last_seq.clone();
    let heartbeat_interval_clone = heartbeat_interval.clone();
    let identified_clone = identified.clone();
    let ws_sender_clone = Arc::new(Mutex::new(ws_sender));
    let ws_sender_for_heartbeat = ws_sender_clone.clone();
    let token_for_heartbeat = token.clone();

    let heartbeat = async move {
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
                warn!(
                    "QQ Gateway: heartbeat failed: {}",
                    text_with_secrets(&e.to_string(), [token_for_heartbeat.as_str()])
                );
                break;
            }
            debug!("QQ Gateway: heartbeat sent (seq={})", seq);
        }
    };

    // Heartbeat is a sibling future in this original task, not a detached
    // spawn. Closing or dropping the connection cannot orphan another task.
    let messages = async {
        loop {
            let next = tokio::select! {
                biased;
                () = cancellation.cancelled() => return Ok(()),
                message = ws_receiver.next() => message,
            };
            match next {
                Some(Ok(Message::Text(text))) => {
                    let redacted = text_with_secrets(&text, [token.as_str()]);
                    debug!(
                        "QQ Gateway: received message: {}",
                        redacted.chars().take(200).collect::<String>()
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
                            if interval_ms == 0 {
                                return Err(ChannelError::ConnectionError(
                                    "QQ Gateway HELLO heartbeat_interval must be non-zero".into(),
                                ));
                            }

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
                                            text_with_secrets(&e.to_string(), [token.as_str()])
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
                            return Err(ChannelError::ConnectionError(
                                "Server requested reconnect".to_string(),
                            ));
                        }
                        OP_DISPATCH => {
                            // Event message (op=0 means dispatch event)
                            if *identified.lock().await {
                                observe_gateway_readiness(&transport, &payload);
                                let handled = tokio::select! {
                                    biased;
                                    () = cancellation.cancelled() => return Ok(()),
                                    result = handle_gateway_event(handler.clone(), &payload, &token) => result,
                                };
                                if let Err(e) = handled {
                                    warn!(
                                        "QQ Gateway: failed to handle event: {}",
                                        text_with_secrets(&e.to_string(), [token.as_str()])
                                    );
                                }
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
                        warn!(
                            "QQ Gateway: failed to send PONG: {}",
                            text_with_secrets(&e.to_string(), [token.as_str()])
                        );
                    }
                }
                Some(Ok(Message::Close(_))) => {
                    info!("QQ Gateway: received close frame");
                    return Ok(());
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    warn!(
                        "QQ Gateway: WebSocket read error: {}",
                        text_with_secrets(&e.to_string(), [token.as_str()])
                    );
                    return Err(ChannelError::ConnectionError(format!(
                        "WebSocket read error: {}",
                        text_with_secrets(&e.to_string(), [token.as_str()])
                    )));
                }
                None => {
                    info!("QQ Gateway: WebSocket stream ended");
                    return Ok(());
                }
            }
        }
    };
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Ok(()),
        result = messages => result,
        () = heartbeat => Err(ChannelError::ConnectionError("QQ heartbeat ended before the connection".into())),
    }
}

fn observe_gateway_readiness(transport: &TransportStatus, payload: &serde_json::Value) {
    let evidence = match payload.get("t").and_then(serde_json::Value::as_str) {
        Some("READY") => Some(ChannelTransportEvidence::QqSessionReady),
        Some("RESUMED") => Some(ChannelTransportEvidence::QqSessionResumed),
        _ => None,
    };
    if let Some(evidence) = evidence {
        transport.publish(ChannelTransportPhase::Ready, Some(evidence), None);
    }
}

/// Parse and handle Gateway events (op=0)
async fn handle_gateway_event(
    handler: Arc<dyn MessageHandler>,
    payload: &serde_json::Value,
    access_token: &str,
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
            handle_c2c_message(handler, payload, access_token).await?;
        }
        "GROUP_AT_MESSAGE_CREATE" | "AT_MESSAGE_CREATE" => {
            handle_group_at_message(handler, payload, access_token).await?;
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
    access_token: &str,
) -> std::result::Result<(), ChannelError> {
    let data = &payload["d"];

    let sender_id = require_sender_id(data["author"]["id"].as_str())?;
    let text = data["content"].as_str().unwrap_or("").to_string();
    let message_id = data["id"].as_str().unwrap_or("").to_string();

    if text.is_empty() {
        return Ok(());
    }

    let inbound = InboundMessage::new(
        "qqbot",
        sender_id,
        sender_id,
        ChatType::Direct,
        &text,
        &message_id,
    );

    dispatch_to_handler(handler, inbound, access_token).await
}

/// Handle group @message
async fn handle_group_at_message(
    handler: Arc<dyn MessageHandler>,
    payload: &serde_json::Value,
    access_token: &str,
) -> std::result::Result<(), ChannelError> {
    let data = &payload["d"];

    let sender_id = require_sender_id(data["author"]["id"].as_str())?;
    // In QQ Bot v2 API the group chat ID field is group_openid; also compatible with legacy format group.id
    let group_id = require_conversation_id(
        data["group_openid"]
            .as_str()
            .or_else(|| data["group"]["id"].as_str()),
    )?;
    let text = data["content"].as_str().unwrap_or("").to_string();
    let message_id = data["id"].as_str().unwrap_or("").to_string();

    if text.is_empty() {
        return Ok(());
    }

    let inbound = InboundMessage::new(
        "qqbot",
        sender_id,
        group_id,
        ChatType::Group,
        &text,
        &message_id,
    );

    dispatch_to_handler(handler, inbound, access_token).await
}

/// Unified dispatch to Handler and send reply
async fn dispatch_to_handler(
    handler: Arc<dyn MessageHandler>,
    inbound: InboundMessage,
    access_token: &str,
) -> std::result::Result<(), ChannelError> {
    match handler.handle(inbound).await {
        Ok(outbound) => {
            // Safely truncate UTF-8 string using chars()
            let redacted = text_with_secrets(&outbound.text, [access_token]);
            let mut characters = redacted.chars();
            let text_preview: String = characters.by_ref().take(50).collect();
            let truncated = characters.next().is_some();
            info!(
                "Handler returned outbound: to={}, text={}",
                outbound.to,
                if truncated {
                    format!("{}...", text_preview)
                } else {
                    text_preview
                }
            );

            // Send reply to QQ
            if let Err(e) = handler.reply(outbound).await {
                warn!(
                    "Failed to send reply: {}",
                    text_with_secrets(&e.to_string(), [access_token])
                );
            }

            Ok(())
        }
        Err(e) => {
            let error = text_with_secrets(&e.to_string(), [access_token]);
            warn!("Handler error: {}", error);
            Err(ChannelError::Other(format!("Handler error: {error}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingHandler {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl MessageHandler for CountingHandler {
        async fn handle(&self, msg: InboundMessage) -> echo_core::error::Result<OutboundMessage> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(OutboundMessage::new(
                &msg.channel_id,
                msg.reply_target(),
                msg.chat_type,
                "ok",
            ))
        }

        async fn reply(&self, _msg: OutboundMessage) -> echo_core::error::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn gateway_only_session_ready_and_resumed_observe_readiness() {
        let transport = TransportStatus::new(ChannelTransportPhase::Connecting);
        let receiver = transport.subscribe();
        for event in [
            json!({"op": 10}),
            json!({"t": "HELLO"}),
            json!({"t": "C2C_MESSAGE_CREATE"}),
        ] {
            observe_gateway_readiness(&transport, &event);
            assert_eq!(receiver.borrow().phase, ChannelTransportPhase::Connecting);
        }
        observe_gateway_readiness(&transport, &json!({"t": "READY"}));
        assert_eq!(
            receiver.borrow().evidence,
            Some(ChannelTransportEvidence::QqSessionReady)
        );
        observe_gateway_readiness(&transport, &json!({"t": "RESUMED"}));
        assert_eq!(
            receiver.borrow().evidence,
            Some(ChannelTransportEvidence::QqSessionResumed)
        );
    }

    #[tokio::test]
    async fn gateway_local_handshake_waits_for_qq_ready_and_cancellation_closes_original_socket()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let identified = Arc::new(tokio::sync::Notify::new());
        let ready = Arc::new(tokio::sync::Notify::new());
        let server_identified = identified.clone();
        let server_ready = ready.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            let mut websocket = tokio_tungstenite::accept_async(socket).await?;
            websocket
                .send(Message::Text(
                    json!({"op":10,"d":{"heartbeat_interval":1000}}).to_string(),
                ))
                .await?;
            let identify = websocket.next().await.ok_or("no IDENTIFY")??;
            let Message::Text(text) = identify else {
                return Err("not an IDENTIFY text frame".into());
            };
            let payload: serde_json::Value = serde_json::from_str(&text)?;
            assert_eq!(
                payload.get("op").and_then(serde_json::Value::as_u64),
                Some(2)
            );
            server_identified.notify_one();
            server_ready.notified().await;
            websocket
                .send(Message::Text(
                    json!({"op":0,"t":"READY","s":1,"d":{}}).to_string(),
                ))
                .await?;
            while let Some(frame) = websocket.next().await {
                // EOF or reset after owner cancellation both close this local socket.
                if frame.is_err() {
                    break;
                }
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        });
        let cancellation = CancellationToken::new();
        let cancel = cancellation.clone();
        let transport = TransportStatus::new(ChannelTransportPhase::Connecting);
        let mut receiver = transport.subscribe();
        let handler = Arc::new(CountingHandler {
            calls: Arc::new(AtomicUsize::new(0)),
        });
        let connection = tokio::spawn(connect_to_gateway(
            format!("ws://{address}"),
            handler,
            "fixture-token".into(),
            cancel,
            transport,
        ));
        tokio::time::timeout(Duration::from_secs(2), identified.notified()).await?;
        assert_eq!(receiver.borrow().phase, ChannelTransportPhase::Connecting);
        ready.notify_one();
        tokio::time::timeout(Duration::from_secs(2), receiver.changed()).await??;
        assert_eq!(receiver.borrow().phase, ChannelTransportPhase::Ready);
        assert_eq!(
            receiver.borrow().evidence,
            Some(ChannelTransportEvidence::QqSessionReady)
        );
        cancellation.cancel();
        tokio::time::timeout(Duration::from_secs(2), connection).await???;
        tokio::time::timeout(Duration::from_secs(2), server)
            .await??
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn gateway_local_zero_hello_interval_returns_error_without_panicking_or_becoming_ready()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            let mut websocket = tokio_tungstenite::accept_async(socket).await?;
            websocket
                .send(Message::Text(
                    json!({"op":10,"d":{"heartbeat_interval":0}}).to_string(),
                ))
                .await?;
            while let Some(frame) = websocket.next().await {
                if frame.is_err() {
                    break;
                }
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        });
        let transport = TransportStatus::new(ChannelTransportPhase::Connecting);
        let receiver = transport.subscribe();
        let handler = Arc::new(CountingHandler {
            calls: Arc::new(AtomicUsize::new(0)),
        });
        let connection = tokio::spawn(connect_to_gateway(
            format!("ws://{address}"),
            handler,
            "fixture-token".into(),
            CancellationToken::new(),
            transport,
        ));
        // The original task must return a normal typed protocol error, not a
        // JoinError panic from constructing Tokio's zero-period interval.
        let result = tokio::time::timeout(Duration::from_secs(2), connection).await??;
        assert!(
            matches!(result, Err(ChannelError::ConnectionError(message)) if message.contains("heartbeat_interval"))
        );
        assert_eq!(receiver.borrow().phase, ChannelTransportPhase::Connecting);
        tokio::time::timeout(Duration::from_secs(2), server)
            .await??
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn missing_or_unknown_sender_never_reaches_qq_handler() -> Result<(), String> {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler: Arc<dyn MessageHandler> = Arc::new(CountingHandler {
            calls: calls.clone(),
        });
        let missing = json!({"d": {"content": "hello", "id": "m1"}});
        let unknown = json!({
            "d": {
                "author": {"id": "unknown"},
                "group_openid": "group-1",
                "content": "hello",
                "id": "m2"
            }
        });

        let direct = handle_c2c_message(handler.clone(), &missing, "test-token").await;
        let group = handle_group_at_message(handler, &unknown, "test-token").await;
        if !matches!(direct, Err(ChannelError::Other(ref message)) if message.contains("sender_id"))
            || !matches!(group, Err(ChannelError::Other(ref message)) if message.contains("sender_id"))
        {
            return Err("QQ did not return typed sender identity errors".to_string());
        }
        if calls.load(Ordering::Acquire) != 0 {
            return Err("QQ forwarded a message without a stable sender".to_string());
        }
        Ok(())
    }
}
