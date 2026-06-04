//! 人工介入（Human-in-the-Loop）
//!
//! 在工具执行前拦截，向外部请求审批或文本输入。
//!
//! ## 设计原则
//!
//! - **事件驱动**: 审批请求通过事件通知上层应用，而非直接阻塞
//! - **统一入口**: 用户输入、审批响应和任务选择共用同一个输入通道
//! - **异步解耦**: Agent 执行与用户交互分离，支持复杂 UI 场景
//! - **策略驱动**: 通过 `PermissionService` 配置权限策略，支持风险等级和会话缓存
//!
//! ## 使用示例
//!
//! ```rust,no_run
//! use echo_core::error::Result;
//! use echo_orchestration::human_loop::{ApprovalDecision, HumanLoopEvent, HumanLoopManager};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<()> {
//! // 创建 manager
//! let manager = Arc::new(HumanLoopManager::new());
//!
//! // 在后台任务中监听事件
//! let mgr = manager.clone();
//! tokio::spawn(async move {
//!     while let Some(event) = mgr.recv_event().await {
//!         match event {
//!             HumanLoopEvent::ApprovalRequest { tool_name, responder, .. } => {
//!                 println!("工具 '{}' 需要审批", tool_name);
//!                 responder.respond(ApprovalDecision::Approved);
//!             }
//!             HumanLoopEvent::InputRequest { prompt, responder } => {
//!                 println!("Agent 询问: {}", prompt);
//!                 responder.respond("用户输入的内容".to_string());
//!             }
//!             HumanLoopEvent::SelectionRequest { task_id, options, responder, .. } => {
//!                 println!("任务 '{}' 需要选择", task_id);
//!                 let selection = options.first().cloned().unwrap_or_default();
//!                 responder.respond(selection, None);
//!             }
//!         }
//!     }
//! });
//!
//! // 将 manager 接入你自己的 runtime 层，或通过 `echo_agent` façade 使用。
//! # Ok(())
//! # }
//! ```

mod approval_cache;
mod audit;
mod batch;
mod classifier;
mod console;
mod pattern;
pub mod permission;
pub mod policy;
mod protected;
pub mod service;
mod webhook;
#[cfg(feature = "websocket")]
mod websocket;

pub use approval_cache::SessionApprovalCache;
pub use audit::{
    CompositePermissionAuditSink, InMemoryPermissionAuditSink, LoggingPermissionAuditSink,
    PermissionAuditEntry, PermissionAuditSink,
};
pub use batch::BatchApprovalProvider;
pub use classifier::{
    Classifier, ClassifierContext, ClassifierResult, CompositeClassifier, DenialTracker,
    LlmClassifier, RiskContext, RuleClassifier,
};
pub use console::ConsoleHumanLoopProvider;
pub use permission::{
    DefaultPermissionRequestHandler, PermissionContext, PermissionRequest,
    PermissionRequestHandler, PermissionResponse, PermissionResponseDecision, PermissionUpdate,
    RiskLevel, SuggestedAction, Suggestion,
};
pub use policy::{ApprovalRule, ApprovalScope, PolicyDecision};
pub use protected::{ProtectedPathChecker, ProtectedPathResult};
pub use service::PermissionService;
pub use webhook::WebhookHumanLoopProvider;
#[cfg(feature = "websocket")]
pub use websocket::WebSocketHumanLoopProvider;

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use echo_core::error::{ReactError, Result};

// ── 审批决策 ───────────────────────────────────────────────────────────────

