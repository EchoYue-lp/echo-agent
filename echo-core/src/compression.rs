//! Context compression — trait and input/output types
//!
//! Implementations live in `echo_state::compression::compressor`.

use crate::error::Result;
use crate::llm::types::{ContentPart, Message, MessageContent};
use crate::tokenizer::{HeuristicTokenizer, Tokenizer};
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
pub use tokio_util::sync::CancellationToken;

/// 投影在模型上下文中的物化位置。
///
/// 位置的区分服务于 prompt KV 缓存：provider 只能复用**逐字节相同的前缀**。
/// 因此只有"同一 generation 内不会变"的固定指令允许改写 system/history 边界，
/// 运行中变化的事实只能追加在已见历史之后。
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionPlacement {
    /// 按 marker 替换 system 块之后、真实历史之前的同一投影。
    ///
    /// 仅适用于固定指令（产品身份、生效规则、绑定方法等）。它的任何变化都会
    /// 使其后全部已见历史的前缀失效，等价于一次 prompt generation 切换。
    #[default]
    Boundary,
    /// 在当前模型安全点的历史末尾追加一个不可变版本。
    ///
    /// 新版本不改写任何已见消息，因此已有前缀保持逐字节相同；旧版本成为普通
    /// 历史，由原 compression 安全点归并，不会永久 protected。
    Tail,
}

/// A replaceable, marker-tagged message projected into model context.
#[derive(Debug, Clone, Default)]
pub struct ContextProjection {
    /// Stable content marker used to identify the projection across refreshes.
    pub marker: String,
    /// Current projected message, or `None` to remove a stale projection.
    pub message: Option<Message>,
    /// 追加式语义版本：内容/来源相同的版本只物化一次。
    ///
    /// `ProjectionPlacement::Tail` 必须提供版本；同一 `(marker, version)` 的
    /// prepare 重入、重试和 checkpoint 重投影都不会重复追加。版本变化表示语义
    /// 事实发生变化（而非"这一步又跑了一轮"），所以生产者必须保证版本只在语义
    /// 变化时改变。
    pub version: Option<String>,
    /// 物化位置。默认 `Boundary` 保持既有 replace 语义。
    pub placement: ProjectionPlacement,
}

impl ContextProjection {
    /// 构造边界替换投影（固定指令，既有语义）。
    pub fn replacing(marker: impl Into<String>, message: Option<Message>) -> Self {
        Self {
            marker: marker.into(),
            message,
            version: None,
            placement: ProjectionPlacement::Boundary,
        }
    }

    /// 构造追加式版本投影（运行事实，每个语义版本只追加一次）。
    pub fn versioned(
        marker: impl Into<String>,
        version: impl Into<String>,
        message: Option<Message>,
    ) -> Self {
        Self {
            marker: marker.into(),
            message,
            version: Some(version.into()),
            placement: ProjectionPlacement::Tail,
        }
    }

    /// 是否为追加式版本投影。
    pub fn is_append_version(&self) -> bool {
        matches!(self.placement, ProjectionPlacement::Tail) && self.version.is_some()
    }
}

/// 框架保留的上下文投影信封前缀。
///
/// 该信封是 framework-owned 的模型可见标记：它既标识"这是框架生成、不是用户原文"，
/// 也是缓存分段与追加生命周期的**唯一**元数据来源。core 与 provider 都不得再按
/// 产品 marker 或正文前缀猜测动态段。
pub const CONTEXT_PROJECTION_ENVELOPE_PREFIX: &str = "<echo-agent-context-projection-v2>";
const LEGACY_CONTEXT_PROJECTION_ENVELOPE_PREFIX: &str = "<echo-agent-context-projection-v1>";

/// 一条框架投影消息的信封头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextProjectionEnvelope<'a> {
    /// marker 的 JSON 编码（与 `ContextProjection::marker` 一致）。
    pub marker_json: &'a str,
    /// 追加式版本 JSON 编码；`None` 表示边界替换投影。
    pub version_json: Option<&'a str>,
}

