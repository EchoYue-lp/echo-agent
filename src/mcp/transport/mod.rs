pub mod http;
pub mod stdio;

use async_trait::async_trait;

use crate::error::Result;
use crate::mcp::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

/// MCP 传输层抽象
///
/// 负责在 Client 和 Server 之间传递 JSON-RPC 消息，
/// 屏蔽底层通信细节（进程 stdin/stdout 或 HTTP）。
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// 发送请求并等待响应（传输层自动管理请求 ID）
    async fn send(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse>;

    /// 发送通知（无需等待响应）
    async fn notify(&self, notification: JsonRpcNotification) -> Result<()>;

    /// 关闭传输层连接
    async fn close(&self);
}
