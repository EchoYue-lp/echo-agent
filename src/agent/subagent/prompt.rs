//! Prompt compilation contracts shared by subagent dispatch paths.

use echo_core::llm::types::{ContentPart, Message, MessageContent, Role};
use serde_json::Value;

use super::context::SubagentContext;
use super::types::ExecutionMode;

/// Controls whether a dispatch starts fresh or receives filtered parent turns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ContextTransferPolicy {
    #[default]
    Fresh,
    InheritStructured,
}

/// Stable provenance for one compiler-owned prompt section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSectionDiagnostic {
    pub id: String,
    pub source: String,
}

/// Structured diagnostics for prompt tests and observability.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptDiagnostics {
    pub sections: Vec<PromptSectionDiagnostic>,
}

impl PromptDiagnostics {
    pub fn record(&mut self, id: impl Into<String>, source: impl Into<String>) {
        self.sections.push(PromptSectionDiagnostic {
            id: id.into(),
            source: source.into(),
        });
    }

    pub fn count(&self, id: &str) -> usize {
        self.sections
            .iter()
            .filter(|section| section.id == id)
            .count()
    }
}

/// Registration-time facts used to compile a cache-stable role system prompt.
#[derive(Debug, Clone)]
pub struct SubagentSystemPromptInput<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub role_prompt: &'a str,
    pub readonly: bool,
    pub can_delegate: bool,
    pub isolation: &'a str,
    /// Optional static environment grounding (OS/arch/date — facts that do not
    /// change per dispatch). Product compilers render it as a system-prompt
    /// section; the framework default compiler ignores it. Dynamic per-dispatch
    /// state (cwd, workspace root) must NOT go here — it belongs in the
    /// invocation, where the runtime knows the actual working directory.
    pub environment: Option<String>,
}

/// Registration-time compiler result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompiledSubagentSystemPrompt {
    pub system_prompt: String,
    pub diagnostics: PromptDiagnostics,
}

/// Dispatch-time facts presented to a prompt compiler.
pub struct SubagentPromptInput<'a> {
    pub agent_name: &'a str,
    pub task: &'a str,
    pub mode: ExecutionMode,
    pub transfer_policy: ContextTransferPolicy,
    pub parent_context: Option<&'a SubagentContext>,
    pub inherit_history: Option<usize>,
    /// Opaque product-layer payload. Framework compilers ignore it.
    pub payload: Option<&'a Value>,
    /// Explicit task constraints from the dispatch request (e.g. the
    /// `agent_tool` `constraints` parameter). Carried independently of
    /// `parent_context` so fresh-context dispatches can still express
    /// boundaries. Product compilers render them in the task context.
    pub constraints: &'a [String],
}

/// Dispatch-time compiler result consumed by the executor.
#[derive(Debug, Clone, Default)]
pub struct CompiledSubagentInvocation {
    pub task_input: String,
    pub history: Vec<Message>,
    pub diagnostics: PromptDiagnostics,
}

