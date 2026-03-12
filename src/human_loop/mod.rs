//! 人工介入（Human-in-the-Loop）
//!
//! 在工具执行前拦截，向外部请求审批或文本输入。
//!
//! ## 设计原则
//!
//! - **事件驱动**: 审批请求通过事件通知上层应用，而非直接阻塞
//! - **统一入口**: 用户输入和审批响应共用同一个输入通道
//! - **异步解耦**: Agent 执行与用户交互分离，支持复杂 UI 场景
//!
//! ## 使用示例
//!
//! ```rust,no_run
//! use echo_agent::human_loop::{HumanLoopEvent, HumanLoopManager, ApprovalDecision};
//! use echo_agent::prelude::*;
//! use std::sync::Arc;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! // 创建 manager
//! let manager = Arc::new(HumanLoopManager::new());
//!
//! // 在后台任务中监听事件
//! let mgr = manager.clone();
//! tokio::spawn(async move {
//!     while let Some(event) = mgr.recv_event().await {
//!         match event {
//!             HumanLoopEvent::ApprovalRequest { tool_name, args, responder, .. } => {
//!                 println!("工具 '{}' 需要审批", tool_name);
//!                 // 用户确认后响应
//!                 responder.respond(ApprovalDecision::Approved);
//!             }
//!             HumanLoopEvent::InputRequest { prompt, responder } => {
//!                 println!("Agent 询问: {}", prompt);
//!                 responder.respond("用户输入的内容".to_string());
//!             }
//!         }
//!     }
//! });
//!
//! // 将 manager 设置为 agent 的审批 provider
//! let config = AgentConfig::standard("qwen3-max", "assistant", "你是一个助手");
//! let mut agent = ReactAgent::new(config);
//! agent.set_human_loop_provider(manager);
//! # Ok(())
//! # }
//! ```

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
use tokio::sync::{mpsc, oneshot};

use crate::error::{ReactError, Result};

// ── 审批决策 ───────────────────────────────────────────────────────────────

/// 审批决策（用户对工具执行的决定）
#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalDecision {
    /// 用户批准执行
    Approved,
    /// 用户拒绝执行
    Rejected { reason: Option<String> },
}

// ── 响应器 ─────────────────────────────────────────────────────────────────

/// 审批响应器：用于向 Agent 返回用户的审批决策
///
/// 通过 [`ApprovalResponder::respond`] 方法返回用户决定。
/// 如果响应器被丢弃而未调用 respond，视为拒绝。
pub struct ApprovalResponder {
    sender: Option<oneshot::Sender<ApprovalDecision>>,
}

impl ApprovalResponder {
    fn new(sender: oneshot::Sender<ApprovalDecision>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    /// 返回用户的审批决策
    pub fn respond(mut self, decision: ApprovalDecision) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(decision);
        }
    }

    /// 快捷方法：批准
    pub fn approve(self) {
        self.respond(ApprovalDecision::Approved);
    }

    /// 快捷方法：拒绝
    pub fn reject(self, reason: Option<String>) {
        self.respond(ApprovalDecision::Rejected { reason });
    }
}

impl Drop for ApprovalResponder {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            // 未响应时默认拒绝
            let _ = sender.send(ApprovalDecision::Rejected {
                reason: Some("No response provided".to_string()),
            });
        }
    }
}

impl std::fmt::Debug for ApprovalResponder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalResponder")
            .field("has_sender", &self.sender.is_some())
            .finish()
    }
}

/// 输入响应器：用于向 Agent 返回用户的文本输入
pub struct InputResponder {
    sender: Option<oneshot::Sender<String>>,
}

impl InputResponder {
    fn new(sender: oneshot::Sender<String>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    /// 返回用户的输入
    pub fn respond(mut self, text: String) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(text);
        }
    }
}

impl Drop for InputResponder {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(String::new());
        }
    }
}

impl std::fmt::Debug for InputResponder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputResponder")
            .field("has_sender", &self.sender.is_some())
            .finish()
    }
}

// ── 事件 ───────────────────────────────────────────────────────────────────

/// 人工介入事件（通知上层应用需要用户介入）
///
/// 上层应用通过 [`HumanLoopManager::recv_event`] 接收事件，
/// 并通过事件中的 `responder` 返回用户决定。
#[derive(Debug)]
pub enum HumanLoopEvent {
    /// Agent 请求审批工具执行
    ApprovalRequest {
        /// 工具名称
        tool_name: String,
        /// 工具参数
        args: Value,
        /// 给用户的提示信息
        prompt: String,
        /// 响应器：用于返回用户决定
        responder: ApprovalResponder,
    },

