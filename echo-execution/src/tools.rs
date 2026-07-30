//! Tool system core — `ToolManager` and tool trait re-exports.
//!
//! The [`ToolManager`] handles registration, execution, concurrency control,
//! and timeout/retry for all tools in an agent session.
//! Uses `DashMap` internally so it can be shared via `Arc`.

use dashmap::DashMap;
use echo_core::error::{Result, ToolError};
use echo_core::llm::types::ToolDefinition;
use echo_core::sandbox::SandboxExecutor;
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use parking_lot::RwLock;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::Semaphore;

pub use echo_core::tools::{
    Tool, ToolContext, ToolExecutionConfig, ToolFailure, ToolFailureCategory, ToolOutputChannel,
    ToolParameters, ToolRecoveryAction, ToolRegistrar, ToolResult, ToolRiskLevel, ToolSideEffect,
    ToolStreamEvent,
};

fn retry_delay_ms(configured_ms: u64, retry_after_ms: Option<u64>, attempt: u32) -> u64 {
    use std::hash::{Hash, Hasher};

    let exponent = attempt.saturating_sub(1).min(5);
    let base = configured_ms
        .saturating_mul(1_u64 << exponent)
        .max(retry_after_ms.unwrap_or(0))
        .min(30_000);
    if base == 0 {
        return 0;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    attempt.hash(&mut hasher);
    configured_ms.hash(&mut hasher);
    if let Ok(elapsed) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        elapsed.as_nanos().hash(&mut hasher);
    }
    let jitter_cap = base.saturating_div(4).max(1);
    base.saturating_add(hasher.finish() % jitter_cap)
        .min(30_000)
}

impl ToolRegistrar for ToolManager {
    fn register(&mut self, tool: Box<dyn Tool>) {
        ToolManager::register(self, tool);
    }
}

/// 工具管理器 — thread-safe tool registry and executor.
pub struct ToolManager {
    tools: DashMap<String, Box<dyn Tool>>,
    config: ToolExecutionConfig,
    /// Write/execute semaphore (limits concurrent write/execute tools).
    semaphore: Option<Arc<Semaphore>>,
    /// Read semaphore (higher limit for concurrent read-only tools).
    read_semaphore: Option<Arc<Semaphore>>,
    /// Cached tool definitions: `(version, definitions)`.
    /// Invalidated by bumping `definitions_version`; rebuilt lazily on next access.
    /// Uses `parking_lot::RwLock` which does not poison on panic.
    cached_definitions: RwLock<Option<(u64, Vec<ToolDefinition>)>>,
    /// Monotonically increasing version counter. On register/unregister the
    /// version is bumped so that the next read rebuilds from the live tool set.
    definitions_version: AtomicU64,
    /// Tool result cache: (tool_name, params_json) -> ToolResult.
    /// Only caches read-only tool results. Cleared on write operations.
    result_cache: RwLock<HashMap<(String, String), (ToolResult, std::time::Instant)>>,
    budget_metrics: ToolBudgetMetrics,
}

/// Deterministic size accounting for the tool definitions sent to an LLM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSchemaStats {
    pub tool_count: usize,
    pub schema_bytes: usize,
    pub estimated_tokens: usize,
}

/// Content-free aggregate metrics for schema and tool-output budgets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolBudgetMetricsSnapshot {
    pub schema_requests: u64,
    pub schema_bytes: u64,
    pub schema_estimated_tokens: u64,
    pub activated_tool_observations: u64,
    pub tool_searches: u64,
    pub tool_search_matches: u64,
    pub tool_search_misses: u64,
    pub tool_selection_failures: u64,
    pub tool_results: u64,
    pub successful_tool_results: u64,
    pub visible_result_bytes: u64,
    pub spilled_payload_bytes: u64,
    pub tool_duration_ms: u64,
    pub artifact_reads: u64,
    pub paginated_results: u64,
    pub pagination_continuations: u64,
}

#[derive(Default)]
struct ToolBudgetMetrics {
    schema_requests: AtomicU64,
    schema_bytes: AtomicU64,
    schema_estimated_tokens: AtomicU64,
    activated_tool_observations: AtomicU64,
    tool_searches: AtomicU64,
    tool_search_matches: AtomicU64,
    tool_search_misses: AtomicU64,
    tool_selection_failures: AtomicU64,
    tool_results: AtomicU64,
    successful_tool_results: AtomicU64,
    visible_result_bytes: AtomicU64,
    spilled_payload_bytes: AtomicU64,
    tool_duration_ms: AtomicU64,
    artifact_reads: AtomicU64,
    paginated_results: AtomicU64,
    pagination_continuations: AtomicU64,
}

/// Catalog search that activates full schemas inside one invocation.
pub struct ToolSearchTool {
    manager: std::sync::Weak<ToolManager>,
}

impl ToolSearchTool {
    pub fn new(manager: std::sync::Weak<ToolManager>) -> Self {
        Self { manager }
    }
}

impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "tool_search"
    }

    fn description(&self) -> &str {
        "Find and activate additional tools by capability or exact name. Matching tool schemas become available on the next model turn."
    }

    fn risk_level(&self) -> ToolRiskLevel {
        // Activation mutates invocation-local visibility, so this must bypass
        // the read-only result cache even though it has no external side effect.
        ToolRiskLevel::Standard
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Capability or exact tool name to find"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10,
                    "description": "Maximum tools to activate (default: 5)"
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a ToolContext,
    ) -> futures::future::BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let query = parameters
                .get("query")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ToolError::MissingParameter("query".to_string()))?;
            let limit = parameters
                .get("limit")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(5)
                .clamp(1, 10);
            let Some(manager) = self.manager.upgrade() else {
                return Ok(ToolResult::error("Tool catalog is no longer available"));
            };
            let query_lower = query.to_lowercase();
            let query_terms = query_lower.split_whitespace().collect::<Vec<_>>();
            let mut matches = manager
                .get_openai_tools()
                .into_iter()
                .filter(|definition| definition.function.name != self.name())
                .filter(|definition| {
                    ctx.tool_visibility
                        .as_ref()
                        .is_none_or(|visibility| visibility.is_eligible(&definition.function.name))
                })
                .filter_map(|definition| {
                    let name_lower = definition.function.name.to_lowercase();
                    let description_lower = definition.function.description.to_lowercase();
                    let score = if name_lower == query_lower {
                        Some(0_u8)
                    } else if name_lower.starts_with(&query_lower) {
                        Some(1)
                    } else if name_lower.contains(&query_lower) {
                        Some(2)
                    } else if query_terms
                        .iter()
                        .all(|term| name_lower.contains(term) || description_lower.contains(term))
                    {
                        Some(3)
                    } else {
                        None
                    }?;
                    Some((score, definition))
                })
                .collect::<Vec<_>>();
            matches.sort_by(|left, right| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| left.1.function.name.cmp(&right.1.function.name))
            });
            let total_matches = matches.len();
            matches.truncate(limit);
            let names = matches
                .iter()
                .map(|(_, definition)| definition.function.name.clone())
                .collect::<Vec<_>>();
            let activated = ctx
                .tool_visibility
                .as_ref()
                .map(|visibility| visibility.activate(names.clone()))
                .unwrap_or_default();
            manager.record_tool_search(total_matches, activated.len());

            if matches.is_empty() {
                return Ok(ToolResult::success(format!(
                    "No eligible tools matched '{query}'. Try a concrete capability or tool name."
                )));
            }

            let lines = matches
                .iter()
                .map(|(_, definition)| {
                    let preview = definition
                        .function
                        .description
                        .chars()
                        .take(160)
                        .collect::<String>();
                    format!("- {}: {preview}", definition.function.name)
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut result = ToolResult::success(format!(
                "Activated tool schemas for the next turn:\n{lines}"
            ))
            .with_truncated(total_matches > limit);
            result
                .metadata
                .insert("matched_tools".to_string(), names.join(","));
            result
                .metadata
                .insert("activated_tools".to_string(), activated.join(","));
            result
                .metadata
                .insert("total_known".to_string(), "true".to_string());
            result
                .metadata
                .insert("total_matches".to_string(), total_matches.to_string());
            Ok(result)
        })
    }
}

impl ToolManager {
    pub fn get_openai_tools(&self) -> Vec<ToolDefinition> {
        let current_version = self.definitions_version.load(Ordering::Acquire);
        if let Some(ref cached) = *self.cached_definitions.read()
            && cached.0 == current_version
        {
            return cached.1.clone();
        }
        // Version mismatch or cache empty — rebuild.
        let mut definitions: Vec<ToolDefinition> = self
            .tools
            .iter()
            .map(|entry| ToolDefinition::from_tool(&**entry.value()))
            .collect();
        // Sort by tool name to ensure deterministic order for prefix caching.
        // This enables LLM provider-side prefix caching (OpenAI, DeepSeek, Anthropic)
        // because consecutive requests with the same tool definitions share a stable
        // cacheable prefix: system prompt → sorted tool definitions → conversation history.
        definitions.sort_by(|a, b| a.function.name.cmp(&b.function.name));
        *self.cached_definitions.write() = Some((current_version, definitions.clone()));
        definitions
    }

    /// Measure the complete registered tool schema using the framework tokenizer.
    pub fn schema_stats(&self) -> std::result::Result<ToolSchemaStats, serde_json::Error> {
        Self::schema_stats_for(&self.get_openai_tools())
    }

    /// Measure an invocation-filtered subset without creating another registry.
    pub fn schema_stats_for(
        definitions: &[ToolDefinition],
    ) -> std::result::Result<ToolSchemaStats, serde_json::Error> {
        let mut definitions = definitions.to_vec();
        definitions.sort_by(|left, right| left.function.name.cmp(&right.function.name));
        let serialized = serde_json::to_string(&definitions)?;
        Ok(ToolSchemaStats {
            tool_count: definitions.len(),
            schema_bytes: serialized.len(),
            estimated_tokens: HeuristicTokenizer.count_tokens(&serialized),
        })
    }

    pub fn record_schema_stats(&self, stats: &ToolSchemaStats) {
        self.budget_metrics
            .schema_requests
            .fetch_add(1, Ordering::Relaxed);
        add_usize(&self.budget_metrics.schema_bytes, stats.schema_bytes);
        add_usize(
            &self.budget_metrics.schema_estimated_tokens,
            stats.estimated_tokens,
        );
        add_usize(
            &self.budget_metrics.activated_tool_observations,
            stats.tool_count,
        );
    }

