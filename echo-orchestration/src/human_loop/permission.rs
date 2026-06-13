//! 权限请求抽象 (PermissionRequest)
//!
//! 提供权限请求的统一抽象，支持不同 UI 实现（CLI、Web、桌面等）。
//!
//! ## 设计原则
//!
//! - **UI 无关**: 核心逻辑与 UI 分离，支持多端适配
//! - **结构化**: 提供丰富的上下文信息，便于 UI 渲染
//! - **可扩展**: 支持建议、风险等级、批量操作等高级特性
//!
//! ## 使用示例
//!
//! ```rust,no_run
//! use async_trait::async_trait;
//! use echo_core::error::Result;
//! use echo_orchestration::human_loop::{
//!     PermissionRequest, PermissionRequestHandler, PermissionResponse, RiskLevel, Suggestion,
//! };
//!
//! struct AllowAllHandler;
//!
//! #[async_trait]
//! impl PermissionRequestHandler for AllowAllHandler {
//!     async fn handle(&self, _request: PermissionRequest) -> Result<PermissionResponse> {
//!         Ok(PermissionResponse::allowed())
//!     }
//! }
//!
//! # async fn example(handler: &dyn PermissionRequestHandler) -> Result<()> {
//! // 创建权限请求
//! let request = PermissionRequest::new("Bash", serde_json::json!({"command": "rm -rf"}))
//!     .with_risk_level(RiskLevel::High)
//!     .with_suggestion(Suggestion::allow_once())
//!     .with_suggestion(Suggestion::deny_always("危险操作"));
//!
//! // 处理请求
//! let response = handler.handle(request).await?;
//! # Ok(())
//! # }
//! # let handler = AllowAllHandler;
//! # let _ = example(&handler);
//! ```

use async_trait::async_trait;
use echo_core::tools::ToolRiskLevel;
use echo_core::tools::permission::{PermissionMode, ToolPermission};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use echo_core::error::Result;

// ── 风险等级 ────────────────────────────────────────────────────────────────────

/// 风险等级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum RiskLevel {
    /// 低风险：只读操作
    #[default]
    Low,
    /// 中等风险：写入操作
    Medium,
    /// 高风险：执行/敏感操作
    High,
    /// 危险：不可逆操作
    Critical,
}

impl RiskLevel {
    /// 根据权限类型推断风险等级
    pub fn from_permissions(permissions: &[ToolPermission]) -> Self {
        if permissions.contains(&ToolPermission::Sensitive) {
            return RiskLevel::Critical;
        }
        if permissions.contains(&ToolPermission::Execute) {
            return RiskLevel::High;
        }
        if permissions.contains(&ToolPermission::Write)
            || permissions.contains(&ToolPermission::Network)
        {
            return RiskLevel::Medium;
        }
        RiskLevel::Low
    }

    /// 是否需要用户确认
    pub fn requires_confirmation(&self) -> bool {
        matches!(self, RiskLevel::High | RiskLevel::Critical)
    }

    /// 获取颜色标识（用于 UI 渲染）
    pub fn color(&self) -> &'static str {
        match self {
            RiskLevel::Low => "green",
            RiskLevel::Medium => "yellow",
            RiskLevel::High => "orange",
            RiskLevel::Critical => "red",
        }
    }

    /// 获取图标（用于 UI 渲染）
    pub fn icon(&self) -> &'static str {
        match self {
            RiskLevel::Low => "✓",
            RiskLevel::Medium => "⚠",
            RiskLevel::High => "⚡",
            RiskLevel::Critical => "🔴",
        }
    }
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskLevel::Low => write!(f, "低风险"),
            RiskLevel::Medium => write!(f, "中等风险"),
            RiskLevel::High => write!(f, "高风险"),
            RiskLevel::Critical => write!(f, "危险"),
        }
    }
}

