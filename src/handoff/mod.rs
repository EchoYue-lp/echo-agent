//! Handoff module — control transfer between Agents
//!
//! Supports transferring execution control from the current Agent to another Agent,
//! along with context information.
//!
//! # Core concepts
//!
//! - [`HandoffTarget`]: Describes the target Agent to transfer to
//! - [`HandoffContext`][]: Context data carried along (message history, metadata, etc.)
//! - [`HandoffResult`][]: Result after handoff execution
//! - [`HandoffManager`]: Manages Agent registration and handoff execution
//!
//! # Example
//!
//! ```rust,no_run
//! use echo_agent::handoff::{HandoffManager, HandoffTarget, HandoffContext};
//! use echo_agent::prelude::*;
//! use std::sync::Arc;
//! use tokio::sync::Mutex;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let mut manager = HandoffManager::new();
//!
//! // Register agent
//! let agent = ReactAgentBuilder::simple("qwen3-max", "I am a translation assistant")?;
//! manager.register("translator", agent);
//!
//! // Execute handoff
//! let target = HandoffTarget::new("translator")
//!     .with_message("Please translate the following to English: Hello world");
//! let context = HandoffContext::new()
//!     .with_metadata("source_lang", "zh");
//! let result = manager.handoff(target, context).await?;
//! println!("Translation result: {}", result.output);
//! # Ok(())
//! # }
//! ```

mod tool;

use crate::agent::Agent;
use crate::error::{AgentError, ReactError, Result};
use crate::llm::types::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

pub use tool::HandoffTool;

// ── HandoffTarget ────────────────────────────────────────────────────────────

/// Describes the target Agent for handoff and the task to execute
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffTarget {
    /// Target agent name (must be registered in HandoffManager)
    pub agent_name: String,
    /// Task message to pass to the target agent
    pub message: Option<String>,
    /// Whether to pass the full conversation history to the target agent
    pub transfer_history: bool,
}

impl HandoffTarget {
    /// Create a new HandoffTarget pointing to the specified agent.
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self {
            agent_name: agent_name.into(),
            message: None,
            transfer_history: false,
        }
    }

    /// Set the message to pass to the target agent
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Enable conversation history transfer
    pub fn with_history(mut self) -> Self {
        self.transfer_history = true;
        self
    }
}

// ── HandoffContext ───────────────────────────────────────────────────────────

/// Context information carried during handoff
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HandoffContext {
    /// Source agent name
    pub source_agent: Option<String>,
    /// Conversation history (populated when transfer_history is true)
    pub messages: Vec<Message>,
    /// Custom metadata (key-value pairs)
    pub metadata: HashMap<String, String>,
}

impl HandoffContext {
    /// Create a new HandoffContext (default values).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the source agent name
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source_agent = Some(source.into());
        self
    }

    /// Set the conversation history
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

// ── HandoffResult ────────────────────────────────────────────────────────────

/// Handoff execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffResult {
    /// Target agent name
    pub target_agent: String,
    /// Source agent name (if any)
    pub source_agent: Option<String>,
    /// Execution output
    pub output: String,
    /// Whether to suggest returning control to the source agent
    pub return_to_source: bool,
}

// ── HandoffManager ───────────────────────────────────────────────────────────

/// Handoff manager: responsible for Agent registration and handoff execution dispatch
pub struct HandoffManager {
    agents: HashMap<String, Arc<Mutex<Box<dyn Agent>>>>,
}

impl HandoffManager {
    /// Create a new HandoffManager instance.
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// Register an Agent (accepts any type implementing the Agent trait)
    pub fn register(&mut self, name: impl Into<String>, agent: impl Agent + 'static) {
        let name = name.into();
        self.agents
            .insert(name, Arc::new(Mutex::new(Box::new(agent))));
    }

    /// Register a boxed Agent
    pub fn register_boxed(&mut self, name: impl Into<String>, agent: Box<dyn Agent>) {
        let name = name.into();
        self.agents.insert(name, Arc::new(Mutex::new(agent)));
    }

    /// Register a pre-wrapped `Arc<Mutex<Box<dyn Agent>>>` agent
    pub fn register_shared(&mut self, name: impl Into<String>, agent: Arc<Mutex<Box<dyn Agent>>>) {
        self.agents.insert(name.into(), agent);
    }

    /// Get the list of registered agent names
    pub fn registered_agents(&self) -> Vec<&str> {
        self.agents.keys().map(|s| s.as_str()).collect()
    }

    /// Check whether an agent is registered
    pub fn has_agent(&self, name: &str) -> bool {
        self.agents.contains_key(name)
    }

