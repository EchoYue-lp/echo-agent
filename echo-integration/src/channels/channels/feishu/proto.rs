//! Feishu WebSocket Protobuf Protocol
//!
//! Feishu long connection uses a custom Protobuf protocol (PBBP2), not plain WebSocket JSON.
//! This module implements Frame and Header encoding/decoding.
//!
//! Protocol reference: larksuite/oapi-sdk-go/ws/pbbp2.proto

use prost::Message;

/// Protobuf Header structure
#[derive(Clone, PartialEq, Message)]
pub struct ProtoHeader {
    #[prost(string, required, tag = "1")]
    pub key: String,

    #[prost(string, required, tag = "2")]
    pub value: String,
}

/// Protobuf Frame structure (Feishu WebSocket message frame)
#[derive(Clone, PartialEq, Message)]
pub struct ProtoFrame {
    /// Sequence number
    #[prost(uint64, required, tag = "1")]
    pub seq_id: u64,

    /// Log ID
    #[prost(uint64, required, tag = "2")]
    pub log_id: u64,

    /// Service ID (obtained from connection URL query parameter)
    #[prost(int32, required, tag = "3")]
    pub service: i32,

    /// Frame type: 0 = control frame (ping/pong), 1 = data frame (event)
    #[prost(int32, required, tag = "4")]
    pub method: i32,

    /// Headers list
    #[prost(message, repeated, tag = "5")]
    pub headers: Vec<ProtoHeader>,

    /// Payload encoding format (usually empty or "json")
    #[prost(string, optional, tag = "6")]
    pub payload_encoding: Option<String>,

    /// Payload type
    #[prost(string, optional, tag = "7")]
    pub payload_type: Option<String>,

    /// Payload data (event JSON or control frame config)
    #[prost(bytes, optional, tag = "8")]
    pub payload: Option<Vec<u8>>,

    /// New log ID (string format)
    #[prost(string, optional, tag = "9")]
    pub log_id_new: Option<String>,
}

// ── 常量定义 ─────────────────────────────────────────────────────────────────

/// Header Key: 时间戳（毫秒）
pub const HEADER_TIMESTAMP: &str = "timestamp";

/// Header Key: 消息类型（event/card/ping/pong）
pub const HEADER_TYPE: &str = "type";

/// Header Key: 消息 ID（拆包时继承）
pub const HEADER_MESSAGE_ID: &str = "message_id";

/// Header Key: 拆包总数（未拆包为 1）
pub const HEADER_SUM: &str = "sum";

/// Header Key: 包序号（未拆包为 0）
pub const HEADER_SEQ: &str = "seq";

/// Header Key: 链路追踪 ID
pub const HEADER_TRACE_ID: &str = "trace_id";

/// Header Key: 业务处理时长（毫秒）
pub const HEADER_BIZ_RT: &str = "biz_rt";

/// 帧类型：控制帧（ping/pong）
pub const FRAME_TYPE_CONTROL: i32 = 0;

/// 帧类型：数据帧（事件/卡片）
pub const FRAME_TYPE_DATA: i32 = 1;

/// 消息类型：事件
pub const MESSAGE_TYPE_EVENT: &str = "event";

/// 消息类型：卡片回调
pub const MESSAGE_TYPE_CARD: &str = "card";

/// 消息类型：Ping
pub const MESSAGE_TYPE_PING: &str = "ping";

/// 消息类型：Pong
pub const MESSAGE_TYPE_PONG: &str = "pong";

/// 错误码：成功
pub const CODE_OK: i32 = 0;

/// 错误码：系统繁忙
pub const CODE_SYSTEM_BUSY: i32 = 1;

/// 错误码：鉴权失败
pub const CODE_AUTH_FAILED: i32 = 514;

/// 错误码：超出连接限制（每个应用最多 50 个连接）
pub const CODE_EXCEED_CONN_LIMIT: i32 = 1000040350;

// ── Frame 辅助方法 ───────────────────────────────────────────────────────────

impl ProtoFrame {
    /// 创建一个新的 Frame（用于发送）
    pub fn new(seq_id: u64, service: i32, method: i32) -> Self {
        Self {
            seq_id,
            log_id: 0,
            service,
            method,
            headers: Vec::new(),
            payload_encoding: None,
            payload_type: None,
            payload: None,
            log_id_new: None,
        }
    }

    /// 添加 Header
    pub fn add_header(&mut self, key: &str, value: &str) {
        self.headers.push(ProtoHeader {
            key: key.to_string(),
            value: value.to_string(),
        });
    }

    /// 获取指定 Header 的值
    pub fn get_header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.key == key)
            .map(|h| h.value.as_str())
    }

    /// 获取指定 Header 的整数值
    pub fn get_header_int(&self, key: &str) -> i32 {
        self.get_header(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    /// 设置 Payload（JSON 字符串）
    pub fn set_payload_json(&mut self, json: &str) {
        self.payload_encoding = Some("json".to_string());
        self.payload = Some(json.as_bytes().to_vec());
    }

    /// 编码为字节
    pub fn encode_to_vec(&self) -> Vec<u8> {
        <Self as Message>::encode_to_vec(self)
    }

    /// 从字节解码
    pub fn decode_from_slice(bytes: &[u8]) -> Option<Self> {
        <Self as Message>::decode(bytes).ok()
    }

    /// 创建 Ping 帧
    pub fn ping(service: i32) -> Self {
        let mut frame = Self::new(0, service, FRAME_TYPE_CONTROL);
        frame.add_header(HEADER_TYPE, MESSAGE_TYPE_PING);
        frame
    }

    /// 创建响应帧（处理事件后回复）
    pub fn response(
        seq_id: u64,
        service: i32,
        code: i32,
        headers: Vec<ProtoHeader>,
        data: Option<Vec<u8>>,
    ) -> Self {
        let mut frame = Self::new(seq_id, service, FRAME_TYPE_DATA);
        frame.headers = headers;

        // 构建响应 JSON
        let response_json = serde_json::json!({
            "code": code,
            "headers": {},
            "data": data.unwrap_or_default()
        });
        frame.set_payload_json(&response_json.to_string());
        frame
    }
}

// ── Headers 辅助结构 ─────────────────────────────────────────────────────────

/// Headers 辅助操作
pub struct HeadersHelper;

impl HeadersHelper {
    /// 从 headers 列表中获取值
    pub fn get<'a>(headers: &'a [ProtoHeader], key: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|h| h.key == key)
            .map(|h| h.value.as_str())
    }

    /// 获取整数值
    pub fn get_int(headers: &[ProtoHeader], key: &str) -> i32 {
        Self::get(headers, key)
            .and_then(|v: &str| v.parse().ok())
            .unwrap_or(0)
    }

    /// 添加 header
    pub fn add(headers: &mut Vec<ProtoHeader>, key: &str, value: &str) {
        headers.push(ProtoHeader {
            key: key.to_string(),
            value: value.to_string(),
        });
    }
}
