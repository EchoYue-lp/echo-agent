//! Lightweight sub-agent — shares parent infrastructure, isolates message history.
//!
//! Unlike "heavy" sub-agents (which create full `ReactAgent` instances), the
//! lightweight variant:
//!
//! - Shares the parent's `LlmClient`, `ToolManager`, and `GuardManager`
//! - Maintains its own isolated `Vec<Message>` context
//! - Returns a **summary string** to the parent (not full message history)
//! - Has near-zero startup cost (no agent construction overhead)

use echo_core::agent::CancellationToken;
use echo_core::error::{AgentError, ReactError, Result};
use echo_core::guard::GuardManager;
use echo_core::llm::types::{Message, MessageContent, Role};
use echo_core::llm::{ChatRequest, LlmClient, ToolDefinition};
use echo_core::tools::{ToolParameters, ToolResult};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::tools::ToolManager;

/// Configuration for a lightweight sub-agent.
#[derive(Debug, Clone)]
pub struct LightweightConfig {
    /// Unique name for dispatch.
    pub name: String,
    /// Human-readable description (exposed in the parent's tool description).
    pub description: String,
    /// Override the model name (None = inherit from parent).
    pub model_override: Option<String>,
    /// Override the system prompt (None = inherit from parent).
    pub system_prompt_override: Option<String>,
    /// Filter available tools by name (None = inherit all from parent).
    pub tool_filter: Option<Vec<String>>,
    /// Max iterations before forced return (None = use parent's default).
    pub max_iterations: Option<usize>,
    /// Timeout in seconds (0 = no timeout).
    pub timeout_secs: u64,
}

impl LightweightConfig {
    /// Create a minimal config.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            model_override: None,
            system_prompt_override: None,
            tool_filter: None,
            max_iterations: None,
            timeout_secs: 0,
        }
    }

    /// Set a model override.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model_override = Some(model.into());
        self
    }

    /// Set a system prompt override.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt_override = Some(prompt.into());
        self
    }

    /// Restrict available tools.
    pub fn with_tool_filter(mut self, tools: Vec<String>) -> Self {
        self.tool_filter = Some(tools);
        self
    }

    /// Set max iterations.
    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = Some(max);
        self
    }

    /// Set timeout.
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

/// A lightweight sub-agent that shares parent infrastructure
/// but maintains its own isolated message context.
pub struct LightweightSubagent {
    /// Sub-agent configuration.
    pub config: LightweightConfig,
    /// Shared LLM client (borrowed from parent).
    llm_client: Arc<dyn LlmClient>,
    /// Shared tool manager (borrowed from parent).
    tool_manager: Arc<ToolManager>,
    /// Shared guard manager (borrowed from parent, optional).
    guard_manager: Option<Arc<GuardManager>>,
    /// Parent's model name (when not overridden).
    parent_model: String,
    /// Parent's system prompt (when not overridden).
    parent_system_prompt: String,
    /// Parent's max iterations (when not overridden).
    parent_max_iterations: usize,
    /// Isolated message history.
    messages: RwLock<Vec<Message>>,
}

impl LightweightSubagent {
    /// Create a new lightweight sub-agent sharing parent infrastructure.
    pub fn new(
        config: LightweightConfig,
        llm_client: Arc<dyn LlmClient>,
        tool_manager: Arc<ToolManager>,
        guard_manager: Option<Arc<GuardManager>>,
        parent_model: String,
        parent_system_prompt: String,
        parent_max_iterations: usize,
    ) -> Self {
        let mut sub = Self {
            config,
            llm_client,
            tool_manager,
            guard_manager,
            parent_model,
            parent_system_prompt,
            parent_max_iterations,
            messages: RwLock::new(Vec::new()),
        };
        sub.init_messages();
        sub
    }

    /// Initialize the sub-agent's message context with its system prompt.
    fn init_messages(&mut self) {
        // We can't call async in new(), so init_messages is called
        // immediately after construction via the factory.
    }

    /// Get the model name for this sub-agent.
    pub fn model_name(&self) -> &str {
        self.config
            .model_override
            .as_deref()
            .unwrap_or(&self.parent_model)
    }

    /// Get the system prompt for this sub-agent.
    pub fn system_prompt(&self) -> &str {
        self.config
            .system_prompt_override
            .as_deref()
            .unwrap_or(&self.parent_system_prompt)
    }

    /// Get max iterations for this sub-agent.
    pub fn max_iterations(&self) -> usize {
        self.config
            .max_iterations
            .unwrap_or(self.parent_max_iterations)
    }

    /// Execute a task within this sub-agent's isolated context.
    ///
    /// Returns a summary string suitable for injection into the parent's
    /// conversation without consuming the parent's context window.
    pub async fn execute(&self, task: &str) -> Result<String> {
        self.execute_with_cancel(task, CancellationToken::new())
            .await
    }