impl ContextProjectionEnvelope<'_> {
    /// 解码后的 marker 文本。
    pub fn marker(&self) -> Option<String> {
        serde_json::from_str(self.marker_json).ok()
    }

    /// 解码后的版本文本。
    pub fn version(&self) -> Option<String> {
        serde_json::from_str(self.version_json?).ok()
    }

    /// 是否为追加式版本投影（带版本）。
    pub fn is_append_version(&self) -> bool {
        self.version_json.is_some()
    }
}

/// 读出消息正文开头的框架投影信封（不存在或格式不符时返回 `None`）。
pub fn context_projection_envelope(message: &Message) -> Option<ContextProjectionEnvelope<'_>> {
    let text = match &message.content {
        MessageContent::Text(content) => Some(content.as_str()),
        MessageContent::Parts(parts) => parts.first().and_then(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        }),
        MessageContent::Empty => None,
    }?;
    parse_context_projection_envelope(text)
}

/// v1 仅有 marker 头，后续全是 Boundary 正文；v2 显式表达 placement/version，
/// 以空行终止头部，绝不从正文推断元数据。
pub fn parse_context_projection_envelope(text: &str) -> Option<ContextProjectionEnvelope<'_>> {
    let (marker_json, version_json) = if let Some(rest) = text
        .strip_prefix(CONTEXT_PROJECTION_ENVELOPE_PREFIX)
        .and_then(|rest| rest.strip_prefix('\n'))
    {
        let (header, _) = rest.split_once("\n\n")?;
        let mut lines = header.lines();
        let marker = lines.next()?.strip_prefix("marker=")?;
        let placement = lines.next()?.strip_prefix("placement=")?;
        let version = match placement {
            "boundary" => None,
            "tail" => Some(lines.next()?.strip_prefix("version=")?),
            _ => return None,
        };
        if lines.next().is_some() {
            return None;
        }
        (marker, version)
    } else {
        let rest = text
            .strip_prefix(LEGACY_CONTEXT_PROJECTION_ENVELOPE_PREFIX)?
            .strip_prefix('\n')?;
        // 已发布 v1 没有 version：合法配置正文也可恰好以 version="x" 开头。
        (rest.split('\n').next()?.strip_prefix("marker=")?, None)
    };
    let marker: String = serde_json::from_str(marker_json).ok()?;
    if marker.is_empty() {
        return None;
    }
    if let Some(version) = version_json {
        let decoded: String = serde_json::from_str(version).ok()?;
        if decoded.is_empty() {
            return None;
        }
    }
    Some(ContextProjectionEnvelope {
        marker_json,
        version_json,
    })
}

/// 判断一段正文是否属于"追加式动态上下文"。
///
/// 判据仅为合法 v2 Tail 信封，不从任意正文前缀推断。
/// provider 适配层用它决定缓存断点不落在可变事实后面，从而不必各自按产品 marker
/// 猜正文。
pub fn is_dynamic_context_text(text: &str) -> bool {
    parse_context_projection_envelope(text).is_some_and(|envelope| envelope.is_append_version())
}

/// 消息是否为框架生成的上下文（投影信封或既有运行时注记）。
pub fn is_generated_context_message(message: &Message) -> bool {
    if context_projection_envelope(message).is_some() {
        return true;
    }
    message.content.as_text_ref().is_some_and(|text| {
        text.starts_with("[Horizon compact:")
            || text.starts_with("[Used tools:")
            || text.starts_with(
                "[Result unavailable — tool result was removed during context compression]",
            )
    })
}

/// 由内容派生稳定的投影版本（SHA-256 前 16 hex）。
///
/// 生产者用它表达"内容相同的版本相同"，从而让重入、重试与恢复重投影不重复追加。
/// 版本只是去重键，不是运行状态权威，也不要求跨产品语义一致。
pub fn projection_content_version(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"echo-agent-projection-version-v1\0");
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Generic run metadata available to a pre-model context projector.
#[derive(Debug, Clone)]
pub struct ProjectionContext {
    pub iteration: usize,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
    pub turn_id: Option<String>,
}

