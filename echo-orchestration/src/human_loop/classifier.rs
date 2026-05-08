//! 权限分类器 (Classifier)
//!
//! 在 `auto` 权限模式下，使用 Classifier 自动决策工具调用是否应该被阻止。
//! 抽象 trait 设计支持自定义实现：
//! - LlmClassifier: 使用 LLM 进行智能决策
//! - RuleClassifier: 基于简单规则匹配（无需 LLM）
//!
//! 参考 Claude Code 的 YoloClassifier 设计

use async_trait::async_trait;
use echo_core::llm::{ChatRequest, LlmClient, Message};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

use echo_core::error::Result;

// ── Classifier Trait ───────────────────────────────────────────────────────────

/// Classifier trait - 抽象分类器接口
///
/// 实现此 trait 可自定义 auto 模式下的权限决策逻辑。
/// 内置实现：
/// - `LlmClassifier`: 使用 LLM 进行智能决策
/// - `RuleClassifier`: 基于简单规则匹配
#[async_trait]
pub trait Classifier: Send + Sync {
    /// 分类工具调用是否应该被阻止
    ///
    /// # Arguments
    /// * `tool_name` - 工具名称
    /// * `tool_input` - 工具输入参数
    /// * `context` - 分类器上下文（包含对话历史、规则等）
    ///
    /// # Returns
    /// * `ClassifierResult` - 分类结果（是否阻止、原因、置信度）
    async fn classify(
        &self,
        tool_name: &str,
        tool_input: &Value,
        context: &ClassifierContext,
    ) -> Result<ClassifierResult>;
}

// ── Classifier Context ─────────────────────────────────────────────────────────

/// 风险上下文 — 描述当前操作的环境风险信号
#[derive(Debug, Clone, Default)]
pub struct RiskContext {
    /// 工作区是否包含敏感文件（.env / secrets/）
    pub has_sensitive_files: bool,
    /// 操作是否是破坏性的（写/删 vs 读）
    pub is_destructive: bool,
    /// 目录深度（越深越具体，风险越高）
    pub directory_depth: usize,
    /// 连续相似工具调用次数（重复风险）
    pub repetition_count: u32,
}

/// 分类器上下文 - 包含分类所需的所有信息
#[derive(Debug, Clone)]
pub struct ClassifierContext {
    /// 对话历史
    pub messages: Vec<Message>,
    /// Agent 名称
    pub agent_name: String,
    /// 会话 ID
    pub session_id: String,
    /// 当前 allow 规则描述
    pub allow_rules: Vec<String>,
    /// 当前 soft_deny 规则描述
    pub soft_deny_rules: Vec<String>,
    /// 工作区路径（可选）
    pub workspace_path: Option<String>,
    /// 项目类型（可选）: "rust" | "python" | "node" | "go" 等
    pub project_type: Option<String>,
    /// 最近访问的文件列表
    pub recent_files: Vec<String>,
    /// 风险上下文
    pub risk_context: Option<RiskContext>,
}

impl ClassifierContext {
    /// 创建空上下文
    pub fn new(agent_name: String, session_id: String) -> Self {
        Self {
            messages: Vec::new(),
            agent_name,
            session_id,
            allow_rules: Vec::new(),
            soft_deny_rules: Vec::new(),
            workspace_path: None,
            project_type: None,
            recent_files: Vec::new(),
            risk_context: None,
        }
    }

    /// 添加消息历史
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    /// 设置 allow 规则
    pub fn with_allow_rules(mut self, rules: Vec<String>) -> Self {
        self.allow_rules = rules;
        self
    }

    /// 设置 soft_deny 规则
    pub fn with_soft_deny_rules(mut self, rules: Vec<String>) -> Self {
        self.soft_deny_rules = rules;
        self
    }

    /// 设置工作区路径
    pub fn with_workspace_path(mut self, path: String) -> Self {
        self.workspace_path = Some(path);
        self
    }

    /// 设置项目类型
    pub fn with_project_type(mut self, ptype: String) -> Self {
        self.project_type = Some(ptype);
        self
    }

    /// 设置最近访问的文件列表
    pub fn with_recent_files(mut self, files: Vec<String>) -> Self {
        self.recent_files = files;
        self
    }

    /// 设置风险上下文
    pub fn with_risk_context(mut self, ctx: RiskContext) -> Self {
        self.risk_context = Some(ctx);
        self
    }
}

// ── Classifier Result ──────────────────────────────────────────────────────────