    /// Agent 请求用户输入文本
    InputRequest {
        /// 给用户的提示信息
        prompt: String,
        /// 响应器：用于返回用户输入
        responder: InputResponder,
    },
}

// ── Manager (事件驱动模式) ────────────────────────────────────────────────

/// 人工介入管理器（事件驱动模式）
///
/// 这是推荐的 Human-in-the-Loop 实现方式：
/// - Agent 通过 `request` 方法发起请求并等待响应
/// - 上层应用通过 `recv_event` 接收事件并返回用户决定
/// - 两者解耦，适合聊天应用、Web 应用等场景
pub struct HumanLoopManager {
    /// 事件发送端（Agent -> 上层应用）
    event_tx: mpsc::Sender<HumanLoopEvent>,
    /// 事件接收端（上层应用接收）
    event_rx: tokio::sync::Mutex<Option<mpsc::Receiver<HumanLoopEvent>>>,
}

impl HumanLoopManager {
    /// 创建新的管理器
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel(16);
        Self {
            event_tx,
            event_rx: tokio::sync::Mutex::new(Some(event_rx)),
        }
    }

    /// 创建带缓冲区大小的管理器
    pub fn with_buffer(buffer_size: usize) -> Self {
        let (event_tx, event_rx) = mpsc::channel(buffer_size);
        Self {
            event_tx,
            event_rx: tokio::sync::Mutex::new(Some(event_rx)),
        }
    }

    /// 接收人工介入事件（上层应用调用）
    ///
    /// 返回 `None` 表示 Manager 已关闭。
    pub async fn recv_event(&self) -> Option<HumanLoopEvent> {
        // 从 Mutex 中取出 receiver，处理后放回
        let mut guard = self.event_rx.lock().await;
        let receiver = guard.as_mut()?;
        receiver.recv().await
    }

    /// 尝试非阻塞接收事件
    pub fn try_recv_event(&self) -> Option<HumanLoopEvent> {
        let mut guard = self.event_rx.blocking_lock();
        let receiver = guard.as_mut()?;
        receiver.try_recv().ok()
    }

    /// 运行事件处理循环，直到 channel 关闭（适用于同步阻塞等待的场景）
    ///
    /// 在 `tokio::spawn` 中使用时，请在 async block 内构造 handler，
    /// 避免生命周期泄漏：
    ///
    /// ```rust,no_run
    /// # use echo_agent::human_loop::{HumanLoopManager, HumanLoopHandler, ApprovalDecision};
    /// # use std::sync::Arc;
    /// # struct MyHandler;
    /// # #[async_trait::async_trait] impl HumanLoopHandler for MyHandler {
    /// #     async fn on_approval(&self, _: &str, _: &serde_json::Value, _: &str) -> ApprovalDecision { todo!() }
    /// #     async fn on_input(&self, _: &str) -> String { todo!() }
    /// # }
    /// # async fn example(manager: Arc<HumanLoopManager>) {
    /// let mgr = manager.clone();
    /// tokio::spawn(async move {
    ///     let handler = MyHandler;          // handler 在 async block 内构造
    ///     mgr.serve(&handler).await;
    /// });
    /// # }
    /// ```
    pub async fn serve(&self, handler: &dyn HumanLoopHandler) {
        while let Some(event) = self.recv_event().await {
            dispatch_event(event, handler).await;
        }
    }
}