    /// Execute with cooperative cancellation support.
    pub async fn execute_with_cancel(
        &self,
        task: &str,
        cancel: CancellationToken,
    ) -> Result<String> {
        // Start with fresh messages: system prompt + user task
        let system_msg = Message {
            role: Role::System,
            content: MessageContent::Text(self.system_prompt().to_string()),
            tool_calls: None,
            name: None,
            tool_call_id: None,
            reasoning_content: None,
        };
        let user_msg = Message {
            role: Role::User,
            content: MessageContent::Text(task.to_string()),
            tool_calls: None,
            name: None,
            tool_call_id: None,
            reasoning_content: None,
        };

        {
            let mut msgs = self.messages.write().await;
            msgs.clear();
            msgs.push(system_msg);
            msgs.push(user_msg);
        }

        let max_iter = self.max_iterations();
        let tool_defs = self.get_tool_definitions();

        for _iteration in 0..max_iter {
            // Check cancellation
            if cancel.is_cancelled() {
                return Err(ReactError::Agent(Box::new(
                    AgentError::InitializationFailed("Sub-agent cancelled".to_string()),
                )));
            }

            // Build request
            let messages = self.messages.read().await.clone();
            let has_tools = !tool_defs.is_empty();
            let request = ChatRequest {
                messages,
                tools: if has_tools {
                    Some(tool_defs.clone())
                } else {
                    None
                },
                ..Default::default()
            };

            // Call LLM
            let response = self.llm_client.chat(request).await.map_err(|e| {
                ReactError::Other(format!("Sub-agent '{}' LLM error: {}", self.config.name, e))
            })?;

            let assistant_msg = response.message.clone();

            // Run guards on the response
            if let Some(ref guard_mgr) = self.guard_manager {
                let text = assistant_msg.text_content().unwrap_or_default();
                let guard_result = guard_mgr
                    .check_all(&text, echo_core::guard::GuardDirection::Output)
                    .await?;
                if guard_result.is_blocked() {
                    return Err(ReactError::Other(format!(
                        "Sub-agent '{}' output blocked by guard",
                        self.config.name
                    )));
                }
            }

            // Check for tool calls (clone to avoid borrow conflict with push)
            let has_tool_calls = assistant_msg
                .tool_calls
                .as_ref()
                .map(|tcs| !tcs.is_empty())
                .unwrap_or(false);

            if has_tool_calls {
                let tool_calls = assistant_msg.tool_calls.clone().unwrap();

                // Push assistant message
                self.messages.write().await.push(assistant_msg);

                // Execute tool calls
                for tc in &tool_calls {
                    // Check cancel
                    if cancel.is_cancelled() {
                        return Err(ReactError::Agent(Box::new(
                            AgentError::InitializationFailed("Sub-agent cancelled".to_string()),
                        )));
                    }

                    let params: ToolParameters =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_default();

                    let tool_result = self
                        .tool_manager
                        .execute_tool(&tc.function.name, params)
                        .await
                        .unwrap_or_else(|e| ToolResult::error(format!("Tool error: {}", e)));

                    // Push tool result as a message
                    let tool_msg = Message {
                        role: Role::Tool,
                        content: MessageContent::Text(tool_result.output),
                        tool_call_id: Some(tc.id.clone()),
                        tool_calls: None,
                        name: None,
                        reasoning_content: None,
                    };
                    self.messages.write().await.push(tool_msg);
                }

                continue; // Next iteration
            }

            // No tool calls — final answer
            return Ok(assistant_msg.text_content().unwrap_or_default().to_string());
        }

        Err(ReactError::Agent(Box::new(
            AgentError::MaxIterationsExceeded(max_iter),
        )))
    }

    /// Get the tool definitions for this sub-agent (respecting tool_filter).
    fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        let tool_names = self.tool_manager.list_tools();
        let names_iter: Box<dyn Iterator<Item = String>> =
            if let Some(ref filter) = self.config.tool_filter {
                Box::new(
                    tool_names
                        .into_iter()
                        .filter(move |name| filter.contains(name)),
                )
            } else {
                Box::new(tool_names.into_iter())
            };
        names_iter
            .filter_map(|name| {
                self.tool_manager
                    .get_tool(&name)
                    .map(|t| ToolDefinition::from_tool(&**t))
            })
            .collect()
    }

    /// Reset the sub-agent's message history for reuse.
    pub async fn reset(&self) {
        self.messages.write().await.clear();
    }

    /// Get the current message count.
    pub async fn message_count(&self) -> usize {
        self.messages.read().await.len()
    }

    /// Get execution duration tracking.
    pub fn timeout(&self) -> Option<Duration> {
        if self.config.timeout_secs > 0 {
            Some(Duration::from_secs(self.config.timeout_secs))
        } else {
            None
        }
    }
}
