//! Feishu WebSocket Long Connection
//!
//! Implements the Feishu long connection protocol (PBBP2), no public IP required.
//!
//! Flow:
//! 1. HTTP get WebSocket URL (includes device_id and service_id)
//! 2. Connect WebSocket, receive binary Protobuf frames
//! 3. Handle control frames (ping/pong) and data frames (events)
//! 4. Large messages are auto-fragmented, requires reassembly
//! 5. Return response frame after processing events

use super::super::super::types::*;
use super::api::{ClientConfig, get_ws_endpoint, http_client};
use super::proto::*;
use dashmap::DashMap;
use echo_core::error::{ChannelError, Result};
use futures::SinkExt;
use futures::StreamExt;
use futures::stream::{SplitSink, SplitStream};
use rand::Rng;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

/// Processed event cache TTL (seconds) — duplicate events within 5 minutes are dropped
const DEDUP_TTL_SECS: u64 = 300;

/// Default ping interval (seconds)
const DEFAULT_PING_INTERVAL: u64 = 120;

/// Default reconnect interval (seconds)
const DEFAULT_RECONNECT_INTERVAL: u64 = 120;

/// Default initial reconnect jitter (seconds)
const DEFAULT_RECONNECT_NONCE: u64 = 30;

/// Default reconnect count (-1 means unlimited)
const DEFAULT_RECONNECT_COUNT: i32 = -1;

/// Exponential backoff max delay (seconds)
const MAX_BACKOFF_SECS: u64 = 300;

/// Stable connection threshold (seconds) — exceeding this time means the connection is stable, reset backoff
const STABLE_THRESHOLD_SECS: u64 = 60;
const MAX_FRAGMENT_COUNT: usize = 256;
const MAX_FRAGMENT_BYTES: usize = 2 * 1024 * 1024;

// Type aliases to simplify complex type signatures
type WsConn =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsSink = SplitSink<WsConn, Message>;
type WsRead = SplitStream<WsConn>;
type SharedSink = Arc<Mutex<WsSink>>;
type FragmentParts = Vec<Option<Vec<u8>>>;
type FragmentCacheEntry = (Instant, FragmentParts);
type FragmentCache = Arc<DashMap<String, FragmentCacheEntry>>;

// ── WebSocket Client ──────────────────────────────────────────────────────────

/// WebSocket long connection client configuration
pub struct WsClientConfig {
    pub app_id: String,
    pub app_secret: String,
    pub domain: String,
    pub auto_reconnect: bool,
    pub ping_interval: Duration,
    pub reconnect_interval: Duration,
    pub reconnect_nonce: Duration,
    pub reconnect_count: i32,
}

impl WsClientConfig {
    pub fn new(app_id: String, app_secret: String, domain: String) -> Self {
        Self {
            app_id,
            app_secret,
            domain,
            auto_reconnect: true,
            ping_interval: Duration::from_secs(DEFAULT_PING_INTERVAL),
            reconnect_interval: Duration::from_secs(DEFAULT_RECONNECT_INTERVAL),
            reconnect_nonce: Duration::from_secs(DEFAULT_RECONNECT_NONCE),
            reconnect_count: DEFAULT_RECONNECT_COUNT,
        }
    }

    /// Update from server-side configuration
    pub fn update_from_server(&mut self, config: &ClientConfig) {
        if let Some(v) = config.ping_interval {
            self.ping_interval = Duration::from_secs(v as u64);
        }
        if let Some(v) = config.reconnect_interval {
            self.reconnect_interval = Duration::from_secs(v as u64);
        }
        if let Some(v) = config.reconnect_nonce {
            self.reconnect_nonce = Duration::from_secs(v as u64);
        }
        if let Some(v) = config.reconnect_count {
            self.reconnect_count = v;
        }
    }
}