    /// Execute a handoff: transfer control to the target agent
    ///
    /// Process:
    /// 1. Look up the target agent
    /// 2. Build a context prompt (including metadata and history)
    /// 3. Invoke the target agent to execute
    /// 4. Return the result
    pub async fn handoff(
        &self,
        target: HandoffTarget,
        context: HandoffContext,
    ) -> Result<HandoffResult> {
        let agent_arc = self.agents.get(&target.agent_name).ok_or_else(|| {
            ReactError::Agent(AgentError::InitializationFailed(format!(
                "Handoff target agent '{}' is not registered. Available agents: {:?}",
                target.agent_name,
                self.registered_agents()
            )))
        })?;

        info!(
            source = ?context.source_agent,
            target = %target.agent_name,
            transfer_history = %target.transfer_history,
            metadata_keys = ?context.metadata.keys().collect::<Vec<_>>(),
            "🤝 Executing handoff"
        );

        // Build the full prompt to send to the target agent (execute outside the lock to avoid holding it too long)
        let full_prompt = {
            let mut prompt_parts = Vec::new();

            // Add source information
            if let Some(source) = &context.source_agent {
                prompt_parts.push(format!("[Handoff source: Agent '{}']", source));
            }

            // Add metadata
            if !context.metadata.is_empty() {
                let meta_lines: Vec<String> = context
                    .metadata
                    .iter()
                    .map(|(k, v)| format!("  - {}: {}", k, v))
                    .collect();
                prompt_parts.push(format!("[Context metadata]\n{}", meta_lines.join("\n")));
            }

            // Add history summary
            if target.transfer_history && !context.messages.is_empty() {
                let history_summary: Vec<String> = context
                    .messages
                    .iter()
                    .filter_map(|msg| {
                        msg.content
                            .as_text_ref()
                            .map(|c| format!("{}: {}", msg.role, c))
                    })
                    .collect();
                prompt_parts.push(format!(
                    "[Conversation history]\n{}",
                    history_summary.join("\n")
                ));
            }

            // Add task message
            if let Some(message) = &target.message {
                prompt_parts.push(format!("[Task]\n{}", message));
            }

            prompt_parts.join("\n\n")
        };

        debug!(
            target = %target.agent_name,
            prompt_len = full_prompt.len(),
            "📨 Sending handoff prompt"
        );

        // Use spawn to avoid holding the lock and blocking other handoff requests during execute
        let agent_arc_clone = agent_arc.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let agent = agent_arc_clone.lock().await;
            let result = agent.execute(&full_prompt).await;
            let _ = tx.send(result);
        });

        let output = rx
            .await
            .map_err(|_| ReactError::Other("Handoff task failed to complete".to_string()))??;

        info!(
            target = %target.agent_name,
            output_len = output.len(),
            "Handoff execution completed"
        );

        Ok(HandoffResult {
            target_agent: target.agent_name,
            source_agent: context.source_agent,
            output,
            return_to_source: false,
        })
    }

    /// Execute a handoff chain: transfer control to multiple Agents in sequence
    ///
    /// Each Agent's output is used as the input for the next Agent.
    /// The full conversation history is passed along the chain.
    pub async fn handoff_chain(
        &self,
        targets: Vec<HandoffTarget>,
        initial_context: HandoffContext,
    ) -> Result<Vec<HandoffResult>> {
        let mut results = Vec::new();
        let mut current_context = initial_context;

        for target in targets {
            // Ensure history is transferred throughout the chain
            let mut target_with_history = target.clone();
            target_with_history.transfer_history = true;

            let result = self
                .handoff(target_with_history, current_context.clone())
                .await?;

            // Update context for the next Agent, preserving all metadata and message history
            // Add this interaction to the message history
            let mut updated_messages = current_context.messages.clone();

            // Build a prompt for this interaction (simulating the prompt built internally by handoff)
            // In practice, we would need to know the exact prompt sent to the Agent
            // Simplified: treat the task message as a user message and the output as an assistant message
            if let Some(task_msg) = &target.message {
                updated_messages.push(Message::user(task_msg.clone()));
            }

            // Add the Agent's output
            updated_messages.push(Message::assistant(result.output.clone()));

            // Update the context: preserve all metadata, add the new message history
            current_context = HandoffContext {
                source_agent: Some(result.target_agent.clone()),
                messages: updated_messages,
                metadata: {
                    let mut metadata = current_context.metadata.clone();
                    metadata.insert("previous_output".to_string(), result.output.clone());
                    metadata
                },
            };

            results.push(result);
        }

        Ok(results)
    }
}

impl Default for HandoffManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handoff_target() {
        let target = HandoffTarget::new("agent_b")
            .with_message("Please process this task")
            .with_history();

        assert_eq!(target.agent_name, "agent_b");
        assert_eq!(target.message.as_deref(), Some("Please process this task"));
        assert!(target.transfer_history);
    }

    #[test]
    fn test_handoff_context() {
        let ctx = HandoffContext::new()
            .with_source("agent_a")
            .with_metadata("key1", "value1")
            .with_metadata("key2", "value2");

        assert_eq!(ctx.source_agent.as_deref(), Some("agent_a"));
        assert_eq!(ctx.metadata.len(), 2);
        assert_eq!(ctx.metadata.get("key1").unwrap(), "value1");
    }

    #[test]
    fn test_handoff_manager_register() {
        let manager = HandoffManager::new();
        assert!(manager.registered_agents().is_empty());
        assert!(!manager.has_agent("test"));
    }

    #[tokio::test]
    async fn test_handoff_agent_not_found() {
        let manager = HandoffManager::new();
        let target = HandoffTarget::new("nonexistent");
        let ctx = HandoffContext::new();

        let result = manager.handoff(target, ctx).await;
        assert!(result.is_err());
    }
}