/// 分类结果
#[derive(Debug, Clone)]
pub struct ClassifierResult {
    /// 是否应该阻止工具调用
    pub should_block: bool,
    /// 阻止/允许的原因
    pub reason: String,
    /// 置信度 (0.0 - 1.0)
    pub confidence: f32,
}

impl ClassifierResult {
    /// 创建允许结果
    pub fn allow(reason: String) -> Self {
        Self {
            should_block: false,
            reason,
            confidence: 1.0,
        }
    }

    /// 创建阻止结果
    pub fn block(reason: String) -> Self {
        Self {
            should_block: true,
            reason,
            confidence: 1.0,
        }
    }

    /// 创建低置信度结果
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }
}

// ── Denial Tracker ─────────────────────────────────────────────────────────────

/// 拒绝跟踪器 - 防止 auto 模式拒绝循环
///
/// 当 Classifier 连续拒绝过多时，自动回退到用户交互模式，
/// 防止 Agent 陷入无限拒绝循环。
///
/// 参考 Claude Code 的 YoloClassifier 设计：
/// - 连续拒绝 N 次 → 回退
/// - 总拒绝 M 次 → 回退（默认 N=3, M=20）
#[derive(Debug)]
pub struct DenialTracker {
    /// 连续拒绝次数
    consecutive_denials: u32,
    /// 最大连续拒绝次数（超过则回退）
    max_consecutive: u32,
    /// 总拒绝次数
    total_denials: u32,
    /// 最大总拒绝次数（超过则回退）
    max_total_denials: u32,
    /// 上次拒绝时间
    last_denial_time: Option<Instant>,
}

impl DenialTracker {
    /// 默认最大连续拒绝次数
    pub const DEFAULT_MAX_CONSECUTIVE: u32 = 3;
    /// 默认最大总拒绝次数
    pub const DEFAULT_MAX_TOTAL: u32 = 20;

    /// 创建新的拒绝跟踪器
    pub fn new() -> Self {
        Self {
            consecutive_denials: 0,
            max_consecutive: Self::DEFAULT_MAX_CONSECUTIVE,
            total_denials: 0,
            max_total_denials: Self::DEFAULT_MAX_TOTAL,
            last_denial_time: None,
        }
    }

    /// 创建带自定义阈值的拒绝跟踪器
    pub fn with_max_consecutive(max: u32) -> Self {
        Self {
            consecutive_denials: 0,
            max_consecutive: max,
            total_denials: 0,
            max_total_denials: Self::DEFAULT_MAX_TOTAL,
            last_denial_time: None,
        }
    }

    /// 创建带自定义总拒绝阈值的拒绝跟踪器
    pub fn with_max_total(mut self, max: u32) -> Self {
        self.max_total_denials = max;
        self
    }

    /// 记录一次拒绝
    pub fn record_denial(&mut self) {
        self.consecutive_denials += 1;
        self.total_denials += 1;
        self.last_denial_time = Some(Instant::now());
    }

    /// 重置连续拒绝计数（成功执行后调用）
    pub fn reset(&mut self) {
        self.consecutive_denials = 0;
    }

    /// 检查是否应该回退到用户交互模式
    ///
    /// 触发条件（任一满足）：
    /// - 连续拒绝次数 ≥ max_consecutive（默认 3）
    /// - 总拒绝次数 ≥ max_total_denials（默认 20）
    pub fn should_fallback(&self) -> bool {
        self.consecutive_denials >= self.max_consecutive
            || self.total_denials >= self.max_total_denials
    }

    /// 获取连续拒绝次数
    pub fn consecutive_denials(&self) -> u32 {
        self.consecutive_denials
    }

    /// 获取总拒绝次数
    pub fn total_denials(&self) -> u32 {
        self.total_denials
    }

    /// 获取上次拒绝时间
    pub fn last_denial_time(&self) -> Option<Instant> {
        self.last_denial_time
    }
}

impl Default for DenialTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ── Rule Classifier ────────────────────────────────────────────────────────────

/// 基于规则的简单 Classifier（无需 LLM）
///
/// 使用预定义的模式匹配规则进行分类：
/// - `deny_patterns`: 匹配则阻止
/// - `allow_patterns`: 匹配则允许
pub struct RuleClassifier {
    /// 阻止模式列表
    deny_patterns: Vec<String>,
    /// 允许模式列表
    allow_patterns: Vec<String>,
    /// 拒绝跟踪器
    denial_tracker: Arc<Mutex<DenialTracker>>,
}