/// 审批决策（用户对工具执行的决定）
#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalDecision {
    /// 用户批准执行
    Approved,
    /// 用户批准执行，并指定审批范围（用于会话级缓存）
    ApprovedWithScope { scope: ApprovalScope },
    /// 用户修改了参数后批准执行
    Modified {
        /// 修改后的参数
        args: Value,
        /// 审批范围
        scope: ApprovalScope,
    },
    /// 用户拒绝执行
    Rejected { reason: Option<String> },
    /// 用户推迟决策（暂不处理）
    Deferred,
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

    /// 快捷方法：带范围批准
    pub fn approve_with_scope(self, scope: ApprovalScope) {
        self.respond(ApprovalDecision::ApprovedWithScope { scope });
    }

    /// 快捷方法：修改参数后批准
    pub fn approve_modified(self, args: Value, scope: ApprovalScope) {
        self.respond(ApprovalDecision::Modified { args, scope });
    }

    /// 快捷方法：拒绝
    pub fn reject(self, reason: Option<String>) {
        self.respond(ApprovalDecision::Rejected { reason });
    }

    /// 快捷方法：推迟决策
    pub fn defer(self) {
        self.respond(ApprovalDecision::Deferred);
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

/// 选择响应器：用于任务 checkpoint 的选项选择
pub struct SelectionResponder {
    sender: Option<oneshot::Sender<(String, Option<String>)>>,
}

impl SelectionResponder {
    fn new(sender: oneshot::Sender<(String, Option<String>)>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    /// 返回用户的选择和可选指令
    pub fn respond(mut self, selection: String, instructions: Option<String>) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send((selection, instructions));
        }
    }
}

impl Drop for SelectionResponder {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            // 未响应时默认取消
            let _ = sender.send(("cancel".to_string(), None));
        }
    }
}

impl std::fmt::Debug for SelectionResponder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectionResponder")
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
        /// 风险等级
        risk_level: RiskLevel,
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

    /// 任务 checkpoint 选择请求
    SelectionRequest {
        /// 任务 ID
        task_id: String,
        /// 给用户的提示信息
        prompt: String,
        /// 可选项列表
        options: Vec<String>,
        /// 上下文数据
        context: Value,
        /// 阶段名称
        phase: String,
        /// 响应器：用于返回用户选择
        responder: SelectionResponder,
    },
}

// ── Manager (事件驱动模式) ────────────────────────────────────────────────

/// 人工介入管理器（事件驱动模式）
///
/// 推荐的 Human-in-the-Loop 实现方式：
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
        let mut guard = self.event_rx.lock().await;
        let receiver = guard.as_mut()?;
        receiver.recv().await
    }

    /// 尝试非阻塞接收事件
    ///
    /// Uses `try_lock()` to avoid blocking the caller. Returns `None` if the
    /// lock is currently held or if no event is available.
    pub fn try_recv_event(&self) -> Option<HumanLoopEvent> {
        let mut guard = self.event_rx.try_lock().ok()?;
        let receiver = guard.as_mut()?;
        receiver.try_recv().ok()
    }

    /// 运行事件处理循环，直到 channel 关闭
    ///
    /// **注意**：此方法会消费 `event_rx`（通过 `take`），
    /// 因此只能调用一次。第二次调用 `serve` 将立即返回。
    /// 如需多次服务，请使用 `serve_with_receiver` 并传入独立的 receiver。
    pub async fn serve(&self, handler: &dyn HumanLoopHandler) {
        // 一次性取出 event_rx，避免每次 recv_event 都锁住并尝试 take
        let receiver = {
            let mut guard = self.event_rx.lock().await;
            guard.take()
        };

        let Some(mut receiver) = receiver else {
            return;
        };

        while let Some(event) = receiver.recv().await {
            dispatch_event(event, handler).await;
        }
    }

    /// 使用指定的 receiver 运行事件处理循环
    ///
    /// 适用于需要将接收端分离到独立任务的场景。
    pub async fn serve_with_receiver(
        mut receiver: mpsc::Receiver<HumanLoopEvent>,
        handler: &dyn HumanLoopHandler,
    ) {
        while let Some(event) = receiver.recv().await {
            dispatch_event(event, handler).await;
        }
    }
}

impl Default for HumanLoopManager {
    fn default() -> Self {
        Self::new()
    }
}

