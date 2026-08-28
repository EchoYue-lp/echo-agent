//! Tool system core — `ToolManager` and tool trait re-exports.
//!
//! The [`ToolManager`] handles registration, execution, concurrency control,
//! and timeout/retry for all tools in an agent session.
//! Uses `DashMap` internally so it can be shared via `Arc`.

use dashmap::DashMap;
use echo_core::error::{AgentError, ReactError, Result, ToolError};
use echo_core::llm::types::ToolDefinition;
use echo_core::sandbox::SandboxExecutor;
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use parking_lot::RwLock;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const TOOL_RESULT_CACHE_TTL: Duration = Duration::from_secs(60);
const TOOL_RESULT_CACHE_CAPACITY: usize = 256;

fn cancelled_error(tool_name: &str) -> ReactError {
    ReactError::Agent(Box::new(AgentError::Cancelled(format!(
        "tool '{tool_name}'"
    ))))
}

fn reject_cancelled(ctx: &ToolContext, tool_name: &str) -> Result<()> {
    if ctx
        .cancel
        .as_ref()
        .is_some_and(|cancel| cancel.is_cancelled())
    {
        Err(cancelled_error(tool_name))
    } else {
        Ok(())
    }
}

async fn acquire_permit(
    semaphore: &Arc<Semaphore>,
    ctx: &ToolContext,
    tool_name: &str,
) -> Result<OwnedSemaphorePermit> {
    let acquire = Arc::clone(semaphore).acquire_owned();
    if let Some(cancel) = ctx.cancel.as_ref() {
        tokio::select! {
            _ = cancel.cancelled() => Err(cancelled_error(tool_name)),
            permit = acquire => permit.map_err(|error| ToolError::ExecutionFailed {
                tool: tool_name.to_string(),
                message: format!("Concurrency limit error: {error}"),
            }.into()),
        }
    } else {
        acquire.await.map_err(|error| {
            ToolError::ExecutionFailed {
                tool: tool_name.to_string(),
                message: format!("Concurrency limit error: {error}"),
            }
            .into()
        })
    }
}

async fn wait_retry_delay(duration: Duration, ctx: &ToolContext, tool_name: &str) -> Result<()> {
    if let Some(cancel) = ctx.cancel.as_ref() {
        tokio::select! {
            _ = cancel.cancelled() => Err(cancelled_error(tool_name)),
            _ = tokio::time::sleep(duration) => Ok(()),
        }
    } else {
        tokio::time::sleep(duration).await;
        Ok(())
    }
}

async fn cancel_aware<T>(
    future: impl std::future::Future<Output = Result<T>>,
    ctx: &ToolContext,
    tool_name: &str,
    drain_started: bool,
) -> Result<T> {
    if drain_started {
        return future.await;
    }
    if let Some(cancel) = ctx.cancel.as_ref() {
        tokio::select! {
            _ = cancel.cancelled() => Err(cancelled_error(tool_name)),
            result = future => result,
        }
    } else {
        future.await
    }
}

