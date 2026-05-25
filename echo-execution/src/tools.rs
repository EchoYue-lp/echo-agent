//! Tool system core — `ToolManager` and tool trait re-exports.
//!
//! The [`ToolManager`] handles registration, execution, concurrency control,
//! and timeout/retry for all tools in an agent session.
//! Uses `DashMap` internally so it can be shared via `Arc`.

use dashmap::DashMap;
use echo_core::error::{Result, ToolError};
use echo_core::llm::types::ToolDefinition;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

pub use echo_core::tools::{
    Tool, ToolExecutionConfig, ToolParameters, ToolRegistrar, ToolResult, ToolRiskLevel,
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
        let definitions: Vec<ToolDefinition> = self
            .tools
            .iter()
            .map(|entry| ToolDefinition::from_tool(&**entry.value()))
            .collect();
        *self.cached_definitions.write() = Some((current_version, definitions.clone()));
        definitions
    }

    fn invalidate_cache(&self) {
        self.definitions_version.fetch_add(1, Ordering::Release);
        // No need to clear cached_definitions — version mismatch will trigger
        // a lazy rebuild on the next access.
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
        self.tools.iter().map(|e| e.key().clone()).collect()
    }

    /// Get a reference to a tool (via DashMap's Ref).
    pub fn get_tool(
        &self,
        tool_name: &str,
    ) -> Option<dashmap::mapref::one::Ref<'_, String, Box<dyn Tool>>> {
        self.tools.get(tool_name)
    }

    pub fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|entry| ToolDefinition::from_tool(&**entry.value()))
            .collect()
    }

    /// 执行工具
    ///
    /// 支持并发控制、超时和重试。
    pub async fn execute_tool(
        &self,
        tool_name: &str,
        parameters: ToolParameters,
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
                    tool.execute(parameters.clone()),
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => Err(ToolError::Timeout(tool_name.to_string()).into()),
                }
            } else {
                tool.execute(parameters.clone()).await
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

}