/// Produces replaceable context immediately before model input preparation.
pub trait PreModelContextProjector: Send + Sync {
    /// 声明本投影是否为本次模型调用的必要输入。
    /// 默认保留既有 best-effort 行为；选择 true 后 project 的错误由原执行入口返回，
    /// 不清空旧投影后继续调用模型。该声明不承载具体产品规则或新状态权威。
    fn is_required(&self) -> bool {
        false
    }

    fn project<'a>(
        &'a self,
        context: &'a ProjectionContext,
    ) -> BoxFuture<'a, Result<Vec<ContextProjection>>>;

    /// 在最终 ChatRequest 构建后核验投影引用是否能在本次实际工具范围内读取。
    /// 默认允许，保留旧消费者；实现只能核验，不能扩展工具上界或改写请求。
    fn validate_model_request<'a>(
        &'a self,
        _context: &'a ProjectionContext,
        _request: &'a crate::llm::ChatRequest,
        _artifacts: Option<&'a crate::tools::artifact::ToolOutputArtifactConfig>,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// Compression pipeline input
pub struct CompressionInput {
    /// Messages to be compressed
    pub messages: Vec<Message>,
    /// Token limit, triggers compression when exceeded
    pub token_limit: usize,
    /// Current user query — used to protect active task context from eviction
    pub current_query: Option<String>,
    /// Focus instructions — user-provided guidance on what to prioritize in summaries
    pub focus_instructions: Option<String>,
    /// Cancellation for every provider call performed by this compression pass.
    pub cancel_token: Option<CancellationToken>,
    /// Tokenizer shared with the caller's budget calculation.
    pub tokenizer: Option<Arc<dyn Tokenizer>>,
}

impl CompressionInput {
    /// Return the configured tokenizer, falling back to the framework default.
    pub fn tokenizer(&self) -> Arc<dyn Tokenizer> {
        self.tokenizer
            .clone()
            .unwrap_or_else(|| Arc::new(HeuristicTokenizer))
    }
}

/// Compression pipeline output
pub struct CompressionOutput {
    /// Final list of messages to keep and send to the LLM
    pub messages: Vec<Message>,
    /// Messages evicted in this compression pass
    pub evicted: Vec<Message>,
    /// Compression checkpoint for audit, replay, and recovery
    pub checkpoint: Option<CompressionCheckpoint>,
}

/// Records a fix applied to maintain valid tool_call ↔ tool_result pairing after compression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPairFix {
    pub tool_call_id: String,
    pub fix_type: ToolPairFixType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolPairFixType {
    /// An orphaned tool_result (no matching assistant tool_calls) was removed.
    OrphanedResultRemoved,
    /// All tool_calls in an assistant message were cleared (no matching results).
    DanglingCallCleared,
    /// A placeholder result was inserted for a missing tool_result.
    PlaceholderResultInserted,
}

