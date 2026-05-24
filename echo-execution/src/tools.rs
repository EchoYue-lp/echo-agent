//! Tool system core — `ToolManager` and tool trait re-exports.
//!
//! The [`ToolManager`] handles registration, execution, concurrency control,
//! and timeout/retry for all tools in an agent session.
//! Uses `DashMap` internally so it can be shared via `Arc`.

use dashmap::DashMap;
use echo_core::error::{Result, ToolError};
use echo_core::llm::types::ToolDefinition;
use std::sync::{Arc, RwLock};
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
    semaphore: Option<Arc<Semaphore>>,
    cached_definitions: RwLock<Option<Vec<ToolDefinition>>>,
}

impl ToolManager {
    pub fn get_openai_tools(&self) -> Vec<ToolDefinition> {
        if let Some(ref cached) = *self.cached_definitions.read().unwrap() {
            return cached.clone();
        }
        let definitions: Vec<ToolDefinition> = self
            .tools
            .iter()
            .map(|entry| ToolDefinition::from_tool(&**entry.value()))
            .collect();
        *self.cached_definitions.write().unwrap() = Some(definitions.clone());
        definitions
    }

    fn invalidate_cache(&self) {
        *self.cached_definitions.write().unwrap() = None;
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
            config: ToolExecutionConfig::default(),
            cached_definitions: RwLock::new(None),
        }
    }

    pub fn new_with_config(config: ToolExecutionConfig) -> Self {
        let semaphore = config
            .max_concurrency
            .map(|n| Arc::new(Semaphore::new(n.max(1))));
        Self {
            tools: DashMap::new(),
            semaphore,
            config,
            cached_definitions: RwLock::new(None),
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

        // 并发控制：获取信号量许可
        let _permit = if let Some(sem) = &self.semaphore {
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

    /// Validate tool parameters synchronously.
    ///
    /// **Warning:** This uses `block_on()` internally and will **panic** if called
    /// from within a Tokio runtime. Use [`Self::validate_tool_parameters_async`]
    /// in async contexts.
    #[deprecated(
        since = "0.2.0",
        note = "Use `validate_tool_parameters_async` instead; this method panics inside Tokio"
    )]
    pub fn validate_tool_parameters(
        &self,
        tool_name: &str,
        parameters: &ToolParameters,
    ) -> Result<()> {
        let tool = self
            .get_tool(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;
        futures::executor::block_on(tool.validate_parameters(parameters))
    }
}
