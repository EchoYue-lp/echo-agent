pub mod http;
pub mod sse;
pub mod stdio;

use futures::future::BoxFuture;

use super::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use echo_core::error::Result;
use std::sync::Arc;

/// MCP 传输层抽象
///
/// 负责在 Client 和 Server 之间传递 JSON-RPC 消息，
/// 屏蔽底层通信细节（进程 stdin/stdout 或 HTTP）。
pub trait McpTransport: Send + Sync {
    /// 发送请求并等待响应（传输层自动管理请求 ID）
    fn send(&self, request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>>;

    /// 发送通知（无需等待响应）
    fn notify(&self, notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>>;

    /// 关闭传输层连接
    fn close(&self) -> BoxFuture<'_, ()>;

    /// 获取通知接收通道（用于接收服务端推送的通知）
    /// 返回 None 表示该传输层不支持通知接收
    fn notification_rx(&self) -> Option<Arc<dyn super::types::JsonRpcNotificationReceiver>>;
}