/// A first-class state object capturing the complete result of a compression pass.
///
/// This is the audit trail for context compression — it records what was done,
/// what was removed, what was fixed, and what was promoted to long-term memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionCheckpoint {
    /// Unique identifier for this checkpoint (UUID v4).
    pub checkpoint_id: String,
    /// Name of the compression strategy used (e.g. "SlidingWindow", "Summary").
    pub strategy: String,
    /// Index range [first, last] of messages covered by this compression pass.
    pub covered_range: Option<(usize, usize)>,
    /// LLM-generated summary, if this strategy produces one.
    pub summary: Option<String>,
    /// Number of messages retained after compression.
    pub retained_count: usize,
    /// Number of messages evicted during compression.
    pub evicted_count: usize,
    /// Number of protected messages excluded from compression.
    pub protected_count: usize,
    /// Tool-call pairing fixes applied after compression.
    pub tool_pair_fixes: Vec<ToolPairFix>,
    /// Number of evicted messages sent to the MemoryPromoter for fact extraction.
    /// Note: this counts evicted messages, not actual extracted facts (promoter runs async).
    pub memory_promotion_count: usize,
    /// Estimated tokens before compression.
    pub token_before: usize,
    /// Estimated tokens after compression.
    pub token_after: usize,
    /// Wall-clock duration of the compression pass in milliseconds.
    pub compression_duration_ms: u64,
    /// Timestamp when this checkpoint was created.
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub created_at: DateTime<Utc>,
    /// User-provided focus instructions for this compression, if any.
    pub focus_instructions: Option<String>,
    /// Compression levels applied (relevant for AdaptiveCompressor).
    pub levels_applied: Vec<String>,
}

impl CompressionCheckpoint {
    /// Create a new checkpoint with a random UUID and the current timestamp.
    pub fn new(strategy: impl Into<String>) -> Self {
        Self {
            checkpoint_id: uuid::Uuid::new_v4().to_string(),
            strategy: strategy.into(),
            covered_range: None,
            summary: None,
            retained_count: 0,
            evicted_count: 0,
            protected_count: 0,
            tool_pair_fixes: Vec::new(),
            memory_promotion_count: 0,
            token_before: 0,
            token_after: 0,
            compression_duration_ms: 0,
            created_at: Utc::now(),
            focus_instructions: None,
            levels_applied: Vec::new(),
        }
    }

    /// Builder-style: set the covered message range.
    pub fn with_covered_range(mut self, first: usize, last: usize) -> Self {
        self.covered_range = Some((first, last));
        self
    }

    /// Builder-style: set the summary text.
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Builder-style: set token counts.
    pub fn with_tokens(mut self, before: usize, after: usize) -> Self {
        self.token_before = before;
        self.token_after = after;
        self
    }

    /// Builder-style: set message counts.
    pub fn with_counts(mut self, retained: usize, evicted: usize) -> Self {
        self.retained_count = retained;
        self.evicted_count = evicted;
        self
    }

    /// Builder-style: set protected message count.
    pub fn with_protected_count(mut self, count: usize) -> Self {
        self.protected_count = count;
        self
    }

    /// Builder-style: set memory promotion count.
    pub fn with_memory_promotion_count(mut self, count: usize) -> Self {
        self.memory_promotion_count = count;
        self
    }

    /// Builder-style: set tool pair fixes.
    pub fn with_tool_fixes(mut self, fixes: Vec<ToolPairFix>) -> Self {
        self.tool_pair_fixes = fixes;
        self
    }

    /// Builder-style: set the compression duration.
    pub fn with_duration_ms(mut self, ms: u64) -> Self {
        self.compression_duration_ms = ms;
        self
    }

    /// Builder-style: set focus instructions.
    pub fn with_focus(mut self, focus: Option<String>) -> Self {
        self.focus_instructions = focus;
        self
    }

    /// Builder-style: set levels applied (for AdaptiveCompressor).
    pub fn with_levels(mut self, levels: Vec<String>) -> Self {
        self.levels_applied = levels;
        self
    }
}

/// Structured summary produced by LLM-based compressors.
///
/// Unlike free-text summaries, each field tracks a specific aspect of the
/// conversation. This enables:
/// - **Field-level merge** for incremental compression (no summary drift)
/// - **Verification** — check each field independently
/// - **Programmatic access** — extract file paths, pending tasks, etc.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StructuredSummary {
    /// User's primary goal and intent
    pub goal: String,
    /// The specific task currently being worked on
    pub current_task: String,
    /// Actions that have been completed
    pub completed_actions: Vec<String>,
    /// Tasks that still need to be done
    pub pending_tasks: Vec<String>,
    /// Decisions made during the conversation
    pub decisions: Vec<String>,
    /// File paths referenced or modified
    pub files_touched: Vec<String>,
    /// Errors encountered and their resolutions
    pub errors: Vec<String>,
    /// Summary of key findings from tool outputs
    pub tool_outputs_summary: String,
    /// User preferences or constraints discovered
    pub user_preferences: Vec<String>,
    /// Non-negotiable requirements, safety boundaries, and acceptance criteria
    pub constraints: Vec<String>,
    /// Exact identifiers, values, versions, commands, and other continuation-critical facts
    pub key_facts: Vec<String>,
    /// Suggested next step
    pub next_step: String,
}