pub use echo_core::tools::{
    ScriptExecutionProfile, ScriptExecutionProfileResolver, Tool, ToolContext, ToolExecutionConfig,
    ToolFailure, ToolFailureCategory, ToolOutputChannel, ToolParameters, ToolRecoveryAction,
    ToolRegistrar, ToolResult, ToolResultContent, ToolRiskLevel, ToolRunner, ToolSideEffect,
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

fn result_cache_key(tool_name: &str, parameters: &ToolParameters) -> (String, String) {
    let params_json = echo_core::utils::canonical_json::canonical_json_bytes(parameters)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    (tool_name.to_string(), params_json)
}

fn validate_schema(tool: &dyn Tool) -> Result<()> {
    jsonschema::validator_for(&tool.parameters()).map_err(|error| {
        ReactError::Config(Box::new(echo_core::error::ConfigError::ConfigFileError(
            format!("tool '{}' has an invalid JSON Schema: {error}", tool.name()),
        )))
    })?;
    Ok(())
}

fn validate_parameters_against_schema(tool: &dyn Tool, parameters: &ToolParameters) -> Result<()> {
    let schema = tool.parameters();
    let validator = jsonschema::validator_for(&schema).map_err(|error| {
        ReactError::Config(Box::new(echo_core::error::ConfigError::ConfigFileError(
            format!("tool '{}' has an invalid JSON Schema: {error}", tool.name()),
        )))
    })?;
    let instance = serde_json::Value::Object(
        parameters
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    if let Err(error) = validator.validate(&instance) {
        return Err(ToolError::InvalidParameter {
            name: tool.name().to_string(),
            message: error.to_string(),
        }
        .into());
    }
    Ok(())
}

impl ToolRegistrar for ToolManager {
    fn register(&mut self, tool: Box<dyn Tool>) {
        ToolManager::register(self, tool);
    }
}

type CachedToolDefinitions = Option<((u64, u64), Vec<ToolDefinition>)>;

/// 工具管理器 — thread-safe tool registry and executor.
pub struct ToolManager {
    tools: DashMap<String, Box<dyn Tool>>,
    registration_lock: parking_lot::Mutex<()>,
    config: ToolExecutionConfig,
    /// Write/execute semaphore (limits concurrent write/execute tools).
    semaphore: Option<Arc<Semaphore>>,
    /// Read semaphore (higher limit for concurrent read-only tools).
    read_semaphore: Option<Arc<Semaphore>>,
    /// Cached tool definitions: `(version, definitions)`.
    /// Invalidated by bumping `definitions_version`; rebuilt lazily on next access.
    /// Uses `parking_lot::RwLock` which does not poison on panic.
    cached_definitions: RwLock<CachedToolDefinitions>,
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
        let dynamic_version = self.tools.iter().fold(0_u64, |revision, entry| {
            revision.wrapping_add(entry.value().schema_revision())
        });
        let cache_key = (current_version, dynamic_version);
        if let Some(ref cached) = *self.cached_definitions.read()
            && cached.0 == cache_key
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
        *self.cached_definitions.write() = Some((cache_key, definitions.clone()));
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
            .artifact
            .as_ref()
            .map(|artifact| artifact.payload_bytes)
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
                .artifact
                .as_ref()
                .map(|artifact| artifact.sha256.as_str())
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
            registration_lock: parking_lot::Mutex::new(()),
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
            registration_lock: parking_lot::Mutex::new(()),
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
        if let Err(error) = self.try_register(tool) {
            tracing::error!(%error, "Tool registration rejected");
        }
    }

    pub fn register_tools(&self, tools: Vec<Box<dyn Tool>>) {
        if let Err(error) = self.try_register_tools(tools) {
            tracing::error!(%error, "Tool batch registration rejected");
        }
    }

    /// Register one tool without replacing an existing canonical name.
    pub fn try_register(&self, tool: Box<dyn Tool>) -> Result<()> {
        validate_schema(tool.as_ref())?;
        let _registration = self.registration_lock.lock();
        let name = tool.name().to_string();
        match self.tools.entry(name.clone()) {
            dashmap::mapref::entry::Entry::Occupied(_) => Err(ReactError::Config(Box::new(
                echo_core::error::ConfigError::ConfigFileError(format!(
                    "tool '{name}' is already registered; use explicit replacement"
                )),
            ))),
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert(tool);
                self.invalidate_cache();
                Ok(())
            }
        }
    }

    /// Atomically register a batch, rejecting internal or existing collisions.
    pub fn try_register_tools(&self, tools: Vec<Box<dyn Tool>>) -> Result<()> {
        let _registration = self.registration_lock.lock();
        let mut names = std::collections::HashSet::with_capacity(tools.len());
        for tool in &tools {
            validate_schema(tool.as_ref())?;
            let name = tool.name();
            if !names.insert(name.to_string()) {
                return Err(ReactError::Config(Box::new(
                    echo_core::error::ConfigError::ConfigFileError(format!(
                        "tool batch contains duplicate name '{name}'"
                    )),
                )));
            }
            if self.tools.contains_key(name) {
                return Err(ReactError::Config(Box::new(
                    echo_core::error::ConfigError::ConfigFileError(format!(
                        "tool '{name}' is already registered; use explicit replacement"
                    )),
                )));
            }
        }
        for tool in tools {
            self.tools.insert(tool.name().to_string(), tool);
        }
        self.invalidate_cache();
        Ok(())
    }

    /// Explicitly replace a tool and return the displaced implementation.
    pub fn replace(&self, tool: Box<dyn Tool>) -> Option<Box<dyn Tool>> {
        let _registration = self.registration_lock.lock();
        let old = self.tools.insert(tool.name().to_string(), tool);
        self.invalidate_cache();
        old
    }

    pub fn unregister(&self, tool_name: &str) -> Option<Box<dyn Tool>> {
        let _registration = self.registration_lock.lock();
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
    /// Called by the agent builder at setup time after selecting a sandbox manager.
    pub fn apply_sandbox(&self, sandbox: Arc<dyn SandboxExecutor>) {
        for mut entry in self.tools.iter_mut() {
            entry.value_mut().set_sandbox(sandbox.clone());
        }
    }

    /// Inject a lazy persisted-script runtime resolver into supporting tools.
    pub fn apply_script_execution_profile_resolver(
        &self,
        resolver: Arc<dyn ScriptExecutionProfileResolver>,
    ) {
        for mut entry in self.tools.iter_mut() {
            entry
                .value_mut()
                .set_script_execution_profile_resolver(resolver.clone());
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

    /// Return names of tools whose rich results the configured model cannot consume.
    pub fn incompatible_tool_names(
        &self,
        available: &[echo_core::llm::ModelInputModality],
    ) -> std::collections::HashSet<String> {
        self.tools
            .iter()
            .filter(|entry| {
                !entry
                    .value()
                    .required_input_modalities()
                    .iter()
                    .all(|required| available.contains(required))
            })
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Return tool schemas compatible with an explicit model capability contract.
    pub fn get_tool_definitions_for_modalities(
        &self,
        available: &[echo_core::llm::ModelInputModality],
    ) -> Vec<ToolDefinition> {
        let incompatible = self.incompatible_tool_names(available);
        self.get_tool_definitions()
            .into_iter()
            .filter(|definition| !incompatible.contains(&definition.function.name))
            .collect()
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
        self.execute_tool_inner(tool_name, parameters, &ToolContext::default(), false)
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
        self.execute_tool_inner(tool_name, parameters, ctx, false)
            .await
    }

    /// Execute while preserving a bounded caller-owned drain window after start.
    ///
    /// Cancellation still rejects a call while it waits for a concurrency
    /// permit or retry delay. Once the tool future starts, this method does not
    /// race-drop it. The tool continues to receive `ctx.cancel` for cooperative
    /// shutdown, while the caller must bound the drain and drop this future
    /// when its grace period expires.
    pub async fn execute_tool_with_context_draining_started(
        &self,
        tool_name: &str,
        parameters: ToolParameters,
        ctx: &ToolContext,
    ) -> Result<ToolResult> {
        self.execute_tool_inner(tool_name, parameters, ctx, true)
            .await
    }

    /// Shared body of [`Self::execute_tool`] / [`Self::execute_tool_with_context`]:
    /// 并发控制、超时、重试、结果缓存，最终通过
    /// [`Tool::execute_with_context`] 路由到具体工具。
    async fn execute_tool_inner(
        &self,
        tool_name: &str,
        parameters: ToolParameters,
        ctx: &ToolContext,
        drain_started: bool,
    ) -> Result<ToolResult> {
        let tool = self
            .get_tool(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;
        validate_parameters_against_schema(tool.as_ref(), &parameters)?;
        tool.validate_parameters(&parameters).await?;
        reject_cancelled(ctx, tool_name)?;

        // 并发控制：获取信号量许可（读/写分离）
        let is_read = tool.risk_level() == ToolRiskLevel::ReadOnly;

        // A write can invalidate every cached read, even when the write later
        // fails: tools may have partially applied an external side effect.
        let cache_key = if is_read {
            Some(result_cache_key(tool_name, &parameters))
        } else {
            self.result_cache.write().clear();
            None
        };
        if let Some(cache_key) = cache_key.as_ref()
            && let Some(result) = self.cached_result(cache_key)
        {
            tracing::debug!("Tool result cache hit: {tool_name}");
            return Ok(result);
        }

        let _permit = if is_read {
            if let Some(sem) = &self.read_semaphore {
                Some(acquire_permit(sem, ctx, tool_name).await?)
            } else {
                None
            }
        } else {
            if let Some(sem) = &self.semaphore {
                Some(acquire_permit(sem, ctx, tool_name).await?)
            } else {
                None
            }
        };

        reject_cancelled(ctx, tool_name)?;

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
                wait_retry_delay(Duration::from_millis(delay_ms), ctx, tool_name).await?;
            }

            let execution = async {
                if self.config.timeout_ms > 0 && !tool.manages_own_timeout() {
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
                }
            };
            let result = cancel_aware(execution, ctx, tool_name, drain_started).await;

            match result {
                Ok(result) if result.success => {
                    if let Some(cache_key) = cache_key.as_ref() {
                        self.store_cached_result(cache_key.clone(), result.clone());
                    }
                    return Ok(result);
                }
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
        validate_parameters_against_schema(tool.as_ref(), parameters)?;
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
        self.execute_tool_stream_with_context_inner(tool_name, parameters, ctx, event_tx, false)
            .await
    }

    /// Streaming counterpart of
    /// [`Self::execute_tool_with_context_draining_started`].
    pub async fn execute_tool_stream_with_context_draining_started(
        &self,
        tool_name: &str,
        parameters: ToolParameters,
        ctx: &ToolContext,
        event_tx: Option<tokio::sync::mpsc::Sender<ToolStreamEvent>>,
    ) -> Result<ToolResult> {
        self.execute_tool_stream_with_context_inner(tool_name, parameters, ctx, event_tx, true)
            .await
    }

    async fn execute_tool_stream_with_context_inner(
        &self,
        tool_name: &str,
        parameters: ToolParameters,
        ctx: &ToolContext,
        event_tx: Option<tokio::sync::mpsc::Sender<ToolStreamEvent>>,
        drain_started: bool,
    ) -> Result<ToolResult> {
        let tool = self
            .get_tool(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;
        reject_cancelled(ctx, tool_name)?;

        let is_read = tool.risk_level() == ToolRiskLevel::ReadOnly;

        let cache_key = if is_read {
            Some(result_cache_key(tool_name, &parameters))
        } else {
            self.result_cache.write().clear();
            None
        };
        if let Some(cache_key) = cache_key.as_ref()
            && let Some(result) = self.cached_result(cache_key)
        {
            tracing::debug!("Tool result cache hit: {tool_name}");
            return Ok(result);
        }

        let _permit = if is_read {
            if let Some(sem) = &self.read_semaphore {
                Some(acquire_permit(sem, ctx, tool_name).await?)
            } else {
                None
            }
        } else {
            if let Some(sem) = &self.semaphore {
                Some(acquire_permit(sem, ctx, tool_name).await?)
            } else {
                None
            }
        };

        reject_cancelled(ctx, tool_name)?;

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
                wait_retry_delay(Duration::from_millis(delay_ms), ctx, tool_name).await?;
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

            let execution = async {
                if self.config.timeout_ms > 0 && !tool.manages_own_timeout() {
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
                }
            };
            let result = cancel_aware(execution, ctx, tool_name, drain_started).await;

            match result {
                Ok(result) if result.success => {
                    if let Some(cache_key) = cache_key.as_ref() {
                        self.store_cached_result(cache_key.clone(), result.clone());
                    }
                    return Ok(result);
                }
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

    fn cached_result(&self, key: &(String, String)) -> Option<ToolResult> {
        let mut cache = self.result_cache.write();
        cache.retain(|_, (_, created)| created.elapsed() < TOOL_RESULT_CACHE_TTL);
        cache.get(key).map(|(result, _)| result.clone())
    }

    fn store_cached_result(&self, key: (String, String), result: ToolResult) {
        let mut cache = self.result_cache.write();
        cache.retain(|_, (_, created)| created.elapsed() < TOOL_RESULT_CACHE_TTL);
        if cache.len() >= TOOL_RESULT_CACHE_CAPACITY
            && let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, (_, created))| *created)
                .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
        cache.insert(key, (result, std::time::Instant::now()));
    }
}

fn add_usize(counter: &AtomicU64, value: usize) {
    counter.fetch_add(u64::try_from(value).unwrap_or(u64::MAX), Ordering::Relaxed);
}

#[cfg(test)]
mod execute_with_context_tests {
    use super::*;
    use echo_core::tools::artifact::ToolOutputArtifactRef;
    use echo_core::tools::{
        InvocationResourceGuard, Tool, ToolContext, ToolOutputChannel, ToolParameters, ToolResult,
        ToolStreamEvent,
    };
    use futures::Stream;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::{Arc, Mutex};

    /// Records the `ToolContext` it was called with, so we can verify
    /// `execute_tool_with_context` actually forwards the caller-supplied ctx.
    struct CtxCapturingTool {
        captured: Arc<Mutex<Option<ToolContext>>>,
    }

    struct NamedTool {
        name: &'static str,
    }

    struct ReadCountingTool {
        calls: Arc<AtomicUsize>,
        output: &'static str,
    }

    struct ImageInputTool;

    struct DelayedStreamingTool;

    struct InternallyTimedTool;

    struct SpawnRetainingTool {
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        settled: Arc<tokio::sync::Notify>,
    }

    struct GuardDropCounter(Arc<AtomicUsize>);

    impl Drop for GuardDropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    struct PendingTool {
        name: &'static str,
        started: Arc<tokio::sync::Notify>,
    }

    struct CompletingAfterCancellationTool {
        started: Arc<tokio::sync::Notify>,
        finished: Arc<AtomicBool>,
        calls: Arc<AtomicUsize>,
    }

    struct TimedOutTool {
        active: Arc<AtomicUsize>,
    }

    struct ActiveExecution(Arc<AtomicUsize>);

    impl Drop for ActiveExecution {
        fn drop(&mut self) {
            self.0.fetch_sub(1, AtomicOrdering::SeqCst);
        }
    }

    impl Tool for TimedOutTool {
        fn name(&self) -> &str {
            "timed_out"
        }

        fn description(&self) -> &str {
            "remains pending until the manager deadline"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _params: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async move {
                self.active.fetch_add(1, AtomicOrdering::SeqCst);
                let _active = ActiveExecution(Arc::clone(&self.active));
                std::future::pending::<()>().await;
                Ok(ToolResult::success("unreachable"))
            })
        }
    }

    impl Tool for PendingTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "waits until cancelled"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _params: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async move {
                self.started.notify_one();
                std::future::pending::<()>().await;
                Ok(ToolResult::success("unreachable"))
            })
        }
    }

    impl Tool for CompletingAfterCancellationTool {
        fn name(&self) -> &str {
            "completing_after_cancellation"
        }

        fn description(&self) -> &str {
            "reaches a terminal safe point after cancellation"
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
                self.started.notify_one();
                tokio::time::sleep(Duration::from_millis(25)).await;
                self.finished.store(true, AtomicOrdering::SeqCst);
                Ok(ToolResult::success("completed"))
            })
        }
    }

    struct SchemaTool {
        calls: Arc<AtomicUsize>,
    }

    impl Tool for SchemaTool {
        fn name(&self) -> &str {
            "schema_tool"
        }

        fn description(&self) -> &str {
            "schema validation fixture"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "required": ["count"],
                "additionalProperties": false,
                "properties": {
                    "count": {"type": "integer", "minimum": 1, "maximum": 3}
                }
            })
        }

        fn execute<'a>(
            &'a self,
            _params: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async move {
                self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(ToolResult::success("executed"))
            })
        }
    }

    struct InvalidSchemaTool;

    impl Tool for InvalidSchemaTool {
        fn name(&self) -> &str {
            "invalid_schema"
        }

        fn description(&self) -> &str {
            "invalid schema fixture"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": 7})
        }

        fn execute<'a>(
            &'a self,
            _params: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async { Ok(ToolResult::success("unreachable")) })
        }
    }

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

    impl Tool for ReadCountingTool {
        fn name(&self) -> &str {
            "read_counting"
        }

        fn description(&self) -> &str {
            "read-only cache fixture"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn risk_level(&self) -> ToolRiskLevel {
            ToolRiskLevel::ReadOnly
        }

        fn execute<'a>(
            &'a self,
            _parameters: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async move {
                self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(ToolResult::success(self.output))
            })
        }
    }

    impl Tool for ImageInputTool {
        fn name(&self) -> &str {
            "image_input"
        }

        fn description(&self) -> &str {
            "requires an image-capable model"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn required_input_modalities(&self) -> &'static [echo_core::llm::ModelInputModality] {
            &[echo_core::llm::ModelInputModality::Image]
        }

        fn execute<'a>(
            &'a self,
            _parameters: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async { Ok(ToolResult::success("ok")) })
        }
    }

    impl Tool for SpawnRetainingTool {
        fn name(&self) -> &str {
            "spawn_retaining"
        }

        fn description(&self) -> &str {
            "retains invocation resources across a tool-owned spawn"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _parameters: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async {
                Err(ToolError::ExecutionFailed {
                    tool: "spawn_retaining".to_string(),
                    message: "ToolContext is required".to_string(),
                }
                .into())
            })
        }

        fn execute_with_context<'a>(
            &'a self,
            _parameters: ToolParameters,
            context: &'a ToolContext,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            let resource_guards = context.resource_guards.clone();
            let started = Arc::clone(&self.started);
            let release = Arc::clone(&self.release);
            let settled = Arc::clone(&self.settled);
            Box::pin(async move {
                let task = tokio::spawn(async move {
                    let retained = resource_guards;
                    started.notify_one();
                    release.notified().await;
                    drop(retained);
                    settled.notify_one();
                });
                std::mem::drop(task);
                futures::future::pending::<echo_core::error::Result<ToolResult>>().await
            })
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
        let drops = Arc::new(AtomicUsize::new(0));
        tm.register(Box::new(CtxCapturingTool {
            captured: captured.clone(),
        }));

        let ctx = ToolContext {
            working_dir: Some(PathBuf::from("/wt/x")),
            conversation_id: Some("c".into()),
            run_id: Some("r".into()),
            resource_guards: vec![InvocationResourceGuard::new(GuardDropCounter(Arc::clone(
                &drops,
            )))],
            ..Default::default()
        };
        let result = tm
            .execute_tool_with_context("capture", ToolParameters::new(), &ctx)
            .await;
        assert!(result.is_ok());

        drop(ctx);
        assert_eq!(drops.load(AtomicOrdering::SeqCst), 0);
        let got = captured
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .unwrap_or_default();
        assert_eq!(
            got.working_dir.as_deref(),
            Some(std::path::Path::new("/wt/x"))
        );
        assert_eq!(got.conversation_id.as_deref(), Some("c"));
        assert_eq!(got.run_id.as_deref(), Some("r"));
        assert_eq!(got.resource_guards.len(), 1);
        drop(got);
        assert_eq!(drops.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn tool_owned_spawn_retains_guards_after_caller_abort() -> echo_core::error::Result<()> {
        let manager = Arc::new(ToolManager::new());
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let settled = Arc::new(tokio::sync::Notify::new());
        let drops = Arc::new(AtomicUsize::new(0));
        manager.register(Box::new(SpawnRetainingTool {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
            settled: Arc::clone(&settled),
        }));
        let context = ToolContext {
            resource_guards: vec![InvocationResourceGuard::new(GuardDropCounter(Arc::clone(
                &drops,
            )))],
            ..ToolContext::default()
        };
        let task_manager = Arc::clone(&manager);
        let caller = tokio::spawn(async move {
            task_manager
                .execute_tool_with_context("spawn_retaining", ToolParameters::new(), &context)
                .await
        });

        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .map_err(|_| ToolError::Timeout("spawn_retaining start".to_string()))?;
        caller.abort();
        let _ = caller.await;
        assert_eq!(drops.load(AtomicOrdering::SeqCst), 0);

        release.notify_one();
        tokio::time::timeout(Duration::from_secs(1), settled.notified())
            .await
            .map_err(|_| ToolError::Timeout("spawn_retaining settlement".to_string()))?;
        assert_eq!(drops.load(AtomicOrdering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn blocking_closure_retains_guards_after_context_drop() -> echo_core::error::Result<()> {
        let drops = Arc::new(AtomicUsize::new(0));
        let context = ToolContext {
            resource_guards: vec![InvocationResourceGuard::new(GuardDropCounter(Arc::clone(
                &drops,
            )))],
            ..ToolContext::default()
        };
        let retained = context.resource_guards.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let closure = tokio::task::spawn_blocking(move || {
            let _ = started_tx.send(());
            let _ = release_rx.blocking_recv();
            drop(retained);
        });

        drop(context);
        started_rx.await.map_err(|_| ToolError::ExecutionFailed {
            tool: "blocking_guard_test".to_string(),
            message: "blocking closure did not start".to_string(),
        })?;
        assert_eq!(drops.load(AtomicOrdering::SeqCst), 0);
        release_tx
            .send(())
            .map_err(|_| ToolError::ExecutionFailed {
                tool: "blocking_guard_test".to_string(),
                message: "blocking closure release channel closed".to_string(),
            })?;
        closure.await.map_err(|error| ToolError::ExecutionFailed {
            tool: "blocking_guard_test".to_string(),
            message: format!("blocking closure failed: {error}"),
        })?;
        assert_eq!(drops.load(AtomicOrdering::SeqCst), 1);
        Ok(())
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
    async fn q_flt_v04_tool_timeout_has_one_typed_outcome_and_no_live_future()
    -> echo_core::error::Result<()> {
        let active = Arc::new(AtomicUsize::new(0));
        let manager = ToolManager::new_with_config(ToolExecutionConfig {
            timeout_ms: 5,
            retry_on_fail: false,
            max_retries: 0,
            retry_delay_ms: 0,
            max_concurrency: None,
            max_read_concurrency: None,
        });
        manager.register(Box::new(TimedOutTool {
            active: Arc::clone(&active),
        }));

        let error = manager
            .execute_tool("timed_out", ToolParameters::new())
            .await
            .err()
            .ok_or_else(|| {
                echo_core::error::ReactError::Other(
                    "pending tool unexpectedly completed".to_string(),
                )
            })?;

        assert!(matches!(
            &error,
            echo_core::error::ReactError::Tool(inner)
                if matches!(inner.as_ref(), echo_core::error::ToolError::Timeout(name) if name == "timed_out")
        ));
        assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
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
        result.artifact = Some(ToolOutputArtifactRef {
            path: PathBuf::from("/tmp/content-free-tool-artifact"),
            artifact_bytes: 4096,
            payload_bytes: 4096,
            sha256: "0".repeat(64),
            retention: "test".to_string(),
        });
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
    async fn read_results_are_cached_and_write_results_invalidate_cache()
    -> echo_core::error::Result<()> {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = ToolManager::new();
        manager.register(Box::new(ReadCountingTool {
            calls: Arc::clone(&calls),
            output: "cached",
        }));
        manager.register(Box::new(NamedTool { name: "write" }));

        let params = ToolParameters::new();
        manager
            .execute_tool("read_counting", params.clone())
            .await?;
        manager
            .execute_tool("read_counting", params.clone())
            .await?;
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);

        manager.execute_tool("write", ToolParameters::new()).await?;
        manager.execute_tool("read_counting", params).await?;
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        Ok(())
    }

    #[tokio::test]
    async fn read_result_cache_has_a_bounded_capacity() -> echo_core::error::Result<()> {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = ToolManager::new();
        manager.register(Box::new(ReadCountingTool {
            calls: Arc::clone(&calls),
            output: "cached",
        }));

        for value in 0..=TOOL_RESULT_CACHE_CAPACITY {
            let mut params = ToolParameters::new();
            params.insert("value".to_string(), serde_json::json!(value));
            manager.execute_tool("read_counting", params).await?;
        }
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            TOOL_RESULT_CACHE_CAPACITY + 1
        );

        let mut first = ToolParameters::new();
        first.insert("value".to_string(), serde_json::json!(0));
        manager.execute_tool("read_counting", first).await?;
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            TOOL_RESULT_CACHE_CAPACITY + 2
        );
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
    async fn schema_validation_prevents_execution() -> echo_core::error::Result<()> {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = ToolManager::new();
        manager.try_register(Box::new(SchemaTool {
            calls: Arc::clone(&calls),
        }))?;

        for parameters in [
            ToolParameters::new(),
            ToolParameters::from([("count".to_string(), serde_json::json!(4))]),
            ToolParameters::from([
                ("count".to_string(), serde_json::json!(2)),
                ("unknown".to_string(), serde_json::json!(true)),
            ]),
        ] {
            assert!(
                manager
                    .execute_tool("schema_tool", parameters)
                    .await
                    .is_err()
            );
        }
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);

        let valid = ToolParameters::from([("count".to_string(), serde_json::json!(2))]);
        assert!(manager.execute_tool("schema_tool", valid).await?.success);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn invalid_schema_and_duplicate_registration_are_rejected_atomically()
    -> echo_core::error::Result<()> {
        let manager = ToolManager::new();
        assert!(manager.try_register(Box::new(InvalidSchemaTool)).is_err());
        manager.try_register(Box::new(NamedTool { name: "stable" }))?;
        assert!(
            manager
                .try_register(Box::new(NamedTool { name: "stable" }))
                .is_err()
        );
        assert!(
            manager
                .try_register_tools(vec![
                    Box::new(NamedTool { name: "new" }),
                    Box::new(NamedTool { name: "stable" }),
                ])
                .is_err()
        );
        assert!(!manager.list_tools().iter().any(|name| name == "new"));
        assert_eq!(manager.list_tools(), vec!["stable"]);
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_stops_running_and_queued_tools() -> echo_core::error::Result<()> {
        let manager = Arc::new(ToolManager::new_with_config(ToolExecutionConfig {
            timeout_ms: 0,
            retry_on_fail: false,
            max_retries: 0,
            retry_delay_ms: 0,
            max_concurrency: Some(1),
            max_read_concurrency: None,
        }));
        let first_started = Arc::new(tokio::sync::Notify::new());
        manager.try_register(Box::new(PendingTool {
            name: "pending_first",
            started: Arc::clone(&first_started),
        }))?;
        manager.try_register(Box::new(PendingTool {
            name: "pending_second",
            started: Arc::new(tokio::sync::Notify::new()),
        }))?;

        let first_cancel = Arc::new(tokio_util::sync::CancellationToken::new());
        let first_ctx = ToolContext {
            cancel: Some(Arc::clone(&first_cancel)),
            ..ToolContext::default()
        };
        let first_manager = Arc::clone(&manager);
        let first = tokio::spawn(async move {
            first_manager
                .execute_tool_with_context("pending_first", ToolParameters::new(), &first_ctx)
                .await
        });
        first_started.notified().await;

        let queued_cancel = Arc::new(tokio_util::sync::CancellationToken::new());
        let queued_ctx = ToolContext {
            cancel: Some(Arc::clone(&queued_cancel)),
            ..ToolContext::default()
        };
        let queued_manager = Arc::clone(&manager);
        let queued = tokio::spawn(async move {
            queued_manager
                .execute_tool_with_context("pending_second", ToolParameters::new(), &queued_ctx)
                .await
        });
        queued_cancel.cancel();
        let queued_result = queued.await.map_err(|error| {
            ReactError::Other(format!("queued tool task failed to join: {error}"))
        })?;
        assert!(queued_result.is_err());

        first_cancel.cancel();
        let first_result = first.await.map_err(|error| {
            ReactError::Other(format!("running tool task failed to join: {error}"))
        })?;
        assert!(first_result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn draining_started_tool_reaches_terminal_safe_point() -> echo_core::error::Result<()> {
        let manager = Arc::new(ToolManager::new());
        let started = Arc::new(tokio::sync::Notify::new());
        let finished = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        manager.try_register(Box::new(CompletingAfterCancellationTool {
            started: Arc::clone(&started),
            finished: Arc::clone(&finished),
            calls: Arc::clone(&calls),
        }))?;

        let cancel = Arc::new(tokio_util::sync::CancellationToken::new());
        let ctx = ToolContext {
            cancel: Some(Arc::clone(&cancel)),
            ..ToolContext::default()
        };
        let execution_manager = Arc::clone(&manager);
        let execution = tokio::spawn(async move {
            execution_manager
                .execute_tool_with_context_draining_started(
                    "completing_after_cancellation",
                    ToolParameters::new(),
                    &ctx,
                )
                .await
        });

        started.notified().await;
        cancel.cancel();
        let result = execution.await.map_err(|error| {
            ReactError::Other(format!("draining tool task failed to join: {error}"))
        })??;
        assert!(result.success);
        assert!(finished.load(AtomicOrdering::SeqCst));
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);

        let cancelled_before_start = Arc::new(tokio_util::sync::CancellationToken::new());
        cancelled_before_start.cancel();
        let cancelled_ctx = ToolContext {
            cancel: Some(cancelled_before_start),
            ..ToolContext::default()
        };
        assert!(
            manager
                .execute_tool_with_context_draining_started(
                    "completing_after_cancellation",
                    ToolParameters::new(),
                    &cancelled_ctx,
                )
                .await
                .is_err()
        );
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_stops_retry_delay_and_stream_wait() -> echo_core::error::Result<()> {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = Arc::new(ToolManager::new_with_config(ToolExecutionConfig {
            timeout_ms: 0,
            retry_on_fail: true,
            max_retries: 2,
            retry_delay_ms: 30_000,
            max_concurrency: None,
            max_read_concurrency: None,
        }));
        manager.try_register(Box::new(RetryOnceTool {
            calls: Arc::clone(&calls),
        }))?;
        manager.try_register(Box::new(DelayedStreamingTool))?;

        let retry_cancel = Arc::new(tokio_util::sync::CancellationToken::new());
        let retry_ctx = ToolContext {
            cancel: Some(Arc::clone(&retry_cancel)),
            ..ToolContext::default()
        };
        let retry_manager = Arc::clone(&manager);
        let retry = tokio::spawn(async move {
            retry_manager
                .execute_tool_with_context("retry_once", ToolParameters::new(), &retry_ctx)
                .await
        });
        while calls.load(AtomicOrdering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        retry_cancel.cancel();
        assert!(
            retry
                .await
                .map_err(|error| ReactError::Other(format!("retry task failed: {error}")))?
                .is_err()
        );
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);

        let stream_cancel = Arc::new(tokio_util::sync::CancellationToken::new());
        let stream_ctx = ToolContext {
            cancel: Some(Arc::clone(&stream_cancel)),
            ..ToolContext::default()
        };
        let stream_manager = Arc::clone(&manager);
        let stream = tokio::spawn(async move {
            stream_manager
                .execute_tool_stream_with_context(
                    "delayed_stream",
                    ToolParameters::new(),
                    &stream_ctx,
                    None,
                )
                .await
        });
        tokio::task::yield_now().await;
        stream_cancel.cancel();
        assert!(
            stream
                .await
                .map_err(|error| ReactError::Other(format!("stream task failed: {error}")))?
                .is_err()
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

    #[test]
    fn tool_definitions_respect_model_input_modalities() {
        let manager = ToolManager::new();
        manager.register(Box::new(NamedTool { name: "text_input" }));
        manager.register(Box::new(ImageInputTool));

        let text_only = manager
            .get_tool_definitions_for_modalities(&[echo_core::llm::ModelInputModality::Text]);
        assert_eq!(
            text_only
                .iter()
                .map(|definition| definition.function.name.as_str())
                .collect::<Vec<_>>(),
            vec!["text_input"]
        );

        let multimodal = manager.get_tool_definitions_for_modalities(&[
            echo_core::llm::ModelInputModality::Text,
            echo_core::llm::ModelInputModality::Image,
        ]);
        assert_eq!(multimodal.len(), 2);
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