/// Bridge the compile-time [`ToolRiskLevel`] into the runtime [`RiskLevel`].
///
/// | `ToolRiskLevel`   | → `RiskLevel` |
/// |-------------------|---------------|
/// | `ReadOnly`        | `Low`         |
/// | `Standard` (default) | `Medium`   |
/// | `Dangerous`       | `High`        |
///
/// Note: `ToolRiskLevel` is a hint declared at tool definition time via
/// `#[tool]` or `Tool::risk_level()`. At runtime, the framework typically
/// recomputes risk from the tool's `ToolPermission` set via
/// [`RiskLevel::from_permissions`], which can yield `Critical` for tools
/// with the `Sensitive` permission — a level that `ToolRiskLevel` has no
/// direct equivalent for.
impl From<ToolRiskLevel> for RiskLevel {
    fn from(level: ToolRiskLevel) -> Self {
        match level {
            ToolRiskLevel::ReadOnly => RiskLevel::Low,
            ToolRiskLevel::Standard => RiskLevel::Medium,
            ToolRiskLevel::Dangerous => RiskLevel::High,
        }
    }
}

// ── 建议操作 ────────────────────────────────────────────────────────────────────

/// 建议操作类型
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SuggestedAction {
    /// 允许本次执行
    AllowOnce,
    /// 允许本次会话中所有相同操作
    AllowForSession,
    /// 始终允许（持久化规则）
    AllowAlways,
    /// 拒绝本次执行
    DenyOnce,
    /// 始终拒绝（持久化规则）
    DenyAlways,
    /// 修改输入后执行
    ModifyInput { updated_input: Value },
    /// 自定义响应
    Custom { response: String },
}

/// 建议选项（供用户选择的选项）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    /// 显示标签
    pub label: String,
    /// 描述说明
    pub description: String,
    /// 对应的动作
    pub action: SuggestedAction,
    /// 是否为推荐选项
    #[serde(default)]
    pub recommended: bool,
    /// 快捷键（可选）
    #[serde(default)]
    pub shortcut: Option<String>,
}

impl Suggestion {
    /// 创建允许本次的建议
    pub fn allow_once() -> Self {
        Self {
            label: "允许".to_string(),
            description: "允许本次执行".to_string(),
            action: SuggestedAction::AllowOnce,
            recommended: false,
            shortcut: Some("y".to_string()),
        }
    }

    /// 创建允许会话的建议
    pub fn allow_for_session() -> Self {
        Self {
            label: "始终允许（本次会话）".to_string(),
            description: "在本次会话中允许所有相同操作".to_string(),
            action: SuggestedAction::AllowForSession,
            recommended: false,
            shortcut: Some("s".to_string()),
        }
    }

    /// 创建始终允许的建议
    pub fn allow_always() -> Self {
        Self {
            label: "始终允许".to_string(),
            description: "添加持久化规则，始终允许此操作".to_string(),
            action: SuggestedAction::AllowAlways,
            recommended: false,
            shortcut: Some("a".to_string()),
        }
    }

    /// 创建拒绝的建议
    pub fn deny_once(reason: impl Into<String>) -> Self {
        Self {
            label: "拒绝".to_string(),
            description: reason.into(),
            action: SuggestedAction::DenyOnce,
            recommended: false,
            shortcut: Some("n".to_string()),
        }
    }

    /// 创建始终拒绝的建议
    pub fn deny_always(reason: impl Into<String>) -> Self {
        Self {
            label: "始终拒绝".to_string(),
            description: reason.into(),
            action: SuggestedAction::DenyAlways,
            recommended: false,
            shortcut: Some("d".to_string()),
        }
    }

    /// 创建修改输入的建议
    pub fn modify_input(label: impl Into<String>, updated_input: Value) -> Self {
        Self {
            label: label.into(),
            description: "修改参数后执行".to_string(),
            action: SuggestedAction::ModifyInput { updated_input },
            recommended: false,
            shortcut: None,
        }
    }

    /// 设置为推荐选项
    pub fn recommended(mut self) -> Self {
        self.recommended = true;
        self
    }

    /// 设置快捷键
    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }
}

// ── 权限请求 ────────────────────────────────────────────────────────────────────

/// 权限请求（向用户请求权限决策的完整上下文）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    /// 工具名称
    pub tool_name: String,
    /// 工具输入参数
    pub tool_input: Value,
    /// 所需权限类型
    pub required_permissions: Vec<ToolPermission>,
    /// 风险等级
    pub risk_level: RiskLevel,
    /// 给用户的提示信息
    pub prompt: String,
    /// 建议选项列表
    pub suggestions: Vec<Suggestion>,
    /// 上下文信息（用于 UI 渲染）
    #[serde(default)]
    pub context: PermissionContext,
    /// 请求 ID（用于追踪）
    #[serde(default)]
    pub request_id: Option<String>,
    /// 会话 ID
    #[serde(default)]
    pub session_id: Option<String>,
    /// Agent 名称
    #[serde(default)]
    pub agent_name: Option<String>,
}