impl HumanLoopProvider for HumanLoopManager {
    fn request(&self, req: HumanLoopRequest) -> BoxFuture<'_, Result<HumanLoopResponse>> {
        Box::pin(async move {
            match req.kind {
                HumanLoopKind::Approval => {
                    let (tx, rx) = oneshot::channel();
                    let responder = ApprovalResponder::new(tx);

                    let risk_level = req.risk_level.unwrap_or(RiskLevel::Medium);

                    let event = HumanLoopEvent::ApprovalRequest {
                        tool_name: req.tool_name.clone().unwrap_or_default(),
                        args: req.args.clone().unwrap_or(Value::Null),
                        prompt: req.prompt.clone(),
                        risk_level,
                        responder,
                    };

                    self.event_tx
                        .send(event)
                        .await
                        .map_err(|_| ReactError::Other("HumanLoop channel closed".to_string()))?;

                    // 等待用户响应（带可选超时）
                    let decision = if let Some(timeout) = req.timeout {
                        match tokio::time::timeout(timeout, rx).await {
                            Ok(result) => result.map_err(|_| {
                                ReactError::Other("Approval responder dropped".to_string())
                            })?,
                            Err(_) => {
                                return Ok(HumanLoopResponse::Timeout);
                            }
                        }
                    } else {
                        rx.await.map_err(|_| {
                            ReactError::Other("Approval responder dropped".to_string())
                        })?
                    };

                    match decision {
                        ApprovalDecision::Approved => Ok(HumanLoopResponse::Approved),
                        ApprovalDecision::ApprovedWithScope { scope } => {
                            Ok(HumanLoopResponse::ApprovedWithScope { scope })
                        }
                        ApprovalDecision::Modified { args, scope } => {
                            Ok(HumanLoopResponse::ModifiedArgs { args, scope })
                        }
                        ApprovalDecision::Rejected { reason } => {
                            Ok(HumanLoopResponse::Rejected { reason })
                        }
                        ApprovalDecision::Deferred => Ok(HumanLoopResponse::Deferred),
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
                HumanLoopKind::Selection => {
                    let (tx, rx) = oneshot::channel();
                    let responder = SelectionResponder::new(tx);

                    let event = HumanLoopEvent::SelectionRequest {
                        task_id: req.task_id.clone().unwrap_or_default(),
                        prompt: req.prompt.clone(),
                        options: req.options.clone().unwrap_or_default(),
                        context: req.context.clone().unwrap_or(Value::Null),
                        phase: req.phase.clone().unwrap_or_default(),
                        responder,
                    };

                    self.event_tx
                        .send(event)
                        .await
                        .map_err(|_| ReactError::Other("HumanLoop channel closed".to_string()))?;

                    let result = if let Some(timeout) = req.timeout {
                        match tokio::time::timeout(timeout, rx).await {
                            Ok(r) => r.map_err(|_| {
                                ReactError::Other("Selection responder dropped".to_string())
                            })?,
                            Err(_) => {
                                return Ok(HumanLoopResponse::Timeout);
                            }
                        }
                    } else {
                        rx.await.map_err(|_| {
                            ReactError::Other("Selection responder dropped".to_string())
                        })?
                    };

                    Ok(HumanLoopResponse::Selection {
                        selection: result.0,
                        instructions: result.1,
                    })
                }
            }
        })
    }
}

// ── HumanLoopHandler trait ────────────────────────────────────────────────────

/// 将 [`HumanLoopEvent`] 转化为具体 UI 交互的桥接接口
///
/// 实现此 trait 即可将 agent 的人工介入请求接入任意输入渠道，
/// 所有实现共用同一套事件驱动基础设施。
pub trait HumanLoopHandler: Send + Sync {
    /// 工具审批请求：展示工具信息，收集用户的批准 / 拒绝决策
    fn on_approval<'a>(
        &'a self,
        tool_name: &'a str,
        args: &'a Value,
        prompt: &'a str,
    ) -> BoxFuture<'a, ApprovalDecision>;

    /// 文本输入请求：展示提示信息，收集用户的自由文本输入
    fn on_input<'a>(&'a self, prompt: &'a str) -> BoxFuture<'a, String>;