impl StructuredSummary {
    /// Parse from a JSON string (LLM response).
    pub fn from_json(json: &str) -> std::result::Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Attempt to extract and parse JSON from an LLM response that may contain
    /// markdown fences or extra text.
    pub fn from_llm_response(response: &str) -> Option<Self> {
        // Try direct parse first
        if let Ok(s) = Self::from_json(response) {
            return Some(s);
        }
        // Try extracting from markdown code fences
        if let Some(json) = extract_json_from_text(response)
            && let Ok(s) = Self::from_json(&json)
        {
            return Some(s);
        }
        None
    }

    /// Serialize to a compact JSON string for storage.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Format as a system message content: `[对话历史摘要]\n{json}`
    pub fn to_system_message(&self) -> String {
        format!("[对话历史摘要]\n{}", self.to_json())
    }

    /// Merge newer summary information into this one (field-level).
    ///
    /// Used by `IncrementalSummaryCompressor` to update the cached summary
    /// without losing information from earlier compression passes.
    pub fn merge_with(&mut self, newer: &StructuredSummary) {
        // goal: track the most recent goal (latest wins, no accumulation noise)
        if !newer.goal.is_empty() && newer.goal != self.goal {
            self.goal = newer.goal.clone();
        }

        // current_task: latest value wins
        if !newer.current_task.is_empty() {
            self.current_task = newer.current_task.clone();
        }

        // completed_actions: append new, deduplicate
        for action in &newer.completed_actions {
            if !self.completed_actions.contains(action) {
                self.completed_actions.push(action.clone());
            }
        }

        // pending_tasks: remove completed ones, add new ones (but not those already completed)
        let completed_set: std::collections::HashSet<&String> =
            newer.completed_actions.iter().collect();
        // Also consider our own completed_actions when deciding what to retain
        let all_completed: std::collections::HashSet<&String> =
            self.completed_actions.iter().collect();
        self.pending_tasks.retain(|t| !completed_set.contains(t));
        for task in &newer.pending_tasks {
            if !self.pending_tasks.contains(task)
                && !completed_set.contains(task)
                && !all_completed.contains(task)
            {
                self.pending_tasks.push(task.clone());
            }
        }

        // decisions: append new, deduplicate
        for decision in &newer.decisions {
            if !self.decisions.contains(decision) {
                self.decisions.push(decision.clone());
            }
        }

        // files_touched: normalize paths, merge and deduplicate
        for file in &newer.files_touched {
            let normalized = normalize_path(file);
            if !self.files_touched.contains(&normalized) {
                self.files_touched.push(normalized);
            }
        }

        // errors: append new, deduplicate
        for error in &newer.errors {
            if !self.errors.contains(error) {
                self.errors.push(error.clone());
            }
        }

        // tool_outputs_summary: latest value wins
        if !newer.tool_outputs_summary.is_empty() {
            self.tool_outputs_summary = newer.tool_outputs_summary.clone();
        }

        // user_preferences: append new, deduplicate
        for pref in &newer.user_preferences {
            if !self.user_preferences.contains(pref) {
                self.user_preferences.push(pref.clone());
            }
        }

        // constraints and key facts are durable anchors: append and deduplicate.
        for constraint in &newer.constraints {
            if !self.constraints.contains(constraint) {
                self.constraints.push(constraint.clone());
            }
        }
        for fact in &newer.key_facts {
            if !self.key_facts.contains(fact) {
                self.key_facts.push(fact.clone());
            }
        }

        // next_step: latest value wins
        if !newer.next_step.is_empty() {
            self.next_step = newer.next_step.clone();
        }
    }
}