/// 权限上下文（额外的上下文信息）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionContext {
    /// 工作目录
    #[serde(default)]
    pub working_directory: Option<String>,
    /// 相关文件列表
    #[serde(default)]
    pub affected_files: Vec<String>,
    /// 预估影响
    #[serde(default)]
    pub estimated_impact: Option<String>,
    /// 额外元数据
    #[serde(default)]
    pub metadata: serde_json::Map<String, Value>,
}

impl PermissionRequest {
    /// 创建新的权限请求
    pub fn new(tool_name: impl Into<String>, tool_input: Value) -> Self {
        let tool_name = tool_name.into();
        let prompt = format!("工具 '{}' 需要执行，是否允许？", tool_name);

        Self {
            tool_name,
            tool_input,
            required_permissions: Vec::new(),
            risk_level: RiskLevel::Low,
            prompt,
            suggestions: Vec::new(),
            context: PermissionContext::default(),
            request_id: None,
            session_id: None,
            agent_name: None,
        }
    }

    /// 设置所需权限
    pub fn with_permissions(mut self, permissions: Vec<ToolPermission>) -> Self {
        self.risk_level = RiskLevel::from_permissions(&permissions);
        self.required_permissions = permissions;
        self
    }

    /// 设置风险等级
    pub fn with_risk_level(mut self, level: RiskLevel) -> Self {
        self.risk_level = level;
        self
    }

    /// 设置提示信息
    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = prompt.into();
        self
    }

    /// 添加建议选项
    pub fn with_suggestion(mut self, suggestion: Suggestion) -> Self {
        self.suggestions.push(suggestion);
        self
    }

    /// 设置建议选项列表
    pub fn with_suggestions(mut self, suggestions: Vec<Suggestion>) -> Self {
        self.suggestions = suggestions;
        self
    }

    /// 设置请求 ID
    pub fn with_request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }

    /// 设置会话 ID
    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// 设置 Agent 名称
    pub fn with_agent_name(mut self, name: impl Into<String>) -> Self {
        self.agent_name = Some(name.into());
        self
    }

    /// 设置上下文
    pub fn with_context(mut self, context: PermissionContext) -> Self {
        self.context = context;
        self
    }

    /// 生成默认建议选项
    pub fn with_default_suggestions(mut self) -> Self {
        self.suggestions = vec![Suggestion::allow_once(), Suggestion::deny_once("取消执行")];
        self
    }

    /// 根据风险等级生成建议选项
    pub fn with_risk_based_suggestions(mut self) -> Self {
        self.suggestions = match self.risk_level {
            RiskLevel::Low => {
                vec![
                    Suggestion::allow_once().recommended(),
                    Suggestion::allow_for_session(),
                ]
            }
            RiskLevel::Medium => {
                vec![
                    Suggestion::allow_once(),
                    Suggestion::allow_for_session(),
                    Suggestion::deny_once("取消执行"),
                ]
            }
            RiskLevel::High => {
                vec![
                    Suggestion::allow_once(),
                    Suggestion::deny_once("拒绝执行").recommended(),
                    Suggestion::deny_always("始终拒绝此操作"),
                ]
            }
            RiskLevel::Critical => {
                vec![
                    Suggestion::deny_once("拒绝执行").recommended(),
                    Suggestion::allow_once(),
                    Suggestion::deny_always("始终拒绝此危险操作"),
                ]
            }
        };
        self
    }

    /// 是否需要用户确认
    pub fn requires_confirmation(&self) -> bool {
        self.risk_level.requires_confirmation() || !self.suggestions.is_empty()
    }
}

// ── 权限响应 ────────────────────────────────────────────────────────────────────

