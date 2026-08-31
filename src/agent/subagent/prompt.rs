//! Prompt compilation contracts shared by subagent dispatch paths.

use echo_core::llm::ToolDefinition;
use echo_core::llm::types::{ContentPart, Message, MessageContent, Role};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;

use super::types::{ExecutionMode, SubagentAccessMode};

/// Controls whether a dispatch starts fresh or receives filtered parent turns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ContextTransferPolicy {
    #[default]
    Fresh,
    InheritStructured,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCapability {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCapabilitySnapshot {
    pub tools: Vec<ToolCapability>,
    pub visible_tools: Vec<String>,
    pub disabled_tools: Vec<String>,
}

impl ToolCapabilitySnapshot {
    pub fn from_definitions(
        definitions: &[ToolDefinition],
        disabled_tools: &HashSet<String>,
    ) -> Self {
        let mut tools = definitions
            .iter()
            .map(|definition| ToolCapability {
                name: definition.function.name.clone(),
                description: definition
                    .function
                    .description
                    .trim()
                    .chars()
                    .take(240)
                    .collect(),
            })
            .collect::<Vec<_>>();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        tools.dedup_by(|left, right| left.name == right.name);
        let registered = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<HashSet<_>>();
        let visible_tools = tools
            .iter()
            .filter(|tool| !disabled_tools.contains(&tool.name))
            .map(|tool| tool.name.clone())
            .collect();
        let mut disabled_tools = disabled_tools
            .iter()
            .filter(|name| registered.contains(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        disabled_tools.sort();
        Self {
            tools,
            visible_tools,
            disabled_tools,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptActor {
    Subagent,
    Primary,
}

#[derive(Debug, Clone, Copy)]
pub struct SubagentExecutionBoundary<'a> {
    pub access: SubagentAccessMode,
    pub isolation: &'a str,
    pub can_delegate: bool,
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
    pub actor: PromptActor,
    pub name: &'a str,
    pub description: &'a str,
    pub role_prompt: &'a str,
    pub capabilities: &'a ToolCapabilitySnapshot,
    pub boundary: SubagentExecutionBoundary<'a>,
}

/// Registration-time compiler result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompiledSubagentSystemPrompt {
    pub system_prompt: String,
    pub diagnostics: PromptDiagnostics,
}

#[derive(Debug, Clone, Default)]
pub struct SubagentTaskContext {
    pub task_title: Option<String>,
    pub user_goal: Option<String>,
    pub workspace: Option<PathBuf>,
    pub files: Vec<String>,
    pub execution_checks: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub required_artifacts: Vec<String>,
    pub constraints: Vec<String>,
}

/// Dispatch-time facts presented to a prompt compiler.
pub struct SubagentInvocation<'a> {
    pub agent_name: &'a str,
    pub task: &'a str,
    pub mode: ExecutionMode,
    pub transfer_policy: ContextTransferPolicy,
    pub history: &'a [Message],
    pub history_limit: Option<usize>,
    /// Current typed input, including any binary attachments. The compiler owns
    /// its final text framing and must preserve non-text content.
    pub current_message: Option<&'a Message>,
    pub context: &'a SubagentTaskContext,
    /// Present only when this invocation narrows the stable registered tool
    /// surface. Product compilers render the override, not a duplicate catalog.
    pub capability_override: Option<&'a ToolCapabilitySnapshot>,
    /// Opaque product-layer payload. Framework compilers ignore it.
    pub payload: Option<&'a Value>,
}

/// Dispatch-time compiler result consumed by the executor.
#[derive(Debug, Clone, Default)]
pub struct CompiledSubagentInvocation {
    pub messages: Vec<Message>,
    pub diagnostics: PromptDiagnostics,
}

impl CompiledSubagentInvocation {
    pub fn task_input(&self) -> String {
        self.messages
            .last()
            .and_then(Message::text_content)
            .unwrap_or_default()
    }

    pub fn history(&self) -> &[Message] {
        self.messages
            .get(..self.messages.len().saturating_sub(1))
            .unwrap_or_default()
    }
}

/// One prompt compiler instance owns registration-time and dispatch-time framing.
pub trait SubagentPromptCompiler: Send + Sync {
    fn compile_system(&self, input: &SubagentSystemPromptInput<'_>)
    -> CompiledSubagentSystemPrompt;

    fn compile_invocation(&self, input: &SubagentInvocation<'_>) -> CompiledSubagentInvocation;
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

    fn compile_invocation(&self, input: &SubagentInvocation<'_>) -> CompiledSubagentInvocation {
        let mut diagnostics = PromptDiagnostics::default();
        diagnostics.record("task", "dispatch_request");
        let mut history = if input.transfer_policy == ContextTransferPolicy::InheritStructured {
            filter_history(input.history, input.history_limit)
        } else {
            Vec::new()
        };
        remove_duplicate_current_message(&mut history, input.current_message);
        let task_input = if input.context.constraints.is_empty() {
            input.task.to_string()
        } else {
            diagnostics.record("constraints", "dispatch_request.constraints");
            format!(
                "{}\n\n[constraints]\n{}\n[/constraints]",
                input.task.trim(),
                input.context.constraints.join("\n")
            )
        };
        let task_input = match input.context.workspace.as_deref() {
            Some(path) => format!(
                "{task_input}\n\n[workspace]\n- root: {}\n[/workspace]",
                path.display()
            ),
            None => task_input,
        };
        let mut messages = history;
        messages.push(compiled_current_message(input.current_message, &task_input));
        CompiledSubagentInvocation {
            messages,
            diagnostics,
        }
    }
}

/// Build the final current user message while retaining binary attachments.
pub fn compiled_current_message(message: Option<&Message>, task_input: &str) -> Message {
    message
        .cloned()
        .map(|message| with_compiled_task(message, task_input))
        .unwrap_or_else(|| Message::user(task_input.to_string()))
}

/// Avoid replaying the exact current user text immediately before the compiled
/// invocation. The current message remains authoritative because it may also
/// carry attachments that the history projection intentionally does not own.
pub fn remove_duplicate_current_message(history: &mut Vec<Message>, current: Option<&Message>) {
    let Some(current_text) = current.and_then(Message::text_content) else {
        return;
    };
    if history.last().is_some_and(|message| {
        message.role == Role::User
            && message
                .text_content()
                .is_some_and(|text| text.trim() == current_text.trim())
    }) {
        history.pop();
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

    #[test]
    fn default_compiler_renders_dispatch_constraints() {
        let context = SubagentTaskContext {
            constraints: vec![
                "Only edit src/prompt.rs".to_string(),
                "Run cargo test".to_string(),
            ],
            ..SubagentTaskContext::default()
        };
        let compiled = DefaultSubagentPromptCompiler.compile_invocation(&SubagentInvocation {
            agent_name: "implementer",
            task: "Update prompt compilation",
            mode: ExecutionMode::Sync,
            transfer_policy: ContextTransferPolicy::Fresh,
            history: &[],
            history_limit: None,
            current_message: None,
            context: &context,
            capability_override: None,
            payload: None,
        });

        assert_eq!(compiled.diagnostics.count("constraints"), 1);
        assert!(compiled.task_input().contains("Only edit src/prompt.rs"));
        assert!(compiled.task_input().contains("Run cargo test"));
    }

    #[test]
    fn default_compiler_owns_isolated_working_directory_framing() {
        let context = SubagentTaskContext {
            workspace: Some(std::path::PathBuf::from("/tmp/eko-work-42")),
            ..SubagentTaskContext::default()
        };
        let compiled = DefaultSubagentPromptCompiler.compile_invocation(&SubagentInvocation {
            agent_name: "implementer",
            task: "Update prompt compilation",
            mode: ExecutionMode::Fork,
            transfer_policy: ContextTransferPolicy::Fresh,
            history: &[],
            history_limit: None,
            current_message: None,
            context: &context,
            capability_override: None,
            payload: None,
        });

        assert!(
            compiled
                .task_input()
                .contains("[workspace]\n- root: /tmp/eko-work-42\n[/workspace]")
        );
    }
}
