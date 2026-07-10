//! Shared types for streaming execution

use crate::llm::types::Message;

/// Streaming execution mode configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamMode {
    /// Single-round execution mode: reset context, restore from checkpoint
    Execute,
    /// Multi-round conversation mode: preserve context, do not reset
    Chat,
}

impl std::fmt::Display for StreamMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamMode::Execute => f.write_str("execute"),
            StreamMode::Chat => f.write_str("chat"),
        }
    }
}

/// Streaming execution initialization parameters
pub(crate) struct StreamInit {
    /// User input text (for audit logs and memory recall)
    pub text: String,
    /// Optional pre-built Message (multimodal scenarios), auto-constructs text message when None
    pub message: Option<Message>,
    /// Log label (e.g. "" or "(multimodal)")
    pub label: String,
    /// Value-scoped metadata captured before waiting for agent execution.
    pub invocation: Option<echo_core::agent::AgentInvocationContext>,
}