/// 权限响应（用户的决策结果）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionResponse {
    /// 用户的决策
    pub decision: PermissionResponseDecision,
    /// 规则更新（可选）
    #[serde(default)]
    pub rule_updates: Vec<PermissionUpdate>,
    /// 用户反馈（可选）
    #[serde(default)]
    pub feedback: Option<String>,
    /// 修改后的输入（如果选择了 ModifyInput）
    #[serde(default)]
    pub updated_input: Option<Value>,
}

/// 权限响应决策
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionResponseDecision {
    /// 允许执行
    Allowed,
    /// 拒绝执行
    Denied { reason: Option<String> },
    /// 需要更多信息
    NeedMoreInfo { question: String },
}

impl PermissionResponse {
    /// 创建允许响应
    pub fn allowed() -> Self {
        Self {
            decision: PermissionResponseDecision::Allowed,
            rule_updates: Vec::new(),
            feedback: None,
            updated_input: None,
        }
    }

    /// 创建拒绝响应
    pub fn denied(reason: Option<String>) -> Self {
        Self {
            decision: PermissionResponseDecision::Denied { reason },
            rule_updates: Vec::new(),
            feedback: None,
            updated_input: None,
        }
    }

    /// 从建议操作创建响应
    ///
    /// `tool_name` 和 `args` 用于构建正确的规则匹配器（matcher），
    /// 而不是错误地使用 suggestion label 作为匹配模式。
    pub fn from_suggestion(suggestion: &Suggestion, tool_name: &str, args: &Value) -> Self {
        // 构建基于 tool_name 和 args 的匹配模式
        let matcher = Self::build_matcher(tool_name, args);

        match &suggestion.action {
            SuggestedAction::AllowOnce => Self::allowed(),
            SuggestedAction::AllowForSession => Self {
                decision: PermissionResponseDecision::Allowed,
                rule_updates: vec![PermissionUpdate::add_session_rule(matcher)],
                feedback: None,
                updated_input: None,
            },
            SuggestedAction::AllowAlways => Self {
                decision: PermissionResponseDecision::Allowed,
                rule_updates: vec![PermissionUpdate::add_permanent_rule(matcher)],
                feedback: None,
                updated_input: None,
            },
            SuggestedAction::DenyOnce => Self::denied(Some(suggestion.description.clone())),
            SuggestedAction::DenyAlways => Self {
                decision: PermissionResponseDecision::Denied {
                    reason: Some(suggestion.description.clone()),
                },
                rule_updates: vec![PermissionUpdate::add_deny_rule(matcher)],
                feedback: None,
                updated_input: None,
            },
            SuggestedAction::ModifyInput { updated_input } => Self {
                decision: PermissionResponseDecision::Allowed,
                rule_updates: Vec::new(),
                feedback: None,
                updated_input: Some(updated_input.clone()),
            },
            SuggestedAction::Custom { response } => Self {
                decision: PermissionResponseDecision::Allowed,
                rule_updates: Vec::new(),
                feedback: Some(response.clone()),
                updated_input: None,
            },
        }
    }

    /// 为给定的 tool_name 和 args 构建规则匹配器字符串
    fn build_matcher(tool_name: &str, args: &Value) -> String {
        // 如果 args 是空对象或 null，只匹配工具名
        if matches!(args, Value::Null) || matches!(args, Value::Object(map) if map.is_empty()) {
            return tool_name.to_string();
        }
        // 否则构建 tool_name(key:*) 形式的匹配模式
        format!("{tool_name}(*)")
    }

    /// 添加反馈
    pub fn with_feedback(mut self, feedback: impl Into<String>) -> Self {
        self.feedback = Some(feedback.into());
        self
    }
}

// ── 权限更新 ────────────────────────────────────────────────────────────────────

/// 权限规则更新
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PermissionUpdate {
    /// 添加规则
    AddRule {
        matcher: String,
        behavior: String,
        source: String,
    },
    /// 移除规则
    RemoveRule { matcher: String },
    /// 设置权限模式
    SetMode { mode: PermissionMode },
}

impl PermissionUpdate {
    /// 创建添加会话规则的更新
    pub fn add_session_rule(matcher: String) -> Self {
        Self::AddRule {
            matcher,
            behavior: "allow".to_string(),
            source: "session".to_string(),
        }
    }