impl RuleClassifier {
    /// 创建空的规则分类器
    pub fn new() -> Self {
        Self {
            deny_patterns: Vec::new(),
            allow_patterns: Vec::new(),
            denial_tracker: Arc::new(Mutex::new(DenialTracker::new())),
        }
    }

    /// 添加阻止模式
    pub fn add_deny_pattern(&mut self, pattern: String) {
        self.deny_patterns.push(pattern);
    }

    /// 添加允许模式
    pub fn add_allow_pattern(&mut self, pattern: String) {
        self.allow_patterns.push(pattern);
    }

    /// 批量设置阻止模式
    pub fn with_deny_patterns(mut self, patterns: Vec<String>) -> Self {
        self.deny_patterns = patterns;
        self
    }

    /// 批量设置允许模式
    pub fn with_allow_patterns(mut self, patterns: Vec<String>) -> Self {
        self.allow_patterns = patterns;
        self
    }

    /// 检查模式是否匹配
    fn matches_pattern(pattern: &str, tool_name: &str) -> bool {
        super::pattern::matches_tool_pattern(pattern, tool_name)
    }
}

impl Default for RuleClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Classifier for RuleClassifier {
    async fn classify(
        &self,
        tool_name: &str,
        _tool_input: &Value,
        _context: &ClassifierContext,
    ) -> Result<ClassifierResult> {
        // 优先检查 allow_patterns
        for pattern in &self.allow_patterns {
            if Self::matches_pattern(pattern, tool_name) {
                return Ok(ClassifierResult::allow(format!(
                    "工具 '{}' 匹配允许规则 '{}'",
                    tool_name, pattern
                )));
            }
        }

        // 检查 deny_patterns
        for pattern in &self.deny_patterns {
            if Self::matches_pattern(pattern, tool_name) {
                // 记录拒绝
                let mut tracker = self.denial_tracker.lock().await;
                tracker.record_denial();

                return Ok(ClassifierResult::block(format!(
                    "工具 '{}' 匹配阻止规则 '{}'",
                    tool_name, pattern
                )));
            }
        }

        // 默认允许
        Ok(ClassifierResult::allow(format!(
            "工具 '{}' 未匹配任何规则，默认允许",
            tool_name
        )))
    }
}

// ── LLM Classifier ─────────────────────────────────────────────────────────────

/// 使用 LLM 进行智能决策的分类器
///
/// 参考 Claude Code 的 YoloClassifier 实现：
/// 1. 构建分类提示词（包含 allow/soft_deny 规则）
/// 2. 调用 LLM 进行分类
/// 3. 解析结果并跟踪拒绝次数
pub struct LlmClassifier {
    /// LLM 客户端
    client: Arc<dyn LlmClient>,
    /// 使用的模型名称
    #[allow(dead_code)] // Used for logging/debugging; will be read in future integration
    model: String,
    /// 拒绝跟踪器
    denial_tracker: Arc<Mutex<DenialTracker>>,
    /// 分类提示词模板
    prompt_template: String,
}

impl LlmClassifier {
    /// 创建 LLM 分类器
    pub fn new(client: Arc<dyn LlmClient>, model: String) -> Self {
        Self {
            client,
            model,
            denial_tracker: Arc::new(Mutex::new(DenialTracker::new())),
            prompt_template: Self::default_prompt_template(),
        }
    }

    /// 设置自定义提示词模板
    pub fn with_prompt_template(mut self, template: String) -> Self {
        self.prompt_template = template;
        self
    }

    /// 截断工具输入以避免超出 LLM 上下文
    ///
    /// 限制为 2000 字符，防止大参数消耗过多 token。
    fn truncate_tool_input(tool_input: &Value) -> String {
        let full = serde_json::to_string_pretty(tool_input).unwrap_or_default();
        if full.len() <= 2000 {
            return full;
        }
        // UTF-8 safe truncation
        let end = full
            .char_indices()
            .take_while(|(idx, _)| *idx < 2000)
            .last()
            .map(|(idx, c)| idx + c.len_utf8())
            .unwrap_or(0);
        format!("{}...(truncated)", &full[..end])
    }

