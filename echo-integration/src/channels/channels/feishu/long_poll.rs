//! Feishu WebSocket Long Connection
//!
//! 实现飞书长连接协议（PBBP2），无需公网 IP。
//!
//! 流程：
//! 1. HTTP 获取 WebSocket URL（包含 device_id 和 service_id）
//! 2. 连接 WebSocket，接收二进制 Protobuf Frame
//! 3. 处理控制帧（ping/pong）和数据帧（事件）
//! 4. 大消息自动分片，需合包处理
//! 5. 处理完事件后返回响应 Frame

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

/// 已处理事件缓存 TTL（秒）—— 5 分钟内的重复事件会被丢弃
const DEDUP_TTL_SECS: u64 = 300;

/// 默认 Ping 间隔（秒）
const DEFAULT_PING_INTERVAL: u64 = 120;

/// 默认重连间隔（秒）
const DEFAULT_RECONNECT_INTERVAL: u64 = 120;

/// 默认首次重连抖动（秒）
const DEFAULT_RECONNECT_NONCE: u64 = 30;

/// 默认重连次数（-1 表示无限）
const DEFAULT_RECONNECT_COUNT: i32 = -1;

/// 指数退避最大延迟（秒）
const MAX_BACKOFF_SECS: u64 = 300;

/// 连接稳定阈值（秒）—— 超过此时间视为连接稳定，重置退避
const STABLE_THRESHOLD_SECS: u64 = 60;

// 类型别名，简化复杂类型签名
type WsConn =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsSink = SplitSink<WsConn, Message>;
type WsRead = SplitStream<WsConn>;
type SharedSink = Arc<Mutex<WsSink>>;

// ── WebSocket Client ──────────────────────────────────────────────────────────

/// WebSocket 长连接客户端配置
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

    /// 从服务端配置更新
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