    /// 创建添加永久规则的更新
    pub fn add_permanent_rule(matcher: String) -> Self {
        Self::AddRule {
            matcher,
            behavior: "allow".to_string(),
            source: "userSettings".to_string(),
        }
    }

    /// 创建添加拒绝规则的更新
    pub fn add_deny_rule(matcher: String) -> Self {
        Self::AddRule {
            matcher,
            behavior: "deny".to_string(),
            source: "userSettings".to_string(),
        }
    }
}

// ── PermissionRequestHandler Trait ─────────────────────────────────────────────

/// 权限请求处理器 trait
///
/// 实现此 trait 可自定义权限请求的 UI 处理逻辑。
/// 内置实现：
/// - `DefaultPermissionRequestHandler`: 使用 HumanLoopProvider
/// - `ConsolePermissionRequestHandler`: 命令行交互
#[async_trait]
pub trait PermissionRequestHandler: Send + Sync {
    /// 处理权限请求
    ///
    /// # Arguments
    /// * `request` - 权限请求
    ///
    /// # Returns
    /// * `PermissionResponse` - 用户的决策
    async fn handle(&self, request: PermissionRequest) -> Result<PermissionResponse>;

    /// 批量处理权限请求（默认实现：逐个处理）
    async fn handle_batch(
        &self,
        requests: Vec<PermissionRequest>,
    ) -> Result<Vec<PermissionResponse>> {
        let mut responses = Vec::with_capacity(requests.len());
        for request in requests {
            responses.push(self.handle(request).await?);
        }
        Ok(responses)
    }

    /// 是否为空占位处理器（Null Handler）。
    ///
    /// 默认返回 `false`。`NullPermissionRequestHandler` 覆盖为 `true`。
    /// 用于 `PermissionService` 判断是否已配置真实处理器，
    /// 避免使用 `type_name_of_val` 等脆弱的字符串匹配。
    fn is_null_handler(&self) -> bool {
        false
    }
}

// ── 默认实现 ────────────────────────────────────────────────────────────────────

/// 默认权限请求处理器（使用 HumanLoopProvider）
pub struct DefaultPermissionRequestHandler<P: super::HumanLoopProvider> {
    provider: Arc<P>,
}