/// WebSocket long connection client
pub struct WsClient {
    config: WsClientConfig,
    http: reqwest::Client,
    /// Connection ID (parsed from URL)
    conn_id: String,
    /// Service ID (used when sending frames)
    service_id: i32,
    /// Message fragment cache (msg_id -> (creation time, fragments))
    fragment_cache: FragmentCache,
    /// Processed event dedup cache (event message_id -> processing time)
    processed_events: Arc<DashMap<String, Instant>>,
    event_locks: Arc<DashMap<String, Arc<Mutex<()>>>>,
    /// Running flag
    running: Arc<Mutex<bool>>,
}

impl WsClient {
    pub fn new(config: WsClientConfig) -> Self {
        Self {
            config,
            http: http_client(),
            conn_id: String::new(),
            service_id: 0,
            fragment_cache: Arc::new(DashMap::new()),
            processed_events: Arc::new(DashMap::new()),
            event_locks: Arc::new(DashMap::new()),
            running: Arc::new(Mutex::new(false)),
        }
    }

    /// Start the client (blocking)
    pub async fn run(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()> {
        *self.running.lock().await = true;

        let mut backoff_secs: u64 = 1;
        let mut attempts_since_stable: u32 = 0;

        loop {
            let connected_at = Instant::now();
            let connect_result = self.connect(handler.clone()).await;

            match connect_result {
                Ok(()) => {
                    info!("Feishu WebSocket: connection closed normally");
                }
                Err(e) => {
                    warn!("Feishu WebSocket: connection error: {:?}", e);
                }
            }

            if !*self.running.lock().await {
                info!("Feishu WebSocket: stopping, no reconnect");
                break;
            }

            if !self.config.auto_reconnect {
                info!("Feishu WebSocket: auto_reconnect disabled, stopping");
                break;
            }

            // Determine if the connection is stable, decide whether to reset backoff
            let stable = connected_at.elapsed().as_secs() >= STABLE_THRESHOLD_SECS;
            if stable {
                backoff_secs = 1;
                attempts_since_stable = 0;
            } else {
                attempts_since_stable += 1;
            }

            // Reconnect logic
            if self.config.reconnect_count >= 0 {
                let max_attempts = self.config.reconnect_count as u32;
                if attempts_since_stable > max_attempts {
                    error!(
                        "Feishu WebSocket: failed to reconnect after {} attempts",
                        self.config.reconnect_count
                    );
                    break;
                }
            }

            // Exponential backoff wait
            self.do_backoff_sleep(backoff_secs).await;

            // Update backoff delay (exponential growth, capped at MAX_BACKOFF_SECS)
            backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
        }

        Ok(())
    }

    /// Exponential backoff wait (with random jitter on first attempt)
    async fn do_backoff_sleep(&self, delay_secs: u64) {
        if delay_secs <= 1 {
            let jitter = rand_jitter(self.config.reconnect_nonce);
            if jitter > Duration::ZERO {
                tokio::time::sleep(jitter).await;
            }
        } else {
            info!(
                "Feishu WebSocket: backing off for {}s before reconnect",
                delay_secs
            );
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;
        }
    }

    /// Stop the client
    pub async fn stop(&mut self) {
        *self.running.lock().await = false;
    }

    /// Establish connection and process messages
    async fn connect(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()> {
        // 1. Get WebSocket URL
        let (ws_url, service_id_str, client_config) = get_ws_endpoint(
            &self.http,
            &self.config.domain,
            &self.config.app_id,
            &self.config.app_secret,
        )
        .await?;

        if let Some(config) = client_config {
            self.config.update_from_server(&config);
        }

        self.service_id = service_id_str.parse().unwrap_or(0);

        let parsed_url = url::Url::parse(&ws_url).map_err(|error| {
            ChannelError::InvalidConfig(format!("Invalid Feishu WebSocket URL: {error}"))
        })?;
        self.conn_id = parsed_url
            .query_pairs()
            .find(|(k, _)| k == "device_id")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();

        info!("Feishu WebSocket: connecting to {}", ws_url);

        // 2. Establish WebSocket connection, split read/write halves
        let (ws_stream, _) = connect_async(&ws_url).await.map_err(|e| {
            ChannelError::ConnectionError(format!("WebSocket connect failed: {}", e))
        })?;

        info!(
            "Feishu WebSocket: connected (conn_id={}, service_id={})",
            self.conn_id, self.service_id
        );

        let (write, read) = ws_stream.split();
        let ws_sink: SharedSink = Arc::new(Mutex::new(write));

        // 3. Start heartbeat task (actually sends ping frames)
        let ping_interval = self.config.ping_interval;
        let service_id = self.service_id;
        let running = self.running.clone();
        let sink_for_ping = ws_sink.clone();

        let ping_task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(ping_interval).await;
                if !*running.lock().await {
                    break;
                }
                let ping_bytes = ProtoFrame::ping(service_id).encode_to_vec();
                let mut sink = sink_for_ping.lock().await;
                if let Err(e) = sink.send(Message::Binary(ping_bytes)).await {
                    warn!("Feishu WebSocket: ping failed: {}", e);
                    break;
                }
                debug!("Feishu WebSocket: ping sent");
            }
        });