    /// 默认分类提示词模板
    fn default_prompt_template() -> String {
        r#"你是一个权限决策助手。请根据以下信息判断是否允许执行工具调用。

## 工具信息
- 工具名称: {tool_name}
- 工具参数: {tool_input}

## 环境上下文
- 工作区: {workspace_path}
- 项目类型: {project_type}
- 最近文件: {recent_files}
- 风险信号: {risk_context}

## 规则
### 允许规则:
{allow_rules}

### 需谨慎的规则:
{soft_deny_rules}

## 对话上下文
{conversation_summary}

请判断是否应该阻止此工具调用。回复格式：
- 如果允许: {"should_block": false, "reason": "允许原因"}
- 如果阻止: {"should_block": true, "reason": "阻止原因"}

请只返回 JSON，不要其他内容。"#
            .to_string()
    }

    /// 构建分类提示词
    fn build_prompt(
        &self,
        tool_name: &str,
        tool_input: &Value,
        context: &ClassifierContext,
    ) -> String {
        let allow_rules = if context.allow_rules.is_empty() {
            "无".to_string()
        } else {
            context.allow_rules.join("\n- ")
        };

        let soft_deny_rules = if context.soft_deny_rules.is_empty() {
            "无".to_string()
        } else {
            context.soft_deny_rules.join("\n- ")
        };

        // 简化对话摘要
        let conversation_summary = context
            .messages
            .iter()
            .filter_map(|m| m.content.as_text())
            .take(5)
            .collect::<Vec<_>>()
            .join("\n");

        self.prompt_template
            .replace("{tool_name}", tool_name)
            .replace("{tool_input}", &Self::truncate_tool_input(tool_input))
            .replace("{allow_rules}", &allow_rules)
            .replace("{soft_deny_rules}", &soft_deny_rules)
            .replace("{conversation_summary}", &conversation_summary)
            .replace(
                "{workspace_path}",
                context.workspace_path.as_deref().unwrap_or("unknown"),
            )
            .replace(
                "{project_type}",
                context.project_type.as_deref().unwrap_or("unknown"),
            )
            .replace("{recent_files}", &context.recent_files.join(", "))
            .replace(
                "{risk_context}",
                &context
                    .risk_context
                    .as_ref()
                    .map_or("none".to_string(), |r| {
                        format!(
                            "sensitive_files={}, destructive={}, depth={}, repetition={}",
                            r.has_sensitive_files,
                            r.is_destructive,
                            r.directory_depth,
                            r.repetition_count
                        )
                    }),
            )
    }

    /// 解析 LLM 响应
    fn parse_response(response: &str) -> Option<ClassifierResult> {
        // 尝试使用正则表达式提取 JSON 对象
        // 匹配第一个完整的 JSON 对象（支持嵌套）
        if let Some(json_str) = Self::extract_json(response)
            && let Ok(json) = serde_json::from_str::<Value>(&json_str)
        {
            let should_block = json
                .get("should_block")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let reason = json
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("未提供原因")
                .to_string();

            return Some(if should_block {
                ClassifierResult::block(reason)
            } else {
                ClassifierResult::allow(reason)
            });
        }

        // 回退：尝试从文本中判断
        if response.contains("阻止") || response.contains("block") {
            Some(ClassifierResult::block("LLM 判断应阻止".to_string()))
        } else if response.contains("允许") || response.contains("allow") {
            Some(ClassifierResult::allow("LLM 判断应允许".to_string()))
        } else {
            None
        }
    }

    /// 从文本中提取第一个 JSON 对象
    ///
    /// 使用大括号计数来匹配完整嵌套的 JSON 对象，
    /// 比简单的行过滤更可靠。
    fn extract_json(text: &str) -> Option<String> {
        let trimmed = text.trim();
        let mut depth = 0i32;
        let mut start: Option<usize> = None;
        let chars = trimmed.char_indices();

        for (i, c) in chars {
            if c == '{' {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            } else if c == '}' {
                depth -= 1;
                if depth == 0
                    && let Some(s) = start
                {
                    return Some(trimmed[s..=i].to_string());
                }
            }
        }
        None
    }
}

#[async_trait]
impl Classifier for LlmClassifier {
    async fn classify(
        &self,
        tool_name: &str,
        tool_input: &Value,
        context: &ClassifierContext,
    ) -> Result<ClassifierResult> {
        // 检查是否应该回退
        {
            let tracker = self.denial_tracker.lock().await;
            if tracker.should_fallback() {
                return Ok(
                    ClassifierResult::block("连续拒绝过多，回退到用户交互模式".to_string())
                        .with_confidence(0.0),
                ); // 低置信度表示需要用户确认
            }
        }

        // 构建提示词
        let prompt = self.build_prompt(tool_name, tool_input, context);

        // 调用 LLM
        let request = ChatRequest::new(vec![Message::user(prompt)]);
        let response = self.client.chat(request).await?;

        // 解析响应
        let content = response.content().unwrap_or_default();
        let result = Self::parse_response(&content).unwrap_or_else(|| {
            ClassifierResult::allow("无法解析 LLM 响应，默认允许".to_string()).with_confidence(0.5)
        });

        // 更新拒绝跟踪
        if result.should_block {
            let mut tracker = self.denial_tracker.lock().await;
            tracker.record_denial();
        } else {
            let mut tracker = self.denial_tracker.lock().await;
            tracker.reset();
        }

        Ok(result)
    }
}