impl Default for HumanLoopManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HumanLoopProvider for HumanLoopManager {
    async fn request(&self, req: HumanLoopRequest) -> Result<HumanLoopResponse> {
        match req.kind {
            HumanLoopKind::Approval => {
                let (tx, rx) = oneshot::channel();
                let responder = ApprovalResponder::new(tx);

                let event = HumanLoopEvent::ApprovalRequest {
                    tool_name: req.tool_name.clone().unwrap_or_default(),
                    args: req.args.clone().unwrap_or(Value::Null),
                    prompt: req.prompt.clone(),
                    responder,
                };

                // 发送事件给上层应用
                self.event_tx
                    .send(event)
                    .await
                    .map_err(|_| ReactError::Other("HumanLoop channel closed".to_string()))?;

                // 等待用户响应
                let decision = rx
                    .await
                    .map_err(|_| ReactError::Other("Approval responder dropped".to_string()))?;

                match decision {
                    ApprovalDecision::Approved => Ok(HumanLoopResponse::Approved),
                    ApprovalDecision::Rejected { reason } => {
                        Ok(HumanLoopResponse::Rejected { reason })
                    }
                }
            }
            HumanLoopKind::Input => {
                let (tx, rx) = oneshot::channel();
                let responder = InputResponder::new(tx);

                let event = HumanLoopEvent::InputRequest {
                    prompt: req.prompt.clone(),
                    responder,
                };

                self.event_tx
                    .send(event)
                    .await
                    .map_err(|_| ReactError::Other("HumanLoop channel closed".to_string()))?;

                let text = rx
                    .await
                    .map_err(|_| ReactError::Other("Input responder dropped".to_string()))?;

                Ok(HumanLoopResponse::Text(text))
            }
        }
    }
}

// ── HumanLoopHandler trait ────────────────────────────────────────────────────

/// 将 [`HumanLoopEvent`] 转化为具体 UI 交互的桥接接口
///
/// 实现此 trait 即可将 agent 的人工介入请求接入任意输入渠道，
/// 所有实现共用同一套事件驱动基础设施（[`HumanLoopManager`] + [`dispatch_event`]），
/// 无需改动 agent 内部逻辑。
///
/// # 内置实现（开箱即用）
///
/// | 实现 | 使用场景 |
/// |------|---------|
/// | 在 `main.rs` 中实现 `CliHumanLoopHandler` | 交互式命令行（统一 `you > ` 入口） |
///
/// # 自定义实现示例
///
/// ```rust,no_run
/// use echo_agent::human_loop::{HumanLoopHandler, ApprovalDecision};
/// use async_trait::async_trait;
/// use serde_json::Value;
///
/// /// WebSocket 实现（伪代码）
/// struct WsHumanLoopHandler { /* ws sender */ }
///
/// #[async_trait]
/// impl HumanLoopHandler for WsHumanLoopHandler {
///     async fn on_approval(&self, tool_name: &str, _args: &Value, prompt: &str) -> ApprovalDecision {
///         // 向 WebSocket 发送审批请求，等待客户端响应
///         // let reply = self.ws.send_and_wait(prompt).await;
///         ApprovalDecision::Approved
///     }
///     async fn on_input(&self, prompt: &str) -> String {
///         // 向 WebSocket 发送输入请求，等待客户端响应
///         String::new()
///     }
/// }
/// ```
#[async_trait]
pub trait HumanLoopHandler: Send + Sync {
    /// 工具审批请求：展示工具信息，收集用户的批准 / 拒绝决策
    async fn on_approval(&self, tool_name: &str, args: &Value, prompt: &str) -> ApprovalDecision;

    /// 文本输入请求：展示提示信息，收集用户的自由文本输入
    async fn on_input(&self, prompt: &str) -> String;
}

/// 将一个 [`HumanLoopEvent`] 分发给 `handler` 处理，并通过 responder 回传结果
///
/// 适合在 `tokio::select!` 循环中直接调用，或配合 [`HumanLoopManager::serve`] 使用。
///
/// ```rust,no_run
/// # use echo_agent::human_loop::{HumanLoopManager, dispatch_event};
/// # use echo_agent::prelude::*;
/// # use std::sync::Arc;
/// # struct MyHandler;
/// # #[async_trait::async_trait] impl echo_agent::human_loop::HumanLoopHandler for MyHandler {
/// #     async fn on_approval(&self, _: &str, _: &serde_json::Value, _: &str) -> echo_agent::human_loop::ApprovalDecision { todo!() }
/// #     async fn on_input(&self, _: &str) -> String { todo!() }
/// # }
/// # async fn example(manager: Arc<HumanLoopManager>) {
/// let handler = MyHandler;
/// while let Some(event) = manager.recv_event().await {
///     dispatch_event(event, &handler).await;
/// }
/// # }
/// ```
pub async fn dispatch_event(event: HumanLoopEvent, handler: &dyn HumanLoopHandler) {
    match event {
        HumanLoopEvent::ApprovalRequest {
            tool_name,
            args,
            prompt,
            responder,
        } => {
            let decision = handler.on_approval(&tool_name, &args, &prompt).await;
            responder.respond(decision);
        }
        HumanLoopEvent::InputRequest { prompt, responder } => {
            let text = handler.on_input(&prompt).await;
            responder.respond(text);
        }
    }
}