/// WebSocket 长连接客户端
pub struct WsClient {
    config: WsClientConfig,
    http: reqwest::Client,
    /// 连接 ID（从 URL 解析）
    conn_id: String,
    /// 服务 ID（用于发送帧）
    service_id: i32,
    /// 消息分片缓存（msg_id -> (创建时间, 各分片)）
    fragment_cache: Arc<DashMap<String, (Instant, Vec<Option<Vec<u8>>>)>>,
    /// 已处理事件去重缓存（event message_id -> 处理时间）
    processed_events: Arc<DashMap<String, Instant>>,
    /// 运行标志
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
            running: Arc::new(Mutex::new(false)),
        }
    }

    /// 启动客户端（阻塞运行）
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

            // 判断连接是否稳定，决定是否重置退避
            let stable = connected_at.elapsed().as_secs() >= STABLE_THRESHOLD_SECS;
            if stable {
                backoff_secs = 1;
                attempts_since_stable = 0;
            } else {
                attempts_since_stable += 1;
            }

            // 重连逻辑
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

            // 指数退避等待
            self.do_backoff_sleep(backoff_secs).await;

            // 更新退避延迟（指数增长，不超过 MAX_BACKOFF_SECS）
            backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
        }

        Ok(())
    }

    /// 指数退避等待（首次带随机抖动）
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

    /// 停止客户端
    pub async fn stop(&mut self) {
        *self.running.lock().await = false;
    }

    /// 建立连接并处理消息
    async fn connect(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()> {
        // 1. 获取 WebSocket URL
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

        let parsed_url = url::Url::parse(&ws_url).unwrap();
        self.conn_id = parsed_url
            .query_pairs()
            .find(|(k, _)| k == "device_id")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();

        info!("Feishu WebSocket: connecting to {}", ws_url);

        // 2. 建立 WebSocket 连接，分离读写端
        let (ws_stream, _) = connect_async(&ws_url).await.map_err(|e| {
            ChannelError::ConnectionError(format!("WebSocket connect failed: {}", e))
        })?;

        info!(
            "Feishu WebSocket: connected (conn_id={}, service_id={})",
            self.conn_id, self.service_id
        );

        let (write, read) = ws_stream.split();
        let ws_sink: SharedSink = Arc::new(Mutex::new(write));

        // 3. 启动心跳任务（真正发送 ping 帧）
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

        // 4. 消息处理循环
        let result = self.message_loop(read, ws_sink, handler).await;
        ping_task.abort();
        result
    }

    /// 消息处理循环
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

    /// 处理二进制 Protobuf Frame
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

    /// 处理控制帧（pong）
    fn handle_control_frame(&mut self, frame: &ProtoFrame) {
        let msg_type = frame.get_header(HEADER_TYPE).unwrap_or("");

        match msg_type {
            MESSAGE_TYPE_PONG => {
                debug!("Feishu WebSocket: received pong");
                if let Some(payload) = &frame.payload {
                    if let Ok(config) = serde_json::from_slice::<ClientConfig>(payload) {
                        self.config.update_from_server(&config);
                        debug!("Feishu WebSocket: updated config from pong");
                    }
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

    /// 处理数据帧（事件）
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

        // 分片处理
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

        // 【关键】立即发送响应帧（必须在 3 秒内，所以先响应再处理）
        self.send_response_frame(sink, frame.clone(), true).await?;

        // 然后异步处理消息
        if msg_type == MESSAGE_TYPE_EVENT {
            // 从 payload 中提取飞书事件的 message_id 用于去重
            let event_message_id = Self::extract_event_message_id(&payload_str);

            // 去重：检查该事件是否已处理过
            if let Some(ref event_mid) = event_message_id {
                if self.processed_events.contains_key(event_mid) {
                    info!(
                        "[V3] Feishu WebSocket: duplicate event detected, message_id={}, skipping",
                        event_mid
                    );
                    return Ok(());
                }
                self.processed_events
                    .insert(event_mid.clone(), Instant::now());
            }

            // 定期清理过期的去重缓存
            self.cleanup_processed_events();

            let handler_clone = handler.clone();
            let payload_owned = payload_str.to_string();
            tokio::spawn(async move {
                let start = chrono::Utc::now().timestamp_millis();
                if let Err(e) = Self::process_event_async(payload_owned, handler_clone).await {
                    warn!("[V3] Feishu WebSocket: async processing error: {:?}", e);
                }
                let end = chrono::Utc::now().timestamp_millis();
                info!(
                    "[V3] Feishu WebSocket: async processing done in {}ms",
                    end - start
                );
            });
        }

        Ok(())
    }

    /// 从事件 payload 中提取飞书消息的 message_id（用于去重）
    fn extract_event_message_id(payload: &str) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(payload).ok()?;
        v["event"]["message"]["message_id"]
            .as_str()
            .or_else(|| v["header"]["event_id"].as_str())
            .map(|s| s.to_string())
    }

    /// 清理过期的去重缓存
    fn cleanup_processed_events(&self) {
        let ttl = Duration::from_secs(DEDUP_TTL_SECS);
        self.processed_events.retain(|_, v| v.elapsed() < ttl);
    }

    /// 合包分片消息（带 TTL 防止内存泄漏）
    fn combine_fragments(
        &self,
        msg_id: &str,
        sum: i32,
        seq: i32,
        payload: Option<Vec<u8>>,
    ) -> Option<Option<Vec<u8>>> {
        let cache_key = msg_id.to_string();

        // 清理超时的残缺分片（5 分钟未完成视为丢失）
        let ttl = Duration::from_secs(DEDUP_TTL_SECS);
        self.fragment_cache
            .retain(|_, (created, _)| created.elapsed() < ttl);

        // 初始化缓存
        if !self.fragment_cache.contains_key(&cache_key) {
            let fragments: Vec<Option<Vec<u8>>> = vec![None; sum as usize];
            self.fragment_cache
                .insert(cache_key.clone(), (Instant::now(), fragments));
        }

        // 更新分片
        if let Some(mut entry) = self.fragment_cache.get_mut(&cache_key) {
            let (_, fragments) = entry.value_mut();
            if seq >= 0 && seq < sum {
                fragments[seq as usize] = payload;
            }

            let all_received = fragments.iter().all(|f| f.is_some());
            if all_received {
                let combined: Vec<u8> = fragments.iter().flat_map(|f| f.clone().unwrap()).collect();
                drop(entry);
                self.fragment_cache.remove(&cache_key);
                return Some(Some(combined));
            }
        }

        None
    }

    /// 异步处理事件
    async fn process_event_async(payload: String, handler: Arc<dyn MessageHandler>) -> Result<()> {
        let event: serde_json::Value = serde_json::from_str(&payload).map_err(|e| {
            ChannelError::ConnectionError(format!("Failed to parse event JSON: {}", e))
        })?;

        let event_type = event["header"]["event_type"].as_str().unwrap_or("");

        debug!(
            "Feishu WebSocket: async processing event type: {}",
            event_type
        );

        match event_type {
            "im.message.receive_v1" => {
                Self::process_im_message(event, handler).await?;
            }
            _ => {}
        }

        Ok(())
    }

    /// 处理 IM 消息事件
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
            if text.len() > 100 {
                &text[..100]
            } else {
                &text
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

    /// 发送响应帧
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

/// 随机抖动
fn rand_jitter(max: Duration) -> Duration {
    let max_ms = max.as_millis() as u64;
    if max_ms == 0 {
        return Duration::ZERO;
    }
    let jitter_ms = rand::rng().random_range(0..max_ms);
    Duration::from_millis(jitter_ms)
}