/// One prompt compiler instance owns registration-time and dispatch-time framing.
pub trait SubagentPromptCompiler: Send + Sync {
    fn compile_system(&self, input: &SubagentSystemPromptInput<'_>)
    -> CompiledSubagentSystemPrompt;

    fn compile_invocation(&self, input: &SubagentPromptInput<'_>) -> CompiledSubagentInvocation;
}

/// Product-neutral fallback used by framework consumers that do not inject a compiler.
#[derive(Debug, Default)]
pub struct DefaultSubagentPromptCompiler;

impl SubagentPromptCompiler for DefaultSubagentPromptCompiler {
    fn compile_system(
        &self,
        input: &SubagentSystemPromptInput<'_>,
    ) -> CompiledSubagentSystemPrompt {
        let mut diagnostics = PromptDiagnostics::default();
        diagnostics.record("role", "subagent_definition");
        CompiledSubagentSystemPrompt {
            system_prompt: input.role_prompt.trim().to_string(),
            diagnostics,
        }
    }

    fn compile_invocation(&self, input: &SubagentPromptInput<'_>) -> CompiledSubagentInvocation {
        let mut diagnostics = PromptDiagnostics::default();
        diagnostics.record("task", "dispatch_request");
        let history = if input.transfer_policy == ContextTransferPolicy::InheritStructured {
            input
                .parent_context
                .map(|context| filter_history(&context.messages, input.inherit_history))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        CompiledSubagentInvocation {
            task_input: input.task.to_string(),
            history,
            diagnostics,
        }
    }
}

/// Keep only provider-safe user turns and complete assistant final turns.
pub fn filter_history(messages: &[Message], limit: Option<usize>) -> Vec<Message> {
    let mut filtered = messages
        .iter()
        .filter(|message| {
            if crate::compression::is_context_projection_message(message)
                || message.tool_call_id.is_some()
                || message
                    .tool_calls
                    .as_ref()
                    .is_some_and(|calls| !calls.is_empty())
                || message
                    .reasoning_content
                    .as_ref()
                    .is_some_and(|reasoning| !reasoning.trim().is_empty())
            {
                return false;
            }
            if !matches!(message.role, Role::User | Role::Assistant) {
                return false;
            }
            message
                .content
                .as_text()
                .map(|content| {
                    let trimmed = content.trim();
                    !trimmed.is_empty() && !trimmed.starts_with("[runtime_context:")
                })
                .unwrap_or(false)
        })
        .cloned()
        .collect::<Vec<_>>();

    if let Some(max) = limit.filter(|max| *max > 0) {
        let start = filtered.len().saturating_sub(max);
        filtered = filtered.into_iter().skip(start).collect();
    }
    filtered
}

/// Replace the text part of a user message while preserving binary attachments.
pub fn with_compiled_task(message: Message, task_input: &str) -> Message {
    let content = match message.content {
        MessageContent::Parts(parts) => {
            let mut updated = Vec::with_capacity(parts.len().saturating_add(1));
            updated.push(ContentPart::Text {
                text: task_input.to_string(),
            });
            updated.extend(
                parts
                    .into_iter()
                    .filter(|part| !matches!(part, ContentPart::Text { .. })),
            );
            MessageContent::Parts(updated)
        }
        _ => MessageContent::Text(task_input.to_string()),
    };
    Message { content, ..message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::llm::types::ToolCall;

    #[test]
    fn filter_history_drops_tool_and_reasoning_turns() {
        let mut tool_assistant = Message::assistant("calling tool".to_string());
        tool_assistant.tool_calls = Some(vec![ToolCall {
            id: "call-1".to_string(),
            call_type: "function".to_string(),
            function: echo_core::llm::types::FunctionCall {
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            },
        }]);
        let mut reasoning = Message::assistant("draft".to_string());
        reasoning.reasoning_content = Some("hidden reasoning".to_string());
        let messages = vec![
            Message::system("system".to_string()),
            Message::user("中文问题".to_string()),
            tool_assistant,
            Message::tool_result(
                "call-1".to_string(),
                "read_file".to_string(),
                "ok".to_string(),
            ),
            reasoning,
            Message::assistant("final answer".to_string()),
        ];

        let filtered = filter_history(&messages, None);

        assert_eq!(filtered.len(), 2);
        assert!(matches!(filtered.first(), Some(message) if message.role == Role::User));
        assert!(matches!(filtered.get(1), Some(message) if message.role == Role::Assistant));
    }

    #[test]
    fn filter_history_applies_limit_after_removing_unsafe_turns() {
        let messages = vec![
            Message::user("first".to_string()),
            Message::tool_result(
                "call-1".to_string(),
                "read_file".to_string(),
                "ok".to_string(),
            ),
            Message::assistant("second".to_string()),
            Message::user("third".to_string()),
        ];

        let filtered = filter_history(&messages, Some(2));

        assert_eq!(filtered.len(), 2);
        assert!(
            filtered
                .first()
                .and_then(Message::text_content)
                .is_some_and(|content| content == "second")
        );
        assert!(
            filtered
                .get(1)
                .and_then(Message::text_content)
                .is_some_and(|content| content == "third")
        );
    }

    #[test]
    fn with_compiled_task_preserves_attachments() {
        let message = Message::user_multimodal(vec![
            ContentPart::Text {
                text: "old".to_string(),
            },
            ContentPart::File {
                name: "notes.txt".to_string(),
                content: "ZGF0YQ==".to_string(),
            },
        ]);

        let compiled = with_compiled_task(message, "new task");
        let parts = compiled.content.parts().unwrap_or_default();
        assert_eq!(parts.len(), 2);
        assert!(matches!(
            parts.first(),
            Some(ContentPart::Text { text }) if text == "new task"
        ));
        assert!(matches!(parts.get(1), Some(ContentPart::File { .. })));
    }
}