impl<P: super::HumanLoopProvider> DefaultPermissionRequestHandler<P> {
    pub fn new(provider: Arc<P>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl<P: super::HumanLoopProvider + 'static> PermissionRequestHandler
    for DefaultPermissionRequestHandler<P>
{
    async fn handle(&self, request: PermissionRequest) -> Result<PermissionResponse> {
        use super::{HumanLoopRequest, HumanLoopResponse};

        let req = HumanLoopRequest::approval(&request.tool_name, request.tool_input.clone());

        match self.provider.request(req).await? {
            HumanLoopResponse::Approved => Ok(PermissionResponse::allowed()),
            HumanLoopResponse::ApprovedWithScope { scope: _ } => Ok(PermissionResponse::allowed()),
            HumanLoopResponse::ModifiedArgs { args, scope: _ } => {
                // 保留用户修改的参数，传递给调用方
                Ok(PermissionResponse {
                    decision: PermissionResponseDecision::Allowed,
                    rule_updates: Vec::new(),
                    feedback: None,
                    updated_input: Some(args),
                })
            }
            HumanLoopResponse::Rejected { reason } => Ok(PermissionResponse::denied(reason)),
            HumanLoopResponse::Text(text) => Ok(PermissionResponse::allowed().with_feedback(text)),
            HumanLoopResponse::Timeout => {
                Ok(PermissionResponse::denied(Some("请求超时".to_string())))
            }
            HumanLoopResponse::Deferred => {
                Ok(PermissionResponse::denied(Some("审批被推迟".to_string())))
            }
            HumanLoopResponse::Selection { .. } => Ok(PermissionResponse::denied(Some(
                "收到意外的 Selection 响应".to_string(),
            ))),
        }
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_level_from_permissions() {
        assert_eq!(
            RiskLevel::from_permissions(&[ToolPermission::Read]),
            RiskLevel::Low
        );
        assert_eq!(
            RiskLevel::from_permissions(&[ToolPermission::Write]),
            RiskLevel::Medium
        );
        assert_eq!(
            RiskLevel::from_permissions(&[ToolPermission::Execute]),
            RiskLevel::High
        );
        assert_eq!(
            RiskLevel::from_permissions(&[ToolPermission::Sensitive]),
            RiskLevel::Critical
        );
    }

    #[test]
    fn test_risk_level_requires_confirmation() {
        assert!(!RiskLevel::Low.requires_confirmation());
        assert!(!RiskLevel::Medium.requires_confirmation());
        assert!(RiskLevel::High.requires_confirmation());
        assert!(RiskLevel::Critical.requires_confirmation());
    }

    #[test]
    fn test_suggestion_create() {
        let s = Suggestion::allow_once();
        assert_eq!(s.label, "允许");
        assert_eq!(s.action, SuggestedAction::AllowOnce);
        assert_eq!(s.shortcut, Some("y".to_string()));
    }

    #[test]
    fn test_permission_request_new() {
        let req = PermissionRequest::new("Bash", serde_json::json!({"command": "ls"}));
        assert_eq!(req.tool_name, "Bash");
        assert_eq!(req.risk_level, RiskLevel::Low);
    }

    #[test]
    fn test_permission_request_with_permissions() {
        let req = PermissionRequest::new("Bash", serde_json::json!({"command": "rm"}))
            .with_permissions(vec![ToolPermission::Execute]);

        assert_eq!(req.risk_level, RiskLevel::High);
        assert_eq!(req.required_permissions.len(), 1);
    }

    #[test]
    fn test_permission_request_risk_based_suggestions() {
        let req = PermissionRequest::new("Bash", serde_json::json!({}))
            .with_risk_level(RiskLevel::Critical)
            .with_risk_based_suggestions();

        assert!(!req.suggestions.is_empty());
        // 危险操作的第一个建议应该是拒绝
        assert!(matches!(
            &req.suggestions[0].action,
            SuggestedAction::DenyOnce
        ));
    }

    #[test]
    fn test_permission_response_allowed() {
        let resp = PermissionResponse::allowed();
        assert!(matches!(resp.decision, PermissionResponseDecision::Allowed));
    }

    #[test]
    fn test_permission_response_denied() {
        let resp = PermissionResponse::denied(Some("test reason".to_string()));
        assert!(matches!(
            resp.decision,
            PermissionResponseDecision::Denied { .. }
        ));
    }

    #[test]
    fn test_permission_response_from_suggestion() {
        let s = Suggestion::allow_once();
        let resp = PermissionResponse::from_suggestion(&s, "Bash", &serde_json::json!({}));
        assert!(matches!(resp.decision, PermissionResponseDecision::Allowed));

        let s = Suggestion::deny_once("test");
        let resp = PermissionResponse::from_suggestion(&s, "Bash", &serde_json::json!({}));
        assert!(matches!(
            resp.decision,
            PermissionResponseDecision::Denied { .. }
        ));
    }

    #[test]
    fn test_permission_response_from_suggestion_matcher() {
        // 验证 matcher 使用 tool_name 而非 suggestion label
        let s = Suggestion::allow_for_session();
        let resp = PermissionResponse::from_suggestion(
            &s,
            "DangerousTool",
            &serde_json::json!({"cmd": "x"}),
        );

        match &resp.rule_updates[0] {
            PermissionUpdate::AddRule { matcher, .. } => {
                assert_eq!(matcher, "DangerousTool(*)");
            }
            _ => panic!("Expected AddRule"),
        }
    }

    #[test]
    fn test_permission_update_create() {
        let update = PermissionUpdate::add_session_rule("Bash".to_string());
        assert!(matches!(update, PermissionUpdate::AddRule { .. }));

        let update = PermissionUpdate::add_deny_rule("Bash(rm:*)".to_string());
        assert!(matches!(update, PermissionUpdate::AddRule { behavior, .. } if behavior == "deny"));
    }

    #[test]
    fn test_permission_request_serialization() {
        let req = PermissionRequest::new("Bash", serde_json::json!({"cmd": "ls"}))
            .with_risk_level(RiskLevel::High)
            .with_default_suggestions();

        let json = serde_json::to_string(&req).unwrap();
        let parsed: PermissionRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.tool_name, "Bash");
        assert_eq!(parsed.risk_level, RiskLevel::High);
    }
}