    /// 任务 checkpoint 选择：展示选项列表，收集用户的选择
    ///
    /// 默认实现选择第一个选项（向后兼容）。
    fn on_selection<'a>(
        &'a self,
        _task_id: &'a str,
        _prompt: &'a str,
        options: &'a [String],
        _context: &'a Value,
        _phase: &'a str,
    ) -> BoxFuture<'a, (String, Option<String>)> {
        Box::pin(async move {
            let selection = options
                .first()
                .cloned()
                .unwrap_or_else(|| "approve".to_string());
            (selection, None)
        })
    }
}

/// 将一个 [`HumanLoopEvent`] 分发给 `handler` 处理
pub async fn dispatch_event(event: HumanLoopEvent, handler: &dyn HumanLoopHandler) {
    match event {
        HumanLoopEvent::ApprovalRequest {
            tool_name,
            args,
            prompt,
            risk_level: _,
            responder,
        } => {
            let decision = handler.on_approval(&tool_name, &args, &prompt).await;
            responder.respond(decision);
        }
        HumanLoopEvent::InputRequest { prompt, responder } => {
            let text = handler.on_input(&prompt).await;
            responder.respond(text);
        }
        HumanLoopEvent::SelectionRequest {
            task_id,
            prompt,
            options,
            context,
            phase,
            responder,
        } => {
            let (selection, instructions) = handler
                .on_selection(&task_id, &prompt, &options, &context, &phase)
                .await;
            responder.respond(selection, instructions);
        }
    }
}

// ── 请求类型 ────────────────────────────────────────────────────────────────

/// 人工介入的场景类型
#[derive(Debug, Clone, PartialEq)]
pub enum HumanLoopKind {
    /// 工具守卫：需要用户对工具执行做批准 / 拒绝决策
    Approval,
    /// 交互澄清：需要用户回复自由文本
    Input,
    /// 任务 checkpoint：用户从预定义选项中选择
    Selection,
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
    /// 风险等级（仅 Approval 场景）
    pub risk_level: Option<RiskLevel>,
    /// 超时时长（None 表示无限等待）
    pub timeout: Option<Duration>,
    /// 任务 ID（Selection 场景：标识等待中的任务）
    pub task_id: Option<String>,
    /// 可选项列表（Selection 场景：如 ["Approve", "Revise", "Cancel"]）
    pub options: Option<Vec<String>>,
    /// 任意上下文数据（Selection 场景：如草稿内容）
    pub context: Option<Value>,
    /// 阶段名称（Selection 场景：等待输入的管线阶段）
    pub phase: Option<String>,
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
            risk_level: None,
            timeout: None,
            task_id: None,
            options: None,
            context: None,
            phase: None,
        }
    }

    /// 构造带风险等级的审批请求
    pub fn approval_with_risk(
        tool_name: impl Into<String>,
        args: Value,
        risk_level: RiskLevel,
    ) -> Self {
        let tool_name = tool_name.into();
        Self {
            kind: HumanLoopKind::Approval,
            prompt: format!("工具 [{}] 需要人工审批（{}风险）", tool_name, risk_level),
            tool_name: Some(tool_name),
            args: Some(args),
            risk_level: Some(risk_level),
            timeout: None,
            task_id: None,
            options: None,
            context: None,
            phase: None,
        }
    }

    /// 构造带超时的审批请求
    pub fn approval_with_timeout(
        tool_name: impl Into<String>,
        args: Value,
        timeout: Duration,
    ) -> Self {
        let tool_name = tool_name.into();
        Self {
            kind: HumanLoopKind::Approval,
            prompt: format!("工具 [{}] 需要人工审批", tool_name),
            tool_name: Some(tool_name),
            args: Some(args),
            risk_level: None,
            timeout: Some(timeout),
            task_id: None,
            options: None,
            context: None,
            phase: None,
        }
    }

    /// 构造文本输入请求
    pub fn input(prompt: impl Into<String>) -> Self {
        Self {
            kind: HumanLoopKind::Input,
            prompt: prompt.into(),
            tool_name: None,
            args: None,
            risk_level: None,
            timeout: None,
            task_id: None,
            options: None,
            context: None,
            phase: None,
        }
    }

    /// 构造选择请求（任务 checkpoint 场景）
    pub fn selection(
        task_id: impl Into<String>,
        prompt: impl Into<String>,
        options: Vec<String>,
    ) -> Self {
        Self {
            kind: HumanLoopKind::Selection,
            prompt: prompt.into(),
            task_id: Some(task_id.into()),
            options: Some(options),
            tool_name: None,
            args: None,
            risk_level: None,
            timeout: None,
            context: None,
            phase: None,
        }
    }

    /// 为选择请求添加上下文数据
    pub fn with_context(mut self, context: Value) -> Self {
        self.context = Some(context);
        self
    }

    /// 为选择请求添加阶段名称
    pub fn with_phase(mut self, phase: impl Into<String>) -> Self {
        self.phase = Some(phase.into());
        self
    }
}