/// Tracks canonical context sources that should survive compression.
///
/// When compression evicts messages, critical context (system prompt, project rules,
/// skill injections) may be lost. `CanonicalContext` stores the authoritative sources
/// so they can be re-injected after compression.
#[derive(Debug, Clone, Default)]
pub struct CanonicalContext {
    /// The base system prompt (without skill/rule extensions).
    pub system_prompt: Option<String>,
    /// Project-level rules (AGENT.md / RULES.md content).
    pub project_rules: Option<String>,
    /// System prompt injections from activated skills.
    pub skill_injections: Vec<String>,
    /// Names of currently active skills.
    pub active_skill_names: Vec<String>,
}

impl CanonicalContext {
    /// Returns true if any canonical source is configured.
    pub fn has_any(&self) -> bool {
        self.system_prompt.is_some()
            || self.project_rules.is_some()
            || !self.skill_injections.is_empty()
    }

    /// Build supplemental re-injection messages inserted after compression.
    ///
    /// The base system prompt is restored directly by `ContextManager`; this
    /// method returns only supplemental canonical context so the prompt is not
    /// represented twice.
    /// Returns `None` if there's nothing to re-inject.
    pub fn to_reinjection_messages(&self) -> Option<Vec<String>> {
        if !self.has_any() {
            return None;
        }

        let mut msgs: Vec<String> = Vec::new();

        // Re-inject project rules as a separate system message
        if let Some(ref rules) = self.project_rules {
            let truncated: String = rules.chars().take(2000).collect();
            msgs.push(format!(
                "[Canonical context — project rules restored]:\n{}",
                truncated
            ));
        }

        // List active skills for LLM awareness
        if !self.active_skill_names.is_empty() {
            msgs.push(format!(
                "[Canonical context — active skills]: {}",
                self.active_skill_names.join(", ")
            ));
        }

        Some(msgs)
    }
}

/// Normalize a file path for deduplication: strip leading `./`, trim whitespace.
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if let Some(stripped) = trimmed.strip_prefix("./") {
        stripped.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Extract JSON content from text that may contain markdown fences or extra text.
fn extract_json_from_text(text: &str) -> Option<String> {
    // Try ```json ... ``` code fence
    if let Some(start) = text.find("```json") {
        let after_fence = text.get(start.saturating_add(7)..)?;
        if let Some(end) = after_fence.find("```") {
            return after_fence.get(..end).map(|value| value.trim().to_string());
        }
    }
    // Try ``` ... ``` code fence
    if let Some(start) = text.find("```") {
        let after_fence = text.get(start.saturating_add(3)..)?;
        if let Some(end) = after_fence.find("```") {
            let candidate = after_fence.get(..end)?.trim().to_string();
            if candidate.starts_with('{') {
                return Some(candidate);
            }
        }
    }
    // Try finding { ... } block
    if let Some(start) = text.find('{')
        && let Some(end) = text.rfind('}')
    {
        let candidate = text.get(start..=end)?.to_string();
        if candidate.chars().count() >= 10 {
            return Some(candidate);
        }
    }
    None
}

/// Unified interface for all compression strategies (async, supports `dyn` trait object)
pub trait ContextCompressor: Send + Sync {
    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>>;

    /// Notify optional observation caches only after the context owner accepts
    /// the final transform. Calculation and rejected candidates do not publish state.
    /// Standalone callers may invoke this after accepting CompressionOutput.messages.
    fn context_committed(&self, _messages: &[Message]) {}

    /// Human-readable name of this compressor, used for metrics tracking.
    /// Override in implementations for a descriptive name.
    fn name(&self) -> &str {
        "custom"
    }
}

/// Allows `Box<dyn ContextCompressor>` to be passed directly to any function accepting
/// `impl ContextCompressor`, without introducing an extra wrapper enum.
impl ContextCompressor for Box<dyn ContextCompressor> {
    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        (**self).compress(input)
    }

    fn name(&self) -> &str {
        (**self).name()
    }

    fn context_committed(&self, messages: &[Message]) {
        (**self).context_committed(messages);
    }
}