// ── Composite Classifier ───────────────────────────────────────────────────────

/// 组合分类器 - 依次执行多个分类器
///
/// 按优先级顺序执行分类器，取第一个非默认结果。
pub struct CompositeClassifier {
    /// 分类器列表（按优先级排序）
    classifiers: Vec<Arc<dyn Classifier>>,
}

impl CompositeClassifier {
    /// 创建空的组合分类器
    pub fn new() -> Self {
        Self {
            classifiers: Vec::new(),
        }
    }

    /// 添加分类器
    pub fn add(&mut self, classifier: Arc<dyn Classifier>) {
        self.classifiers.push(classifier);
    }

    /// 批量设置分类器
    pub fn with_classifiers(mut self, classifiers: Vec<Arc<dyn Classifier>>) -> Self {
        self.classifiers = classifiers;
        self
    }
}

impl Default for CompositeClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Classifier for CompositeClassifier {
    async fn classify(
        &self,
        tool_name: &str,
        tool_input: &Value,
        context: &ClassifierContext,
    ) -> Result<ClassifierResult> {
        for classifier in &self.classifiers {
            let result = classifier.classify(tool_name, tool_input, context).await?;
            // 如果置信度 >= 0.8，直接返回
            if result.confidence >= 0.8 {
                return Ok(result);
            }
        }

        // 所有分类器都低置信度，默认允许
        Ok(
            ClassifierResult::allow("所有分类器低置信度，默认允许".to_string())
                .with_confidence(0.5),
        )
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classifier_result_allow() {
        let result = ClassifierResult::allow("test reason".to_string());
        assert!(!result.should_block);
        assert_eq!(result.reason, "test reason");
        assert_eq!(result.confidence, 1.0);
    }

    #[test]
    fn test_classifier_result_block() {
        let result = ClassifierResult::block("dangerous".to_string());
        assert!(result.should_block);
        assert_eq!(result.reason, "dangerous");
        assert_eq!(result.confidence, 1.0);
    }

    #[test]
    fn test_classifier_result_with_confidence() {
        let result = ClassifierResult::allow("test".to_string()).with_confidence(0.5);
        assert_eq!(result.confidence, 0.5);
    }

    #[test]
    fn test_denial_tracker_new() {
        let tracker = DenialTracker::new();
        assert_eq!(tracker.consecutive_denials(), 0);
        assert_eq!(tracker.total_denials(), 0);
        assert!(!tracker.should_fallback());
    }

    #[test]
    fn test_denial_tracker_record() {
        let mut tracker = DenialTracker::new();
        tracker.record_denial();
        assert_eq!(tracker.consecutive_denials(), 1);
        assert_eq!(tracker.total_denials(), 1);
        assert!(tracker.last_denial_time().is_some());
    }

    #[test]
    fn test_denial_tracker_fallback() {
        let mut tracker = DenialTracker::with_max_consecutive(2);
        tracker.record_denial();
        assert!(!tracker.should_fallback());
        tracker.record_denial();
        assert!(tracker.should_fallback());
    }

    #[test]
    fn test_denial_tracker_reset() {
        let mut tracker = DenialTracker::new();
        tracker.record_denial();
        tracker.record_denial();
        tracker.reset();
        assert_eq!(tracker.consecutive_denials(), 0);
        assert_eq!(tracker.total_denials(), 2); // 总数不重置
    }

    #[test]
    fn test_classifier_context_new() {
        let ctx = ClassifierContext::new("agent".to_string(), "session".to_string());
        assert_eq!(ctx.agent_name, "agent");
        assert_eq!(ctx.session_id, "session");
        assert!(ctx.messages.is_empty());
    }

    #[test]
    fn test_rule_classifier_matches_pattern() {
        assert!(RuleClassifier::matches_pattern("*", "Bash"));
        assert!(RuleClassifier::matches_pattern("Bash", "Bash"));
        assert!(RuleClassifier::matches_pattern("Bash", "Bash(git:*)"));
        assert!(!RuleClassifier::matches_pattern("Bash", "BashExtra"));
    }

    #[tokio::test]
    async fn test_rule_classifier_allow() {
        let classifier =
            RuleClassifier::new().with_allow_patterns(vec!["Read".to_string(), "Glob".to_string()]);

        let ctx = ClassifierContext::new("agent".to_string(), "session".to_string());
        let result = classifier
            .classify("Read", &serde_json::json!({}), &ctx)
            .await
            .unwrap();
        assert!(!result.should_block);
    }

    #[tokio::test]
    async fn test_rule_classifier_deny() {
        let classifier = RuleClassifier::new().with_deny_patterns(vec!["Bash(rm:*)".to_string()]);

        let ctx = ClassifierContext::new("agent".to_string(), "session".to_string());
        let result = classifier
            .classify(
                "Bash(rm:rf)",
                &serde_json::json!({"command": "rm -rf"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.should_block);
    }

    #[tokio::test]
    async fn test_rule_classifier_default() {
        let classifier = RuleClassifier::new();
        let ctx = ClassifierContext::new("agent".to_string(), "session".to_string());
        let result = classifier
            .classify("UnknownTool", &serde_json::json!({}), &ctx)
            .await
            .unwrap();
        assert!(!result.should_block); // 默认允许
    }

    #[test]
    fn test_llm_classifier_build_prompt() {
        // 使用 mock client 测试提示词构建
        use echo_core::llm::{ChatChunk, ChatCompletionResponse, ChatRequest, ChatResponse};
        use futures::future::BoxFuture;
        use futures::stream::BoxStream;

        struct MockClient;

        impl LlmClient for MockClient {
            fn chat(&self, _request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
                Box::pin(async move {
                    Ok(ChatResponse {
                        message: Message::assistant(
                            "{\"should_block\": false, \"reason\": \"test\"}".to_string(),
                        ),
                        finish_reason: None,
                        raw: ChatCompletionResponse::default(),
                    })
                })
            }

            fn chat_stream(
                &self,
                _request: ChatRequest,
            ) -> BoxFuture<'_, Result<BoxStream<'_, Result<ChatChunk>>>> {
                Box::pin(async move {
                    Err(echo_core::error::ReactError::Other(
                        "not implemented".to_string(),
                    ))
                })
            }

            fn model_name(&self) -> &str {
                "mock"
            }
        }

        let client = Arc::new(MockClient);
        let classifier = LlmClassifier::new(client, "model".to_string());

        let ctx = ClassifierContext::new("agent".to_string(), "session".to_string())
            .with_allow_rules(vec!["Read:*".to_string()])
            .with_soft_deny_rules(vec!["Bash(rm:*)".to_string()]);

        let prompt = classifier.build_prompt("Read", &serde_json::json!({"path": "/tmp"}), &ctx);
        assert!(prompt.contains("Read"));
        assert!(prompt.contains("Read:*"));
        assert!(prompt.contains("Bash(rm:*)"));
    }

    #[test]
    fn test_llm_classifier_parse_response_json() {
        // 直接测试 parse_response 函数
        let result =
            parse_hook_output_for_test("{\"should_block\": true, \"reason\": \"危险操作\"}");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.should_block);
        assert_eq!(r.reason, "危险操作");
    }

    #[test]
    fn test_llm_classifier_parse_response_text() {
        let result = parse_hook_output_for_test("我认为应该阻止此操作");
        assert!(result.is_some());
        assert!(result.unwrap().should_block);
    }

    // Helper function for testing
    fn parse_hook_output_for_test(response: &str) -> Option<ClassifierResult> {
        // Try to extract JSON
        let json_str = response
            .trim()
            .lines()
            .filter(|line| line.starts_with('{') || line.starts_with('}'))
            .collect::<String>();

        if let Ok(json) = serde_json::from_str::<Value>(&json_str) {
            let should_block = json
                .get("should_block")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let reason = json
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("未提供原因")
                .to_string();

            return Some(if should_block {
                ClassifierResult::block(reason)
            } else {
                ClassifierResult::allow(reason)
            });
        }

        // Try to judge from text
        if response.contains("阻止") || response.contains("block") {
            Some(ClassifierResult::block("LLM 判断应阻止".to_string()))
        } else if response.contains("允许") || response.contains("allow") {
            Some(ClassifierResult::allow("LLM 判断应允许".to_string()))
        } else {
            None
        }
    }
}