    pub fn record_tool_search(&self, matched: usize, activated: usize) {
        self.budget_metrics
            .tool_searches
            .fetch_add(1, Ordering::Relaxed);
        add_usize(&self.budget_metrics.tool_search_matches, matched);
        if matched == 0 || activated == 0 {
            self.budget_metrics
                .tool_search_misses
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_tool_selection_failure(&self) {
        self.budget_metrics
            .tool_selection_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_tool_result(
        &self,
        tool_name: &str,
        result: &ToolResult,
        visible_bytes: usize,
        duration_ms: u64,
    ) {
        self.budget_metrics
            .tool_results
            .fetch_add(1, Ordering::Relaxed);
        if result.success {
            self.budget_metrics
                .successful_tool_results
                .fetch_add(1, Ordering::Relaxed);
        }
        add_usize(&self.budget_metrics.visible_result_bytes, visible_bytes);
        let spilled_bytes = result
            .metadata
            .get("artifact_payload_bytes")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        self.budget_metrics
            .spilled_payload_bytes
            .fetch_add(spilled_bytes, Ordering::Relaxed);
        self.budget_metrics
            .tool_duration_ms
            .fetch_add(duration_ms, Ordering::Relaxed);
        let paginated = result.metadata.contains_key("page.returned");
        if tool_name == "read_artifact" && result.success {
            self.budget_metrics
                .artifact_reads
                .fetch_add(1, Ordering::Relaxed);
        }
        if paginated {
            self.budget_metrics
                .paginated_results
                .fetch_add(1, Ordering::Relaxed);
            if result
                .metadata
                .get("page.truncated")
                .is_some_and(|value| value == "true")
            {
                self.budget_metrics
                    .pagination_continuations
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        tracing::info!(
            target: "echo_agent::tool_budget",
            tool = tool_name,
            success = result.success,
            visible_bytes,
            spilled_payload_bytes = spilled_bytes,
            duration_ms,
            paginated,
            artifact_sha256 = result
                .metadata
                .get("artifact_sha256")
                .map(String::as_str)
                .unwrap_or(""),
            "tool result budget"
        );
    }

    pub fn budget_metrics(&self) -> ToolBudgetMetricsSnapshot {
        ToolBudgetMetricsSnapshot {
            schema_requests: self.budget_metrics.schema_requests.load(Ordering::Relaxed),
            schema_bytes: self.budget_metrics.schema_bytes.load(Ordering::Relaxed),
            schema_estimated_tokens: self
                .budget_metrics
                .schema_estimated_tokens
                .load(Ordering::Relaxed),
            activated_tool_observations: self
                .budget_metrics
                .activated_tool_observations
                .load(Ordering::Relaxed),
            tool_searches: self.budget_metrics.tool_searches.load(Ordering::Relaxed),
            tool_search_matches: self
                .budget_metrics
                .tool_search_matches
                .load(Ordering::Relaxed),
            tool_search_misses: self
                .budget_metrics
                .tool_search_misses
                .load(Ordering::Relaxed),
            tool_selection_failures: self
                .budget_metrics
                .tool_selection_failures
                .load(Ordering::Relaxed),
            tool_results: self.budget_metrics.tool_results.load(Ordering::Relaxed),
            successful_tool_results: self
                .budget_metrics
                .successful_tool_results
                .load(Ordering::Relaxed),
            visible_result_bytes: self
                .budget_metrics
                .visible_result_bytes
                .load(Ordering::Relaxed),
            spilled_payload_bytes: self
                .budget_metrics
                .spilled_payload_bytes
                .load(Ordering::Relaxed),
            tool_duration_ms: self.budget_metrics.tool_duration_ms.load(Ordering::Relaxed),
            artifact_reads: self.budget_metrics.artifact_reads.load(Ordering::Relaxed),
            paginated_results: self
                .budget_metrics
                .paginated_results
                .load(Ordering::Relaxed),
            pagination_continuations: self
                .budget_metrics
                .pagination_continuations
                .load(Ordering::Relaxed),
        }
    }

    fn invalidate_cache(&self) {
        self.definitions_version.fetch_add(1, Ordering::Release);
        // No need to clear cached_definitions — version mismatch will trigger
        // a lazy rebuild on the next access.
    }

    /// Force the cached tool definitions to be rebuilt on the next read.
    ///
    /// This is useful when a tool's JSON schema depends on external runtime
    /// metadata while the registered tool set itself has not changed.
    pub fn invalidate_definition_cache(&self) {
        self.invalidate_cache();
    }
}

impl Default for ToolManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolManager {
    pub fn new() -> Self {
        Self {
            tools: DashMap::new(),
            semaphore: None,
            read_semaphore: None,
            config: ToolExecutionConfig::default(),
            cached_definitions: RwLock::new(None),
            definitions_version: AtomicU64::new(0),
            result_cache: RwLock::new(HashMap::new()),
            budget_metrics: ToolBudgetMetrics::default(),
        }
    }

    pub fn new_with_config(config: ToolExecutionConfig) -> Self {
        let semaphore = config
            .max_concurrency
            .map(|n| Arc::new(Semaphore::new(n.max(1))));
        let read_semaphore = config
            .max_read_concurrency
            .map(|n| Arc::new(Semaphore::new(n.max(1))));
        Self {
            tools: DashMap::new(),
            semaphore,
            read_semaphore,
            config,
            cached_definitions: RwLock::new(None),
            definitions_version: AtomicU64::new(0),
            result_cache: RwLock::new(HashMap::new()),
            budget_metrics: ToolBudgetMetrics::default(),
        }
    }

    pub fn max_concurrency(&self) -> Option<usize> {
        self.config.max_concurrency
    }

    /// Register a tool (takes `&self` via DashMap interior mutability).
    pub fn register(&self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
        self.invalidate_cache();
    }

    pub fn register_tools(&self, tools: Vec<Box<dyn Tool>>) {
        for tool in tools {
            self.tools.insert(tool.name().to_string(), tool);
        }
        self.invalidate_cache();
    }

    pub fn unregister(&self, tool_name: &str) -> Option<Box<dyn Tool>> {
        let tool = self.tools.remove(tool_name).map(|(_, v)| v);
        if tool.is_some() {
            self.invalidate_cache();
        }
        tool
    }

    pub fn list_tools(&self) -> Vec<String> {
        let mut tools: Vec<String> = self.tools.iter().map(|e| e.key().clone()).collect();
        tools.sort();
        tools
    }

    /// Inject a sandbox executor into all registered tools that support it.
    ///
    /// Iterates the [`DashMap`] with `iter_mut()`, calling
    /// [`Tool::set_sandbox`] on each tool. Tools that override the method
    /// (currently `ShellTool` and `RunCodeTool`) accept the executor;
    /// all others ignore it via the default `false` implementation.
    ///
    /// Called from [`set_sandbox_manager`] at agent-setup time (P2).
    pub fn apply_sandbox(&self, sandbox: Arc<dyn SandboxExecutor>) {
        for mut entry in self.tools.iter_mut() {
            entry.value_mut().set_sandbox(sandbox.clone());
        }
    }

    /// Get a reference to a tool (via DashMap's Ref).
    pub fn get_tool(
        &self,
        tool_name: &str,
    ) -> Option<dashmap::mapref::one::Ref<'_, String, Box<dyn Tool>>> {
        self.tools.get(tool_name)
    }

    pub fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions: Vec<ToolDefinition> = self
            .tools
            .iter()
            .map(|entry| ToolDefinition::from_tool(&**entry.value()))
            .collect();
        definitions.sort_by(|a, b| a.function.name.cmp(&b.function.name));
        definitions
    }

    /// 执行工具
    ///
    /// 支持并发控制、超时和重试。等价于以空 [`ToolContext`] 调用
    /// [`Self::execute_tool_with_context`]（向后兼容）。
    pub async fn execute_tool(
        &self,
        tool_name: &str,
        parameters: ToolParameters,
    ) -> Result<ToolResult> {
        self.execute_tool_inner(tool_name, parameters, &ToolContext::default())
            .await
    }

    /// 带运行时上下文执行工具。
    ///
    /// [`ToolManager`] 本身不持有任何 `ToolContext` 状态 —— `ctx` 由调用方
    /// （ExecuteStage）每次传入，从 per-agent 的 config 构造。这保证了
    /// 跨会话共享同一个 `ToolManager`（如 AgentPool 的 pooled agent）也
    /// 不会串会话：每个 agent 的 `working_dir` 只跟随它自己的 ctx。
    pub async fn execute_tool_with_context(
        &self,
        tool_name: &str,
        parameters: ToolParameters,
        ctx: &ToolContext,
    ) -> Result<ToolResult> {
        self.execute_tool_inner(tool_name, parameters, ctx).await
    }

    /// Shared body of [`Self::execute_tool`] / [`Self::execute_tool_with_context`]:
    /// 并发控制、超时、重试、结果缓存，最终通过
    /// [`Tool::execute_with_context`] 路由到具体工具。
    async fn execute_tool_inner(
        &self,
        tool_name: &str,
        parameters: ToolParameters,
        ctx: &ToolContext,
    ) -> Result<ToolResult> {
        let tool = self
            .get_tool(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;

        // 并发控制：获取信号量许可（读/写分离）
        let is_read = tool.risk_level() == ToolRiskLevel::ReadOnly;

        // Check result cache for read-only tools
        if is_read {
            let params_json = serde_json::to_string(&parameters).unwrap_or_default();
            let cache_key = (tool_name.to_string(), params_json);
            if let Some((result, ts)) = self.result_cache.read().get(&cache_key)
                && ts.elapsed() < std::time::Duration::from_secs(60)
            {
                tracing::debug!("Tool result cache hit: {tool_name}");
                return Ok(result.clone());
            }
        }

        let _permit = if is_read {
            if let Some(sem) = &self.read_semaphore {
                sem.acquire().await.ok()
            } else {
                None
            }
        } else {
            if let Some(sem) = &self.semaphore {
                match sem.acquire().await {
                    Ok(permit) => Some(permit),
                    Err(e) => {
                        tracing::warn!("Failed to acquire semaphore permit: {}", e);
                        return Err(ToolError::ExecutionFailed {
                            tool: tool_name.to_string(),
                            message: format!("Concurrency limit error: {}", e),
                        }
                        .into());
                    }
                }
            } else {
                None
            }
        };

        let max_retries = if self.config.retry_on_fail {
            self.config.max_retries
        } else {
            0
        };

        let mut last_err: Option<echo_core::error::ReactError> = None;
        let mut next_retry_after_ms = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay_ms = retry_delay_ms(
                    self.config.retry_delay_ms,
                    next_retry_after_ms.take(),
                    attempt,
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            let result = if self.config.timeout_ms > 0 && !tool.manages_own_timeout() {
                match tokio::time::timeout(
                    Duration::from_millis(self.config.timeout_ms),
                    tool.execute_with_context(parameters.clone(), ctx),
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => Err(ToolError::Timeout(tool_name.to_string()).into()),
                }
            } else {
                tool.execute_with_context(parameters.clone(), ctx).await
            };

            match result {
                Ok(result) if result.success => return Ok(result),
                Ok(result)
                    if attempt < max_retries
                        && result
                            .failure
                            .as_ref()
                            .is_some_and(ToolFailure::allows_automatic_retry) =>
                {
                    next_retry_after_ms = result
                        .failure
                        .as_ref()
                        .and_then(|failure| failure.retry_after_ms);
                }
                Ok(result) => return Ok(result),
                Err(error) if attempt < max_retries => {
                    let failure = ToolFailure::from_error(&error, !is_read);
                    if failure.allows_automatic_retry() {
                        last_err = Some(error);
                    } else {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }

        Err(last_err.unwrap_or_else(|| ToolError::NotFound(tool_name.to_string()).into()))
    }

    /// Validate tool parameters asynchronously.
    ///
    /// This is the preferred method — it works correctly inside a Tokio runtime.
    pub async fn validate_tool_parameters_async(
        &self,
        tool_name: &str,
        parameters: &ToolParameters,
    ) -> Result<()> {
        let tool = self
            .get_tool(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;
        tool.validate_parameters(parameters).await
    }

    /// Whether the named tool supports streaming execution via [`Tool::execute_stream`].
    ///
    /// Returns `false` for unknown tools (they don't exist).
    pub fn supports_streaming(&self, tool_name: &str) -> bool {
        match self.get_tool(tool_name) {
            Some(tool) => tool.supports_streaming(),
            None => false,
        }
    }

    /// Execute a tool while forwarding incremental events with backpressure.
    ///
    /// The concurrency permit and per-attempt timeout cover the complete
    /// stream lifetime. Attempts are retried only until the first output chunk
    /// has been delivered to the caller.
    pub async fn execute_tool_stream_with_context(
        &self,
        tool_name: &str,
        parameters: ToolParameters,
        ctx: &ToolContext,
        event_tx: Option<tokio::sync::mpsc::Sender<ToolStreamEvent>>,
    ) -> Result<ToolResult> {
        let tool = self
            .get_tool(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;

        let is_read = tool.risk_level() == ToolRiskLevel::ReadOnly;

        let _permit = if is_read {
            if let Some(sem) = &self.read_semaphore {
                match sem.acquire().await {
                    Ok(permit) => Some(permit),
                    Err(e) => {
                        return Err(ToolError::ExecutionFailed {
                            tool: tool_name.to_string(),
                            message: format!("Concurrency limit error: {e}"),
                        }
                        .into());
                    }
                }
            } else {
                None
            }
        } else {
            if let Some(sem) = &self.semaphore {
                match sem.acquire().await {
                    Ok(permit) => Some(permit),
                    Err(e) => {
                        tracing::warn!("Failed to acquire semaphore permit: {}", e);
                        return Err(ToolError::ExecutionFailed {
                            tool: tool_name.to_string(),
                            message: format!("Concurrency limit error: {}", e),
                        }
                        .into());
                    }
                }
            } else {
                None
            }
        };

        let max_retries = if self.config.retry_on_fail {
            self.config.max_retries
        } else {
            0
        };
        let mut last_err: Option<echo_core::error::ReactError> = None;
        let mut next_retry_after_ms = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay_ms = retry_delay_ms(
                    self.config.retry_delay_ms,
                    next_retry_after_ms.take(),
                    attempt,
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            let mut output_forwarded = false;
            let consume_stream = async {
                use futures::StreamExt;

                let mut stream = tool
                    .execute_stream_with_context(parameters.clone(), ctx)
                    .await?;
                while let Some(event) = stream.next().await {
                    match event {
                        ToolStreamEvent::Complete(result) => return Ok(result),
                        event @ ToolStreamEvent::Output { .. } => {
                            output_forwarded = true;
                            if let Some(tx) = event_tx.as_ref() {
                                tx.send(event)
                                    .await
                                    .map_err(|_| ToolError::ExecutionFailed {
                                        tool: tool_name.to_string(),
                                        message: "Tool stream receiver closed".into(),
                                    })?;
                            }
                        }
                        event @ ToolStreamEvent::Progress { .. } => {
                            if let Some(tx) = event_tx.as_ref() {
                                tx.send(event)
                                    .await
                                    .map_err(|_| ToolError::ExecutionFailed {
                                        tool: tool_name.to_string(),
                                        message: "Tool stream receiver closed".into(),
                                    })?;
                            }
                        }
                    }
                }
                Err(ToolError::ExecutionFailed {
                    tool: tool_name.to_string(),
                    message: "Tool stream ended without a Complete event".into(),
                }
                .into())
            };

            let result = if self.config.timeout_ms > 0 && !tool.manages_own_timeout() {
                match tokio::time::timeout(
                    Duration::from_millis(self.config.timeout_ms),
                    consume_stream,
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(ToolError::Timeout(tool_name.to_string()).into()),
                }
            } else {
                consume_stream.await
            };

            match result {
                Ok(result) if result.success => return Ok(result),
                Ok(result)
                    if attempt < max_retries
                        && !output_forwarded
                        && result
                            .failure
                            .as_ref()
                            .is_some_and(ToolFailure::allows_automatic_retry) =>
                {
                    next_retry_after_ms = result
                        .failure
                        .as_ref()
                        .and_then(|failure| failure.retry_after_ms);
                }
                Ok(result) => return Ok(result),
                Err(error) if attempt < max_retries && !output_forwarded => {
                    let failure = ToolFailure::from_error(&error, !is_read);
                    if failure.allows_automatic_retry() {
                        last_err = Some(error);
                    } else {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ToolError::ExecutionFailed {
                tool: tool_name.to_string(),
                message: "Tool stream execution failed without an error".into(),
            }
            .into()
        }))
    }
}

fn add_usize(counter: &AtomicU64, value: usize) {
    counter.fetch_add(u64::try_from(value).unwrap_or(u64::MAX), Ordering::Relaxed);
}

#[cfg(test)]
mod execute_with_context_tests {
    use super::*;
    use echo_core::tools::{
        Tool, ToolContext, ToolOutputChannel, ToolParameters, ToolResult, ToolStreamEvent,
    };
    use futures::Stream;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::{Arc, Mutex};

    /// Records the `ToolContext` it was called with, so we can verify
    /// `execute_tool_with_context` actually forwards the caller-supplied ctx.
    struct CtxCapturingTool {
        captured: Arc<Mutex<Option<ToolContext>>>,
    }

    struct NamedTool {
        name: &'static str,
    }

    struct DelayedStreamingTool;

    struct InternallyTimedTool;

    impl Tool for InternallyTimedTool {
        fn name(&self) -> &str {
            "internally_timed"
        }

        fn description(&self) -> &str {
            "long-running tool with its own deadline"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _params: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(25)).await;
                Ok(ToolResult::success("completed under internal deadline"))
            })
        }

        fn exempt_from_batch_timeout(&self) -> bool {
            true
        }
    }

    struct RetryOnceTool {
        calls: Arc<AtomicUsize>,
    }

    impl Tool for RetryOnceTool {
        fn name(&self) -> &str {
            "retry_once"
        }

        fn description(&self) -> &str {
            "returns one transient failure, then succeeds"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _params: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async move {
                let call = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                if call == 0 {
                    return Ok(ToolResult::error("temporary outage").with_failure(
                        ToolFailure::new(ToolFailureCategory::Transient).retryable(),
                    ));
                }
                Ok(ToolResult::success("recovered"))
            })
        }
    }

    struct InvalidArgumentsTool {
        calls: Arc<AtomicUsize>,
    }

    impl Tool for InvalidArgumentsTool {
        fn name(&self) -> &str {
            "invalid_arguments"
        }

        fn description(&self) -> &str {
            "always rejects the arguments"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _params: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async move {
                self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(ToolResult::failure(
                    ToolFailureCategory::InvalidArguments,
                    "missing query",
                ))
            })
        }
    }

    struct OutputThenTransientFailureTool {
        calls: Arc<AtomicUsize>,
    }

    impl Tool for OutputThenTransientFailureTool {
        fn name(&self) -> &str {
            "output_then_fail"
        }

        fn description(&self) -> &str {
            "emits output before a transient failure"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _params: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async { Ok(ToolResult::success("unused")) })
        }

        fn execute_stream_with_context<'a>(
            &'a self,
            _params: ToolParameters,
            _ctx: &ToolContext,
        ) -> futures::future::BoxFuture<
            'a,
            echo_core::error::Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>,
        > {
            Box::pin(async move {
                self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                let failure = ToolResult::error("temporary outage")
                    .with_failure(ToolFailure::new(ToolFailureCategory::Transient).retryable());
                Ok(Box::pin(futures::stream::iter(vec![
                    ToolStreamEvent::Output {
                        channel: ToolOutputChannel::Stdout,
                        chunk: "partial".to_string(),
                    },
                    ToolStreamEvent::Complete(failure),
                ]))
                    as Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send>>)
            })
        }

        fn supports_streaming(&self) -> bool {
            true
        }
    }

    impl Tool for DelayedStreamingTool {
        fn name(&self) -> &str {
            "delayed_stream"
        }

        fn description(&self) -> &str {
            "emits output before completing"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _p: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async { Ok(ToolResult::success("done")) })
        }

        fn execute_stream_with_context<'a>(
            &'a self,
            _params: ToolParameters,
            _ctx: &ToolContext,
        ) -> futures::future::BoxFuture<
            'a,
            echo_core::error::Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>,
        > {
            Box::pin(async {
                let events = futures::stream::unfold(0_u8, |state| async move {
                    match state {
                        0 => Some((
                            ToolStreamEvent::Output {
                                channel: ToolOutputChannel::Stdout,
                                chunk: "first".into(),
                            },
                            1,
                        )),
                        1 => {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            Some((ToolStreamEvent::Complete(ToolResult::success("done")), 2))
                        }
                        _ => None,
                    }
                });
                Ok(Box::pin(events) as Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>)
            })
        }

        fn supports_streaming(&self) -> bool {
            true
        }
    }

    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "test tool"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _p: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async { Ok(ToolResult::success("ok")) })
        }
    }

    impl Tool for CtxCapturingTool {
        fn name(&self) -> &str {
            "capture"
        }
        fn description(&self) -> &str {
            "captures the ctx passed to execute_with_context"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn execute<'a>(
            &'a self,
            _p: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            unreachable!("should route through execute_with_context")
        }
        fn execute_with_context<'a>(
            &'a self,
            _p: ToolParameters,
            ctx: &'a ToolContext,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            let cap = self.captured.clone();
            Box::pin(async move {
                *cap.lock().unwrap() = Some(ctx.clone());
                Ok(ToolResult::success("ok"))
            })
        }
    }

    #[tokio::test]
    async fn test_execute_tool_with_context_forwards_ctx() {
        let tm = ToolManager::new();
        let captured = Arc::new(Mutex::new(None));
        tm.register(Box::new(CtxCapturingTool {
            captured: captured.clone(),
        }));

        let ctx = ToolContext {
            working_dir: Some(PathBuf::from("/wt/x")),
            conversation_id: Some("c".into()),
            run_id: Some("r".into()),
            ..Default::default()
        };
        tm.execute_tool_with_context("capture", ToolParameters::new(), &ctx)
            .await
            .unwrap();

        let got = captured.lock().unwrap().clone().expect("ctx not captured");
        assert_eq!(
            got.working_dir.as_deref(),
            Some(std::path::Path::new("/wt/x"))
        );
        assert_eq!(got.conversation_id.as_deref(), Some("c"));
        assert_eq!(got.run_id.as_deref(), Some("r"));
    }

    #[tokio::test]
    async fn streaming_output_is_forwarded_before_execution_completes() {
        let manager = Arc::new(ToolManager::new());
        manager.register(Box::new(DelayedStreamingTool));
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);
        let task_manager = Arc::clone(&manager);

        let handle = tokio::spawn(async move {
            task_manager
                .execute_tool_stream_with_context(
                    "delayed_stream",
                    ToolParameters::new(),
                    &ToolContext::default(),
                    Some(event_tx),
                )
                .await
        });

        let event = tokio::time::timeout(Duration::from_millis(50), event_rx.recv())
            .await
            .expect("first output should arrive promptly")
            .expect("stream channel should remain open");
        assert!(matches!(
            event,
            ToolStreamEvent::Output {
                channel: ToolOutputChannel::Stdout,
                ref chunk,
            } if chunk == "first"
        ));
        assert!(!handle.is_finished());

        let result = handle
            .await
            .expect("streaming task should join")
            .expect("streaming tool should complete");
        assert_eq!(result.output, "done");
    }

    #[tokio::test]
    async fn internally_timed_tool_bypasses_ordinary_execution_timeout()
    -> echo_core::error::Result<()> {
        let manager = ToolManager::new_with_config(ToolExecutionConfig {
            timeout_ms: 1,
            retry_on_fail: false,
            max_retries: 0,
            retry_delay_ms: 0,
            max_concurrency: None,
            max_read_concurrency: None,
        });
        manager.register(Box::new(InternallyTimedTool));

        let result = manager
            .execute_tool("internally_timed", ToolParameters::new())
            .await?;

        assert!(result.success);
        assert_eq!(result.output, "completed under internal deadline");
        Ok(())
    }

    #[tokio::test]
    async fn internally_timed_tool_stream_bypasses_ordinary_execution_timeout()
    -> echo_core::error::Result<()> {
        let manager = ToolManager::new_with_config(ToolExecutionConfig {
            timeout_ms: 1,
            retry_on_fail: false,
            max_retries: 0,
            retry_delay_ms: 0,
            max_concurrency: None,
            max_read_concurrency: None,
        });
        manager.register(Box::new(InternallyTimedTool));

        let result = manager
            .execute_tool_stream_with_context(
                "internally_timed",
                ToolParameters::new(),
                &ToolContext::default(),
                None,
            )
            .await?;

        assert!(result.success);
        assert_eq!(result.output, "completed under internal deadline");
        Ok(())
    }

    #[test]
    fn budget_metrics_store_only_content_free_aggregates() {
        let manager = ToolManager::new();
        manager.record_schema_stats(&ToolSchemaStats {
            tool_count: 3,
            schema_bytes: 120,
            estimated_tokens: 30,
        });
        manager.record_tool_search(2, 1);
        manager.record_tool_selection_failure();
        let mut result = ToolResult::success("sensitive output is not retained");
        result
            .metadata
            .insert("page.returned".to_string(), "2".to_string());
        result
            .metadata
            .insert("page.truncated".to_string(), "true".to_string());
        result
            .metadata
            .insert("artifact_payload_bytes".to_string(), "4096".to_string());
        manager.record_tool_result("read_artifact", &result, 128, 25);

        assert_eq!(
            manager.budget_metrics(),
            ToolBudgetMetricsSnapshot {
                schema_requests: 1,
                schema_bytes: 120,
                schema_estimated_tokens: 30,
                activated_tool_observations: 3,
                tool_searches: 1,
                tool_search_matches: 2,
                tool_search_misses: 0,
                tool_selection_failures: 1,
                tool_results: 1,
                successful_tool_results: 1,
                visible_result_bytes: 128,
                spilled_payload_bytes: 4096,
                tool_duration_ms: 25,
                artifact_reads: 1,
                paginated_results: 1,
                pagination_continuations: 1,
            }
        );
    }

    #[tokio::test]
    async fn retries_only_explicit_transient_failures() -> echo_core::error::Result<()> {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = ToolManager::new_with_config(ToolExecutionConfig {
            timeout_ms: 0,
            retry_on_fail: true,
            max_retries: 2,
            retry_delay_ms: 0,
            max_concurrency: None,
            max_read_concurrency: None,
        });
        manager.register(Box::new(RetryOnceTool {
            calls: Arc::clone(&calls),
        }));

        let result = manager
            .execute_tool("retry_once", ToolParameters::new())
            .await?;

        assert!(result.success);
        assert_eq!(result.output, "recovered");
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        Ok(())
    }

    #[tokio::test]
    async fn invalid_arguments_are_not_retried() -> echo_core::error::Result<()> {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = ToolManager::new_with_config(ToolExecutionConfig {
            timeout_ms: 0,
            retry_on_fail: true,
            max_retries: 2,
            retry_delay_ms: 0,
            max_concurrency: None,
            max_read_concurrency: None,
        });
        manager.register(Box::new(InvalidArgumentsTool {
            calls: Arc::clone(&calls),
        }));

        let result = manager
            .execute_tool("invalid_arguments", ToolParameters::new())
            .await?;

        assert!(!result.success);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(
            result.failure.map(|failure| failure.category),
            Some(ToolFailureCategory::InvalidArguments)
        );
        Ok(())
    }

    #[tokio::test]
    async fn streaming_output_prevents_automatic_retry() -> echo_core::error::Result<()> {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = ToolManager::new_with_config(ToolExecutionConfig {
            timeout_ms: 0,
            retry_on_fail: true,
            max_retries: 2,
            retry_delay_ms: 0,
            max_concurrency: None,
            max_read_concurrency: None,
        });
        manager.register(Box::new(OutputThenTransientFailureTool {
            calls: Arc::clone(&calls),
        }));
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);

        let result = manager
            .execute_tool_stream_with_context(
                "output_then_fail",
                ToolParameters::new(),
                &ToolContext::default(),
                Some(event_tx),
            )
            .await?;
        let event = event_rx.recv().await;

        assert!(matches!(event, Some(ToolStreamEvent::Output { .. })));
        assert!(!result.success);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn test_tool_lists_are_deterministically_sorted() {
        let tm = ToolManager::new();
        tm.register(Box::new(NamedTool { name: "zeta" }));
        tm.register(Box::new(NamedTool { name: "alpha" }));
        tm.register(Box::new(NamedTool { name: "middle" }));

        assert_eq!(tm.list_tools(), vec!["alpha", "middle", "zeta"]);

        let definition_names: Vec<String> = tm
            .get_tool_definitions()
            .into_iter()
            .map(|definition| definition.function.name)
            .collect();
        assert_eq!(definition_names, vec!["alpha", "middle", "zeta"]);

        let openai_names: Vec<String> = tm
            .get_openai_tools()
            .into_iter()
            .map(|definition| definition.function.name)
            .collect();
        assert_eq!(openai_names, vec!["alpha", "middle", "zeta"]);

        let stats = tm.schema_stats().unwrap_or(ToolSchemaStats {
            tool_count: 0,
            schema_bytes: 0,
            estimated_tokens: 0,
        });
        assert_eq!(stats.tool_count, 3);
        assert!(stats.schema_bytes > 0);
        assert!(stats.estimated_tokens > 0);

        let mut reversed = tm.get_openai_tools();
        reversed.reverse();
        assert_eq!(ToolManager::schema_stats_for(&reversed).ok(), Some(stats));
    }

    #[tokio::test]
    async fn tool_search_activates_only_eligible_matches() -> echo_core::error::Result<()> {
        let manager = Arc::new(ToolManager::new());
        manager.register(Box::new(NamedTool { name: "git_status" }));
        manager.register(Box::new(NamedTool { name: "git_commit" }));
        manager.register(Box::new(ToolSearchTool::new(Arc::downgrade(&manager))));
        let visibility = Arc::new(echo_core::tools::ToolVisibilityState::new(
            ["git_status".to_string(), "tool_search".to_string()]
                .into_iter()
                .collect(),
            ["tool_search".to_string()].into_iter().collect(),
        ));
        let context = ToolContext {
            tool_visibility: Some(Arc::clone(&visibility)),
            ..ToolContext::default()
        };

        let result = manager
            .execute_tool_with_context(
                "tool_search",
                ToolParameters::from([(
                    "query".to_string(),
                    serde_json::Value::String("git".to_string()),
                )]),
                &context,
            )
            .await?;

        assert!(result.success);
        assert!(visibility.is_visible("git_status"));
        assert!(!visibility.is_visible("git_commit"));
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_tool_passes_default_ctx() {
        // The legacy execute_tool must still work: it routes through the same
        // inner path with a default (all-None) ctx.
        let tm = ToolManager::new();
        let captured = Arc::new(Mutex::new(None));
        tm.register(Box::new(CtxCapturingTool {
            captured: captured.clone(),
        }));

        tm.execute_tool("capture", ToolParameters::new())
            .await
            .unwrap();

        let got = captured.lock().unwrap().clone().expect("ctx not captured");
        assert!(got.working_dir.is_none());
        assert!(got.conversation_id.is_none());
        assert!(got.run_id.is_none());
    }

    /// P0 regression (review P0): two "agents" sharing the SAME ToolManager
    /// must NOT cross-contaminate each other's working_dir. The ToolManager
    /// holds no cwd state — each call supplies its own ctx, so concurrent or
    /// interleaved calls with different ctxs stay isolated.
    ///
    /// This simulates the AgentPool pattern where pooled agents share one
    /// ToolManager via Arc. If ToolManager ever started caching working_dir,
    /// this test would catch it.
    #[tokio::test]
    async fn test_shared_tool_manager_does_not_cross_contaminate_cwd() {
        use std::path::Path;

        // One shared ToolManager (mirrors AgentPool's shared Arc<ToolManager>).
        let tm = ToolManager::new();
        // Single capture slot is overwritten each call — so we run the two
        // "sessions" strictly sequentially and assert after each, which is
        // enough to prove the ctx comes from the caller, not ToolManager state.
        let captured = Arc::new(Mutex::new(None));
        tm.register(Box::new(CtxCapturingTool {
            captured: captured.clone(),
        }));

        // "Session A" binds worktree /wt/a.
        let ctx_a = ToolContext {
            working_dir: Some(PathBuf::from("/wt/a")),
            conversation_id: Some("conv-a".into()),
            run_id: Some("run-a".into()),
            ..Default::default()
        };
        tm.execute_tool_with_context("capture", ToolParameters::new(), &ctx_a)
            .await
            .unwrap();
        let got_a = captured
            .lock()
            .unwrap()
            .clone()
            .expect("A: ctx not captured");
        assert_eq!(got_a.working_dir.as_deref(), Some(Path::new("/wt/a")));
        assert_eq!(got_a.conversation_id.as_deref(), Some("conv-a"));

        // "Session B" binds worktree /wt/b on the SAME ToolManager.
        let ctx_b = ToolContext {
            working_dir: Some(PathBuf::from("/wt/b")),
            conversation_id: Some("conv-b".into()),
            run_id: Some("run-b".into()),
            ..Default::default()
        };
        tm.execute_tool_with_context("capture", ToolParameters::new(), &ctx_b)
            .await
            .unwrap();
        let got_b = captured
            .lock()
            .unwrap()
            .clone()
            .expect("B: ctx not captured");
        assert_eq!(got_b.working_dir.as_deref(), Some(Path::new("/wt/b")));
        assert_eq!(got_b.conversation_id.as_deref(), Some("conv-b"));

        // Session A re-runs and must still get /wt/a, proving B's call did not
        // mutate any shared ToolManager state.
        tm.execute_tool_with_context("capture", ToolParameters::new(), &ctx_a)
            .await
            .unwrap();
        let got_a2 = captured
            .lock()
            .unwrap()
            .clone()
            .expect("A2: ctx not captured");
        assert_eq!(
            got_a2.working_dir.as_deref(),
            Some(Path::new("/wt/a")),
            "session A must still see /wt/a after session B used the same ToolManager"
        );
    }
}
