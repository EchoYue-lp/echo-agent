//! 人工介入（Human-in-the-Loop）
//!
//! 在工具执行前拦截，向外部请求审批或文本输入。
//! 实现 [`HumanLoopProvider`] trait 可接入任意审批渠道。

mod console;
mod webhook;
mod websocket;

pub use console::ConsoleHumanLoopProvider;
pub use webhook::WebhookHumanLoopProvider;
pub use websocket::WebSocketHumanLoopProvider;

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Result;

// ── 请求 ─────────────────────────────────────────────────────────────────────

/// 人工介入的场景类型。
#[derive(Debug, Clone)]
pub enum HumanLoopKind {
    /// 工具守卫：需要用户对工具执行做批准 / 拒绝决策。
    Approval,
    /// 交互澄清：需要用户回复自由文本（意图确认、补充信息等）。
    Input,
}

/// 向人工发起的介入请求。
///
/// 使用 [`HumanLoopRequest::approval`] 或 [`HumanLoopRequest::input`] 构造，
/// 统一传入 [`HumanLoopProvider::request`]。
#[derive(Debug, Clone)]
pub struct HumanLoopRequest {
    /// 请求类型（审批 or 文本输入）
    pub kind: HumanLoopKind,
    /// 展示给用户的提示信息
    pub prompt: String,
    /// 工具名称（仅 Approval 场景有值）
    pub tool_name: Option<String>,
    /// 工具参数（仅 Approval 场景有值）
    pub args: Option<Value>,
}

impl HumanLoopRequest {
    /// 构造审批请求：请求用户对工具执行做批准 / 拒绝决策。
    pub fn approval(tool_name: impl Into<String>, args: Value) -> Self {
        let tool_name = tool_name.into();
        Self {
            kind: HumanLoopKind::Approval,
            prompt: format!("工具 [{tool_name}] 需要人工审批，是否批准执行？(y/n)"),
            tool_name: Some(tool_name),
            args: Some(args),
        }
    }

    /// 构造文本输入请求：请求用户回复自由文本。
    pub fn input(prompt: impl Into<String>) -> Self {
        Self {
            kind: HumanLoopKind::Input,
            prompt: prompt.into(),
            tool_name: None,
            args: None,
        }
    }
}

// ── 响应 ─────────────────────────────────────────────────────────────────────

/// 人工介入的响应结果。
#[derive(Debug, Clone)]
pub enum HumanLoopResponse {
    /// 用户批准（对应 Approval 请求）
    Approved,
    /// 用户拒绝（对应 Approval 请求）
    Rejected { reason: Option<String> },
    /// 用户输入的自由文本（对应 Input 请求）
    Text(String),
    /// 等待超时，未收到响应
    Timeout,
}

// ── Provider trait ────────────────────────────────────────────────────────────

/// 人工介入 Provider trait，统一管理审批与交互两种场景。
///
/// 内置实现：
/// - [`ConsoleHumanLoopProvider`]：命令行 stdin（异步，不阻塞 tokio 线程）
/// - [`WebhookHumanLoopProvider`]：HTTP 同步回调，向外部系统发送请求并等待决策
/// - [`WebSocketHumanLoopProvider`]：本地 WebSocket 服务器，向已连接客户端推送请求
#[async_trait]
pub trait HumanLoopProvider: Send + Sync {
    /// 发起人工介入请求，统一入口。
    ///
    /// - `Approval` 请求应返回 [`HumanLoopResponse::Approved`] / [`HumanLoopResponse::Rejected`] / [`HumanLoopResponse::Timeout`]
    /// - `Input` 请求应返回 [`HumanLoopResponse::Text`] / [`HumanLoopResponse::Timeout`]
    async fn request(&self, req: HumanLoopRequest) -> Result<HumanLoopResponse>;
}

/// 默认 Provider：使用命令行控制台。
pub fn default_provider() -> Arc<dyn HumanLoopProvider> {
    Arc::new(ConsoleHumanLoopProvider)
}

// ── Guard 管理器 ──────────────────────────────────────────────────────────────

/// 工具执行前的人工审批管理器（guard 模式）。
///
/// 通过 [`ReactAgent::add_need_appeal_tool`] 标记工具为"需要审批"，
/// 执行前会调用注入的 [`HumanLoopProvider`] 请求确认。
pub struct HumanApprovalManager {
    need_approval_tools: HashSet<String>,
}

impl HumanApprovalManager {
    pub fn new() -> Self {
        HumanApprovalManager {
            need_approval_tools: HashSet::new(),
        }
    }

    pub fn mark_need_approval(&mut self, tool_name: String) {
        self.need_approval_tools.insert(tool_name);
    }

    pub fn needs_approval(&self, tool_name: &str) -> bool {
        self.need_approval_tools.contains(tool_name)
    }
}

impl Default for HumanApprovalManager {
    fn default() -> Self {
        Self::new()
    }
}