#[cfg(test)]
mod tests {
    use super::{StructuredSummary, parse_context_projection_envelope};

    fn assert_legacy_boundary_body_is_not_metadata(body: &str) {
        let text = format!("<echo-agent-context-projection-v1>\nmarker=\"config\"\n{body}");
        let header = parse_context_projection_envelope(&text);
        assert!(header.is_some(), "已发布 v1 的任意正文不应抹去框架身份");
        assert!(header.is_some_and(|value| {
            value.marker().as_deref() == Some("config") && !value.is_append_version()
        }));
    }

    #[test]
    fn legacy_v1_version_number_is_boundary_body() {
        assert_legacy_boundary_body_is_not_metadata("version=1\n配置事实🚀");
    }

    #[test]
    fn legacy_v1_version_string_is_boundary_body() {
        for body in ["version=\"x\"\n配置事实🚀", "version=\"\"\n配置事实🚀"] {
            assert_legacy_boundary_body_is_not_metadata(body);
        }
    }

    #[test]
    fn v2_envelope_requires_explicit_terminated_header() {
        for header in [
            "marker=\"config\"\nplacement=tail\nversion=\"v\"",
            "marker=\"config\"\nplacement=boundary\nversion=\"x\"\n\n",
            "marker=\"config\"\nplacement=tail\nversion=\"\"\n\n",
            "marker=\"config\"\nplacement=tail\nversion=1\n\n",
            "marker=\"config\"\nplacement=tail\n\nversion=\"body\"",
            "marker=\"config\"\nplacement=other\n\n",
            "marker=\"config\"\nplacement=boundary\nplacement=tail\n\n",
        ] {
            let text = format!("<echo-agent-context-projection-v2>\n{header}");
            assert!(parse_context_projection_envelope(&text).is_none());
        }
        let text = "<echo-agent-context-projection-v2>\nmarker=\"config\"\nplacement=tail\nversion=\"v\"\n\nversion=\"正文\"";
        let header = parse_context_projection_envelope(text);
        assert!(header.is_some_and(|value| value.version().as_deref() == Some("v")));
    }

    #[test]
    fn structured_summary_accepts_partial_provider_json() -> Result<(), serde_json::Error> {
        let summary = StructuredSummary::from_json(
            r#"{"goal":"ship compaction","constraints":["preserve transcript"]}"#,
        )?;

        assert_eq!(summary.goal, "ship compaction");
        assert_eq!(summary.constraints, vec!["preserve transcript"]);
        assert!(summary.key_facts.is_empty());
        assert!(summary.pending_tasks.is_empty());
        Ok(())
    }

    #[test]
    fn structured_summary_merge_preserves_constraints_and_key_facts() {
        let mut previous = StructuredSummary {
            constraints: vec!["preserve transcript".to_string()],
            key_facts: vec!["window=128000".to_string()],
            ..StructuredSummary::default()
        };
        let newer = StructuredSummary {
            constraints: vec![
                "preserve transcript".to_string(),
                "keep recent raw messages".to_string(),
            ],
            key_facts: vec!["window=128000".to_string(), "keep_recent=20".to_string()],
            ..StructuredSummary::default()
        };

        previous.merge_with(&newer);

        assert_eq!(
            previous.constraints,
            vec!["preserve transcript", "keep recent raw messages"]
        );
        assert_eq!(previous.key_facts, vec!["window=128000", "keep_recent=20"]);
    }
}