// ── 请求类型（保留兼容性）────────────────────────────────────────────────────

/// 人工介入的场景类型
#[derive(Debug, Clone, PartialEq)]
pub enum HumanLoopKind {
    /// 工具守卫：需要用户对工具执行做批准 / 拒绝决策
    Approval,
    /// 交互澄清：需要用户回复自由文本
    Input,
}

/// 向人工发起的介入请求
#[derive(Debug, Clone)]
pub struct HumanLoopRequest {
    /// 请求类型
    pub kind: HumanLoopKind,
    /// 给用户的提示信息
    pub prompt: String,
    /// 工具名称（仅 Approval 场景）
    pub tool_name: Option<String>,
    /// 工具参数（仅 Approval 场景）
    pub args: Option<Value>,
}

impl HumanLoopRequest {
    /// 构造审批请求
    pub fn approval(tool_name: impl Into<String>, args: Value) -> Self {
        let tool_name = tool_name.into();
        Self {
            kind: HumanLoopKind::Approval,
            prompt: format!("工具 [{}] 需要人工审批", tool_name),
            tool_name: Some(tool_name),
            args: Some(args),
        }
    }

    /// 构造文本输入请求
    pub fn input(prompt: impl Into<String>) -> Self {
        Self {
            kind: HumanLoopKind::Input,
            prompt: prompt.into(),
            tool_name: None,
            args: None,
        }
    }
}

// ── 响应类型 ───────────────────────────────────────────────────────────────

/// 人工介入的响应结果
#[derive(Debug, Clone)]
pub enum HumanLoopResponse {
    /// 用户批准
    Approved,
    /// 用户拒绝
    Rejected { reason: Option<String> },
    /// 用户输入的文本
    Text(String),
    /// 等待超时
    Timeout,
}

// ── Provider trait ────────────────────────────────────────────────────────────

/// 人工介入 Provider trait
///
/// 内置实现：
/// - [`HumanLoopManager`]：事件驱动模式（推荐）
/// - [`ConsoleHumanLoopProvider`]：命令行阻塞模式
/// - [`WebhookHumanLoopProvider`]：HTTP 回调模式
/// - [`WebSocketHumanLoopProvider`]：WebSocket 模式
#[async_trait]
pub trait HumanLoopProvider: Send + Sync {
    /// 发起人工介入请求
    async fn request(&self, req: HumanLoopRequest) -> Result<HumanLoopResponse>;
}

/// 默认 Provider：命令行阻塞模式（适用于非交互式程序）
///
/// 在交互式 CLI 场景中，应使用 [`HumanLoopManager`] 并自行处理事件，
/// 以便与主循环共享同一个用户输入入口。
pub fn default_provider() -> Arc<dyn HumanLoopProvider> {
    Arc::new(ConsoleHumanLoopProvider)
}

// ── Guard 管理器 ──────────────────────────────────────────────────────────────

/// 工具执行前的人工审批管理器（guard 模式）
///
/// 通过 [`ReactAgent::add_need_approval_tool`] 标记工具为"需要审批"，
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