        // 4. Message processing loop
        let result = self.message_loop(read, ws_sink, handler).await;
        ping_task.abort();
        result
    }

    /// Message processing loop
    async fn message_loop(
        &mut self,
        mut stream: WsRead,
        sink: SharedSink,
        handler: Arc<dyn MessageHandler>,
    ) -> Result<()> {
        loop {
            match stream.next().await {
                Some(Ok(Message::Binary(bytes))) => {
                    self.handle_binary_frame(&sink, bytes, handler.clone())
                        .await?;
                }
                Some(Ok(Message::Ping(_))) => {
                    debug!("Feishu WebSocket: received raw ping");
                }
                Some(Ok(Message::Pong(_))) => {
                    debug!("Feishu WebSocket: received raw pong");
                }
                Some(Ok(Message::Close(_))) => {
                    info!("Feishu WebSocket: server closed connection");
                    return Ok(());
                }
                Some(Ok(Message::Text(text))) => {
                    warn!("Feishu WebSocket: unexpected text message: {}", text);
                }
                Some(Ok(Message::Frame(_))) => {}
                Some(Err(e)) => {
                    warn!("Feishu WebSocket: read error: {}", e);
                    return Err(
                        ChannelError::ConnectionError(format!("WebSocket error: {}", e)).into(),
                    );
                }
                None => {
                    info!("Feishu WebSocket: stream ended");
                    return Ok(());
                }
            }
        }
    }

    /// Handle binary Protobuf frame
    async fn handle_binary_frame(
        &mut self,
        sink: &SharedSink,
        bytes: Vec<u8>,
        handler: Arc<dyn MessageHandler>,
    ) -> Result<()> {
        let frame = ProtoFrame::decode_from_slice(&bytes).ok_or_else(|| {
            ChannelError::ConnectionError("Failed to decode Protobuf frame".to_string())
        })?;

        match frame.method {
            FRAME_TYPE_CONTROL => {
                self.handle_control_frame(&frame);
            }
            FRAME_TYPE_DATA => {
                self.handle_data_frame(sink, frame, handler).await?;
            }
            _ => {
                warn!("Feishu WebSocket: unknown frame method: {}", frame.method);
            }
        }

        Ok(())
    }

    /// Handle control frame (pong)
    fn handle_control_frame(&mut self, frame: &ProtoFrame) {
        let msg_type = frame.get_header(HEADER_TYPE).unwrap_or("");

        match msg_type {
            MESSAGE_TYPE_PONG => {
                debug!("Feishu WebSocket: received pong");
                if let Some(payload) = &frame.payload
                    && let Ok(config) = serde_json::from_slice::<ClientConfig>(payload)
                {
                    self.config.update_from_server(&config);
                    debug!("Feishu WebSocket: updated config from pong");
                }
            }
            MESSAGE_TYPE_PING => {
                debug!("Feishu WebSocket: received ping from server");
            }
            _ => {
                debug!("Feishu WebSocket: unknown control frame type: {}", msg_type);
            }
        }
    }

    /// Handle data frame (event)
    async fn handle_data_frame(
        &mut self,
        sink: &SharedSink,
        frame: ProtoFrame,
        handler: Arc<dyn MessageHandler>,
    ) -> Result<()> {
        let msg_type = frame.get_header(HEADER_TYPE).unwrap_or("");
        let msg_id = frame.get_header(HEADER_MESSAGE_ID).unwrap_or("");
        let sum = frame.get_header_int(HEADER_SUM);
        let seq = frame.get_header_int(HEADER_SEQ);

        // Fragment handling
        let payload = if sum > 1 {
            match self.combine_fragments(msg_id, sum, seq, frame.payload.clone()) {
                Some(p) => p,
                None => return Ok(()),
            }
        } else {
            frame.payload.clone()
        };

        let payload = match payload {
            Some(p) => p,
            None => return Ok(()),
        };

        let payload_str = String::from_utf8_lossy(&payload);

        info!(
            "[V3] Feishu WebSocket: received data frame, msg_id={}, type={}",
            msg_id, msg_type
        );

        // Process first: success acknowledgement must mean recoverable work completed.
        if msg_type == MESSAGE_TYPE_EVENT {
            // Extract the Feishu event's message_id from payload for dedup
            let event_message_id = Self::extract_event_message_id(&payload_str);

            let event_lock = event_message_id.as_ref().map(|event_mid| {
                self.event_locks
                    .entry(event_mid.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(())))
                    .clone()
            });
            let _event_guard = if let Some(lock) = &event_lock {
                Some(lock.lock().await)
            } else {
                None
            };

            // Dedup: check if this event has already been processed
            if let Some(ref event_mid) = event_message_id
                && self.processed_events.contains_key(event_mid)
            {
                info!(
                    "[V3] Feishu WebSocket: duplicate event detected, message_id={}, skipping",
                    event_mid
                );
                return Ok(());
            }

            // Periodically clean up expired dedup cache entries
            self.cleanup_processed_events();

            if let Err(error) = Self::process_event_async(payload_str.to_string(), handler).await {
                if let Some(event_mid) = &event_message_id {
                    self.event_locks.remove(event_mid);
                }
                return Err(error);
            }
            if let Some(event_mid) = event_message_id {
                self.processed_events
                    .insert(event_mid.clone(), Instant::now());
                self.event_locks.remove(&event_mid);
            }
        }

        self.send_response_frame(sink, frame, true).await?;

        Ok(())
    }

    /// Extract the Feishu message's message_id from event payload (for dedup)
    fn extract_event_message_id(payload: &str) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(payload).ok()?;
        v["event"]["message"]["message_id"]
            .as_str()
            .or_else(|| v["header"]["event_id"].as_str())
            .map(|s| s.to_string())
    }

    /// Clean up expired dedup cache entries
    fn cleanup_processed_events(&self) {
        let ttl = Duration::from_secs(DEDUP_TTL_SECS);
        self.processed_events.retain(|_, v| v.elapsed() < ttl);
    }

    /// Reassemble fragmented messages (with TTL to prevent memory leak)
    fn combine_fragments(
        &self,
        msg_id: &str,
        sum: i32,
        seq: i32,
        payload: Option<Vec<u8>>,
    ) -> Option<Option<Vec<u8>>> {
        let count = usize::try_from(sum).ok()?;
        let index = usize::try_from(seq).ok()?;
        if msg_id.is_empty()
            || count == 0
            || count > MAX_FRAGMENT_COUNT
            || index >= count
            || payload
                .as_ref()
                .is_some_and(|part| part.len() > MAX_FRAGMENT_BYTES)
        {
            return None;
        }
        let cache_key = msg_id.to_string();

        // Clean up timed-out incomplete fragments (5 minutes without completion is considered lost)
        let ttl = Duration::from_secs(DEDUP_TTL_SECS);
        self.fragment_cache
            .retain(|_, (created, _)| created.elapsed() < ttl);

        // A message id is expected to be stable, but malformed or retried
        // frames can reuse it with a different fragment count. Replace the
        // entry when the declared shape changes; otherwise an old, shorter
        // vector would make the new sequence index out of bounds.
        let shape_changed = self
            .fragment_cache
            .get(&cache_key)
            .is_some_and(|entry| entry.value().1.len() != count);
        if shape_changed {
            self.fragment_cache.remove(&cache_key);
        }

        // Initialize cache
        if !self.fragment_cache.contains_key(&cache_key) {
            let fragments: Vec<Option<Vec<u8>>> = vec![None; count];
            self.fragment_cache
                .insert(cache_key.clone(), (Instant::now(), fragments));
        }

        // Update fragment
        if let Some(mut entry) = self.fragment_cache.get_mut(&cache_key) {
            let (_, fragments) = entry.value_mut();
            let current_bytes = fragments
                .iter()
                .filter_map(Option::as_ref)
                .fold(0usize, |total, part| total.saturating_add(part.len()));
            let incoming_bytes = payload.as_ref().map_or(0, Vec::len);
            if current_bytes.saturating_add(incoming_bytes) > MAX_FRAGMENT_BYTES {
                drop(entry);
                self.fragment_cache.remove(&cache_key);
                return None;
            }
            let Some(fragment) = fragments.get_mut(index) else {
                // The entry may have been concurrently replaced after the
                // shape check. Treat this frame as incomplete and discard the
                // malformed cache entry instead of panicking.
                drop(entry);
                self.fragment_cache.remove(&cache_key);
                return None;
            };
            *fragment = payload;

            let all_received = fragments.iter().all(|f| f.is_some());
            if all_received {
                let combined: Vec<u8> = fragments
                    .iter()
                    .filter_map(Option::as_ref)
                    .flatten()
                    .copied()
                    .collect();
                drop(entry);
                self.fragment_cache.remove(&cache_key);
                return Some(Some(combined));
            }
        }

        None
    }

    /// Process event asynchronously
    async fn process_event_async(payload: String, handler: Arc<dyn MessageHandler>) -> Result<()> {
        let event: serde_json::Value = serde_json::from_str(&payload).map_err(|e| {
            ChannelError::ConnectionError(format!("Failed to parse event JSON: {}", e))
        })?;

        let event_type = event["header"]["event_type"].as_str().unwrap_or("");

        debug!(
            "Feishu WebSocket: async processing event type: {}",
            event_type
        );

        if event_type == "im.message.receive_v1" {
            Self::process_im_message(event, handler).await?;
        }

        Ok(())
    }

    /// Handle IM message event
    async fn process_im_message(
        event: serde_json::Value,
        handler: Arc<dyn MessageHandler>,
    ) -> Result<()> {
        let message = &event["event"]["message"];
        let sender = &event["event"]["sender"];

        let sender_id = sender["sender_id"]["open_id"]
            .as_str()
            .or_else(|| sender["sender_id"]["user_id"].as_str())
            .unwrap_or("unknown")
            .to_string();

        let chat_id = message["chat_id"].as_str().unwrap_or("").to_string();
        let chat_type_str = message["chat_type"].as_str().unwrap_or("p2p");
        let message_id = message["message_id"].as_str().unwrap_or("").to_string();

        let chat_type = if chat_type_str == "group" {
            ChatType::Group
        } else {
            ChatType::Direct
        };

        let msg_type = message["message_type"].as_str().unwrap_or("text");
        let content_str = message["content"].as_str().unwrap_or("{}");
        let text = Self::parse_message_content_static(msg_type, content_str);

        if text.is_empty() {
            debug!("Feishu WebSocket: empty text, ignoring");
            return Ok(());
        }

        info!(
            "[V3] Feishu WebSocket: processing message from {} in {}: {}",
            sender_id,
            chat_id,
            if text.chars().count() > 100 {
                let truncated: String = text.chars().take(100).collect();
                truncated
            } else {
                text.clone()
            }
        );

        let inbound =
            InboundMessage::new("feishu", sender_id, chat_id, chat_type, text, message_id);

        match handler.handle(inbound).await {
            Ok(outbound) => {
                if let Err(e) = handler.reply(outbound).await {
                    warn!("Feishu WebSocket: failed to send reply: {:?}", e);
                }
            }
            Err(e) => {
                warn!("Feishu WebSocket: handler error: {:?}", e);
            }
        }

        Ok(())
    }

    fn parse_message_content_static(msg_type: &str, content_str: &str) -> String {
        let content: serde_json::Value = serde_json::from_str(content_str).unwrap_or_default();

        match msg_type {
            "text" => content["text"].as_str().unwrap_or("").to_string(),
            "post" => Self::parse_rich_text_static(&content),
            "interactive" => String::new(),
            "image" => "[image]".to_string(),
            "file" => "[file]".to_string(),
            _ => content["text"].as_str().unwrap_or("").to_string(),
        }
    }

    fn parse_rich_text_static(content: &serde_json::Value) -> String {
        let paragraphs: Vec<String> = content["content"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|para| {
                        if let Some(elements) = para.as_array() {
                            let text_parts: Vec<String> = elements
                                .iter()
                                .filter_map(|elem| {
                                    let tag = elem["tag"].as_str().unwrap_or("");
                                    match tag {
                                        "text" | "at" => elem["text"].as_str().map(String::from),
                                        "img" => Some("[image]".to_string()),
                                        "file" | "media" => Some("[file]".to_string()),
                                        _ => None,
                                    }
                                })
                                .collect();
                            if text_parts.is_empty() {
                                None
                            } else {
                                Some(text_parts.join(" "))
                            }
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        paragraphs.join("\n\n")
    }

    /// Send response frame
    async fn send_response_frame(
        &mut self,
        sink: &SharedSink,
        original_frame: ProtoFrame,
        success: bool,
    ) -> Result<()> {
        let msg_id = original_frame
            .get_header(HEADER_MESSAGE_ID)
            .unwrap_or("")
            .to_string();

        let mut response_frame = original_frame.clone();
        let code = if success { 0 } else { 500 };
        let response_json = serde_json::json!({ "code": code });
        response_frame.payload = Some(response_json.to_string().into_bytes());
        response_frame.payload_encoding = Some("json".to_string());

        let frame_bytes = response_frame.encode_to_vec();

        let mut ws = sink.lock().await;
        ws.send(Message::Binary(frame_bytes))
            .await
            .map_err(|e| ChannelError::SendError(format!("Failed to send response: {}", e)))?;

        info!(
            "[V3] Feishu WebSocket: response sent immediately for msg_id={}",
            msg_id
        );
        Ok(())
    }
}

/// Random jitter
fn rand_jitter(max: Duration) -> Duration {
    let max_ms = max.as_millis() as u64;
    if max_ms == 0 {
        return Duration::ZERO;
    }
    let jitter_ms = rand::rng().random_range(0..max_ms);
    Duration::from_millis(jitter_ms)
}

#[cfg(test)]
mod tests {
    use super::{WsClient, WsClientConfig};

    fn client() -> WsClient {
        WsClient::new(WsClientConfig::new(
            "app-id".to_string(),
            "app-secret".to_string(),
            "wss://example.test".to_string(),
        ))
    }

    #[test]
    fn fragment_count_change_replaces_cached_shape_without_panicking() {
        let client = client();

        assert_eq!(
            client.combine_fragments("same-id", 2, 0, Some(b"old".to_vec())),
            None
        );

        // The same id now declares three parts. This used to index the old
        // two-element vector with seq=2 and panic.
        assert_eq!(
            client.combine_fragments("same-id", 3, 2, Some(b"c".to_vec())),
            None
        );
        assert_eq!(
            client.combine_fragments("same-id", 3, 0, Some(b"a".to_vec())),
            None
        );
        assert_eq!(
            client.combine_fragments("same-id", 3, 1, Some(b"b".to_vec())),
            Some(Some(b"abc".to_vec()))
        );
    }
}