// ── 响应类型 ───────────────────────────────────────────────────────────────

/// 人工介入的响应结果
#[derive(Debug, Clone)]
pub enum HumanLoopResponse {
    /// 用户批准
    Approved,
    /// 用户带范围批准
    ApprovedWithScope { scope: ApprovalScope },
    /// 用户修改了参数后批准
    ModifiedArgs { args: Value, scope: ApprovalScope },
    /// 用户拒绝
    Rejected { reason: Option<String> },
    /// 用户输入的文本
    Text(String),
    /// 等待超时
    Timeout,
    /// 用户推迟决策
    Deferred,
    /// 用户选择了选项（任务 checkpoint 响应）
    Selection {
        selection: String,
        instructions: Option<String>,
    },
}

// ── Provider trait ──────────────────────────────────────────────────────────

/// 人工介入 Provider trait
///
/// 内置实现：
/// - [`HumanLoopManager`]：事件驱动模式（推荐）
/// - [`ConsoleHumanLoopProvider`]：命令行阻塞模式
/// - [`WebhookHumanLoopProvider`]：HTTP 回调模式
/// - [`WebSocketHumanLoopProvider`]：WebSocket 模式
pub trait HumanLoopProvider: Send + Sync {
    /// 发起人工介入请求
    fn request(&self, req: HumanLoopRequest) -> BoxFuture<'_, Result<HumanLoopResponse>>;
}

/// 默认 Provider：命令行阻塞模式
pub fn default_provider() -> Arc<dyn HumanLoopProvider> {
    Arc::new(ConsoleHumanLoopProvider)
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────

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
            ApprovalDecision::Approved => {}
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
    fn test_approval_responder_approve_with_scope() {
        let (tx, mut rx) = oneshot::channel();
        let responder = ApprovalResponder::new(tx);

        responder.approve_with_scope(ApprovalScope::Session);

        let result = rx.try_recv();
        assert!(result.is_ok());
        match result.unwrap() {
            ApprovalDecision::ApprovedWithScope { scope } => {
                assert_eq!(scope, ApprovalScope::Session);
            }
            _ => panic!("Should be ApprovedWithScope"),
        }
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
        }

        let result = rx.try_recv();
        assert!(result.is_ok());
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
    fn test_human_loop_request_with_risk() {
        let request = HumanLoopRequest::approval_with_risk(
            "dangerous_tool",
            serde_json::json!({"cmd": "rm -rf"}),
            RiskLevel::Critical,
        );

        assert_eq!(request.kind, HumanLoopKind::Approval);
        assert_eq!(request.risk_level, Some(RiskLevel::Critical));
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
        let deferred = HumanLoopResponse::Deferred;

        assert!(matches!(approved, HumanLoopResponse::Approved));
        assert!(matches!(rejected, HumanLoopResponse::Rejected { .. }));
        assert!(matches!(text, HumanLoopResponse::Text(_)));
        assert!(matches!(timeout, HumanLoopResponse::Timeout));
        assert!(matches!(deferred, HumanLoopResponse::Deferred));
    }

    #[tokio::test]
    async fn test_human_loop_manager_new() {
        let _manager = HumanLoopManager::new();
    }

    #[test]
    fn test_human_loop_kind_variants() {
        assert!(matches!(HumanLoopKind::Approval, HumanLoopKind::Approval));
        assert!(matches!(HumanLoopKind::Input, HumanLoopKind::Input));
    }
}