// ── 单元测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_approval_decision_variants() {
        let approved = ApprovalDecision::Approved;
        let rejected = ApprovalDecision::Rejected {
            reason: Some("test".to_string()),
        };

        match approved {
            ApprovalDecision::Approved => assert!(true),
            _ => panic!("Should be Approved"),
        }

        match rejected {
            ApprovalDecision::Rejected { reason } => assert_eq!(reason, Some("test".to_string())),
            _ => panic!("Should be Rejected"),
        }
    }

    #[test]
    fn test_approval_responder_respond() {
        let (tx, mut rx) = oneshot::channel();
        let responder = ApprovalResponder::new(tx);

        responder.respond(ApprovalDecision::Approved);

        let result = rx.try_recv();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ApprovalDecision::Approved);
    }

    #[test]
    fn test_approval_responder_approve() {
        let (tx, mut rx) = oneshot::channel();
        let responder = ApprovalResponder::new(tx);

        responder.approve();

        let result = rx.try_recv();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ApprovalDecision::Approved);
    }

    #[test]
    fn test_approval_responder_reject() {
        let (tx, mut rx) = oneshot::channel();
        let responder = ApprovalResponder::new(tx);

        responder.reject(Some("test reason".to_string()));

        let result = rx.try_recv();
        assert!(result.is_ok());
        match result.unwrap() {
            ApprovalDecision::Rejected { reason } => {
                assert_eq!(reason, Some("test reason".to_string()))
            }
            _ => panic!("Should be Rejected"),
        }
    }

    #[test]
    fn test_approval_responder_drop_without_response() {
        let (tx, mut rx) = oneshot::channel();
        {
            let _responder = ApprovalResponder::new(tx);
            // responder 被丢弃但未调用 respond
        }

        let result = rx.try_recv();
        assert!(result.is_ok());
        // 未响应时默认拒绝
        match result.unwrap() {
            ApprovalDecision::Rejected { reason } => assert!(reason.is_some()),
            _ => panic!("Should be Rejected"),
        }
    }

    #[test]
    fn test_input_responder_respond() {
        let (tx, mut rx) = oneshot::channel();
        let responder = InputResponder::new(tx);

        responder.respond("user input".to_string());

        let result = rx.try_recv();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "user input");
    }

    #[test]
    fn test_human_loop_request_approval() {
        let request = HumanLoopRequest::approval("test_tool", serde_json::json!({"arg": "value"}));

        assert_eq!(request.kind, HumanLoopKind::Approval);
        assert_eq!(request.tool_name, Some("test_tool".to_string()));
        assert!(request.args.is_some());
    }

    #[test]
    fn test_human_loop_request_input() {
        let request = HumanLoopRequest::input("Please enter your name");

        assert_eq!(request.kind, HumanLoopKind::Input);
        assert_eq!(request.prompt, "Please enter your name");
        assert!(request.tool_name.is_none());
        assert!(request.args.is_none());
    }

    #[test]
    fn test_human_loop_response_variants() {
        let approved = HumanLoopResponse::Approved;
        let rejected = HumanLoopResponse::Rejected {
            reason: Some("test".to_string()),
        };
        let text = HumanLoopResponse::Text("hello".to_string());
        let timeout = HumanLoopResponse::Timeout;

        match approved {
            HumanLoopResponse::Approved => assert!(true),
            _ => panic!("Should be Approved"),
        }

        match rejected {
            HumanLoopResponse::Rejected { reason } => assert_eq!(reason, Some("test".to_string())),
            _ => panic!("Should be Rejected"),
        }

        match text {
            HumanLoopResponse::Text(s) => assert_eq!(s, "hello"),
            _ => panic!("Should be Text"),
        }

        match timeout {
            HumanLoopResponse::Timeout => assert!(true),
            _ => panic!("Should be Timeout"),
        }
    }

    #[test]
    fn test_human_approval_manager_new() {
        let manager = HumanApprovalManager::new();
        assert!(!manager.needs_approval("any_tool"));
    }

    #[test]
    fn test_human_approval_manager_mark_need_approval() {
        let mut manager = HumanApprovalManager::new();

        manager.mark_need_approval("dangerous_tool".to_string());

        assert!(manager.needs_approval("dangerous_tool"));
        assert!(!manager.needs_approval("safe_tool"));
    }

    #[test]
    fn test_human_approval_manager_multiple_tools() {
        let mut manager = HumanApprovalManager::new();

        manager.mark_need_approval("tool1".to_string());
        manager.mark_need_approval("tool2".to_string());
        manager.mark_need_approval("tool3".to_string());

        assert!(manager.needs_approval("tool1"));
        assert!(manager.needs_approval("tool2"));
        assert!(manager.needs_approval("tool3"));
        assert!(!manager.needs_approval("tool4"));
    }

    #[tokio::test]
    async fn test_human_loop_manager_new() {
        let manager = HumanLoopManager::new();
        // 验证可以成功创建
        let _ = manager;
    }

    #[test]
    fn test_human_loop_kind_variants() {
        let approval = HumanLoopKind::Approval;
        let input = HumanLoopKind::Input;

        match approval {
            HumanLoopKind::Approval => assert!(true),
            _ => panic!("Should be Approval"),
        }

        match input {
            HumanLoopKind::Input => assert!(true),
            _ => panic!("Should be Input"),
        }
    }
}
