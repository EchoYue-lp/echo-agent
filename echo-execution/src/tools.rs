//! Tool system core — `ToolManager` and tool trait re-exports.
//!
//! The [`ToolManager`] handles registration, execution, concurrency control,
//! and timeout/retry for all tools in an agent session.
//! Uses `DashMap` internally so it can be shared via `Arc`.

use dashmap::DashMap;
use echo_core::error::{Result, ToolError};
use echo_core::llm::types::ToolDefinition;
use echo_core::sandbox::SandboxExecutor;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::Semaphore;

pub use echo_core::tools::{
    Tool, ToolContext, ToolExecutionConfig, ToolParameters, ToolRegistrar, ToolResult,
    ToolRiskLevel, ToolStreamEvent,
};

impl ToolRegistrar for ToolManager {
    fn register(&mut self, tool: Box<dyn Tool>) {
        (&*self).register(tool);
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
}

impl ToolManager {
    pub fn get_openai_tools(&self) -> Vec<ToolDefinition> {
        let current_version = self.definitions_version.load(Ordering::Acquire);
        if let Some(ref cached) = *self.cached_definitions.read() {
            if cached.0 == current_version {
                return cached.1.clone();
            }
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
        let is_read = crate::risk::ToolRiskClassifier::classify(tool_name)
            == crate::risk::ToolRiskCategory::ReadOnly;

        // Check result cache for read-only tools
        if is_read {
            let params_json = serde_json::to_string(&parameters).unwrap_or_default();
            let cache_key = (tool_name.to_string(), params_json);
            if let Some((result, ts)) = self.result_cache.read().get(&cache_key) {
                if ts.elapsed() < std::time::Duration::from_secs(60) {
                    tracing::debug!("Tool result cache hit: {tool_name}");
                    return Ok(result.clone());
                }
            }
        }

        let _permit = if is_read {
            if let Some(sem) = &self.read_semaphore {
                match sem.acquire().await {
                    Ok(permit) => Some(permit),
                    Err(_) => None,
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

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay_ms = self.config.retry_delay_ms * (1u64 << (attempt as u64 - 1).min(5));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            let result = if self.config.timeout_ms > 0 {
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
                Ok(r) => return Ok(r),
                Err(e) if attempt < max_retries => {
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
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

    /// Stream tool execution, collecting all [`ToolStreamEvent`]s into a Vec.
    ///
    /// This method applies the same concurrency control, timeout, and retry
    /// semantics as [`Self::execute_tool`], but routes through
    /// [`Tool::execute_stream`] when the tool supports streaming.
    ///
    /// For tools that do not support streaming, the default `execute_stream`
    /// implementation wraps [`Tool::execute`] into a single
    /// [`ToolStreamEvent::Complete`], so this method still works correctly
    /// (it simply returns a Vec with one element).
    ///
    /// # Returns
    ///
    /// A Vec of [`ToolStreamEvent`] ending with a [`ToolStreamEvent::Complete`].
    /// If the stream does not produce a `Complete` event (e.g. timeout), the
    /// last event may be a `Progress` or `PartialOutput`, and the caller
    /// should treat this as an incomplete execution.
    pub async fn execute_tool_stream_collect(
        &self,
        tool_name: &str,
        parameters: ToolParameters,
    ) -> Result<Vec<ToolStreamEvent>> {
        let tool = self
            .get_tool(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;

        // Concurrency control: acquire semaphore permit (read/write separation)
        let is_read = crate::risk::ToolRiskClassifier::classify(tool_name)
            == crate::risk::ToolRiskCategory::ReadOnly;

        let _permit = if is_read {
            if let Some(sem) = &self.read_semaphore {
                match sem.acquire().await {
                    Ok(permit) => Some(permit),
                    Err(_) => None,
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

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay_ms = self.config.retry_delay_ms * (1u64 << (attempt as u64 - 1).min(5));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            let stream_result = if self.config.timeout_ms > 0 {
                match tokio::time::timeout(
                    Duration::from_millis(self.config.timeout_ms),
                    tool.execute_stream(parameters.clone()),
                )
                .await
                {
                    Ok(future_result) => future_result,
                    Err(_) => Err(ToolError::Timeout(tool_name.to_string()).into()),
                }
            } else {
                tool.execute_stream(parameters.clone()).await
            };

            match stream_result {
                Ok(stream) => {
                    // Consume the stream, collecting all events
                    use futures::StreamExt;
                    let events: Vec<ToolStreamEvent> = stream.collect().await;
                    return Ok(events);
                }
                Err(e) if attempt < max_retries => {
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }

        Err(last_err.unwrap_or_else(|| ToolError::NotFound(tool_name.to_string()).into()))
    }
}

#[cfg(test)]
mod execute_with_context_tests {
    use super::*;
    use echo_core::tools::{Tool, ToolContext, ToolParameters, ToolResult};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    /// Records the `ToolContext` it was called with, so we can verify
    /// `execute_tool_with_context` actually forwards the caller-supplied ctx.
    struct CtxCapturingTool {
        captured: Arc<Mutex<Option<ToolContext>>>,
    }

    struct NamedTool {
        name: &'static str,
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
