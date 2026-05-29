//! Context assembly — centralized message list construction with budget awareness.
//!
//! The [`ContextAssembler`] collects all context sources and produces an ordered
//! `Vec<Message>` in one pass. When a token budget is configured, lower-priority
//! sources are compressed or truncated to stay within limits.

pub mod selector;
pub use selector::ContextSelector;

use crate::llm::types::Message;

// ── SourcePriority ───────────────────────────────────────────────────

/// Priority weight for context sources. Higher = kept first under budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourcePriority {
    /// Critical: always included (system prompt).
    Critical = 10,
    /// High: project rules, developer instructions.
    High = 8,
    /// Medium: conversation history, tool results.
    Medium = 5,
    /// Low: memory recall, subagent reports.
    Low = 3,
    /// Best-effort: large file contents, old history.
    BestEffort = 1,
}

// ── ContextBudget ────────────────────────────────────────────────────

/// Per-source token budget configuration.
#[derive(Debug, Clone)]
pub struct ContextBudget {
    /// Total token limit for the assembled context.
    pub total_tokens: usize,
    /// Minimum tokens to reserve for the user message.
    pub user_reserve: usize,
    /// Maximum tokens for conversation history.
    pub history_max: usize,
    /// Maximum tokens for tool results.
    pub tool_results_max: usize,
    /// Maximum tokens for memory recall.
    pub memory_max: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            total_tokens: 128_000,
            user_reserve: 500,
            history_max: 50_000,
            tool_results_max: 20_000,
            memory_max: 5_000,
        }
    }
}

impl ContextBudget {
    pub fn new(total_tokens: usize) -> Self {
        Self {
            total_tokens,
            ..Default::default()
        }
    }
}

// ── ContextSources ───────────────────────────────────────────────────

/// All possible context sources that can be assembled into the message list.
#[derive(Clone, Default)]
pub struct ContextSources {
    /// The system prompt (always first).
    pub system_prompt: Option<String>,
    /// Developer / project instructions (as system messages).
    pub developer_instructions: Vec<String>,
    /// Project rules (as system messages).
    pub project_rules: Vec<String>,
    /// Current task state (as system message).
    pub task_state: Option<String>,
    /// Hook-injected messages (system messages from lifecycle hooks).
    pub hook_injected: Vec<String>,
    /// Recalled long-term memories (injected as a user message).
    pub memory_recall: Option<String>,
    /// Conversation history so far.
    pub conversation_history: Vec<Message>,
    /// Tool results from the current turn.
    pub tool_results: Vec<Message>,
    /// Sub-agent reports.
    pub subagent_reports: Vec<Message>,
    /// The user's input message.
    pub user_message: Option<Message>,
}

/// Centralized context assembler.
///
/// Collects all context sources and produces a canonical ordered message list
/// for the LLM call. When attached to a [`ReactAgent`](crate::agent::ReactAgent)
/// via the builder, `run_react_loop` delegates assembly here instead of pushing
/// messages individually.
pub struct ContextAssembler {
    /// Optional token budget. When set, assembly respects per-source limits.
    pub budget: Option<ContextBudget>,
}

impl Default for ContextAssembler {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextAssembler {
    /// Create a new assembler with default settings.
    pub fn new() -> Self {
        Self { budget: None }
    }

    /// Set a token budget for budget-aware assembly.
    pub fn with_budget(mut self, budget: ContextBudget) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Assemble all sources into a single ordered message list.
    ///
    /// # Ordering
    ///
    /// 1. System prompt
    /// 2. Developer instructions (as system messages)
    /// 3. Project rules (as system messages)
    /// 4. Task state (as system message)
    /// 5. Hook-injected messages (as system messages)
    /// 6. Memory recall (as user message)
    /// 7. Conversation history
    /// 8. Sub-agent reports
    /// 9. Tool results
    /// 10. User message
    pub fn assemble(&self, sources: ContextSources) -> Vec<Message> {
        let mut messages = Vec::new();

        // 1. System prompt
        if let Some(sys) = &sources.system_prompt {
            messages.push(Message::system(sys.clone()));
        }

        // 2. Developer instructions
        for inst in &sources.developer_instructions {
            messages.push(Message::system(format!("[Developer Instructions]\n{inst}")));
        }

        // 3. Project rules
        for rule in &sources.project_rules {
            messages.push(Message::system(format!("[Project Rule]\n{rule}")));
        }

        // 4. Task state
        if let Some(state) = &sources.task_state {
            messages.push(Message::system(format!("[Task State]\n{state}")));
        }

        // 5. Hook-injected messages
        for hook_msg in &sources.hook_injected {
            messages.push(Message::system(hook_msg.clone()));
        }

        // 6. Memory recall (budget-aware truncation)
        if let Some(mem) = &sources.memory_recall {
            let mem_text = if let Some(ref budget) = self.budget {
                if mem.len() > budget.memory_max {
                    let cut = (0..=budget.memory_max)
                        .rev()
                        .find(|i| mem.is_char_boundary(*i))
                        .unwrap_or(budget.memory_max);
                    format!("{}...", &mem[..cut])
                } else {
                    mem.clone()
                }
            } else {
                mem.clone()
            };
            messages.push(Message::user(mem_text));
        }

        // 7. Conversation history (budget-aware: estimate tokens ≈ chars/4)
        let history = if let Some(ref budget) = self.budget {
            let mut token_est = 0usize;
            let keep_count = sources
                .conversation_history
                .iter()
                .rev()
                .take_while(|m| {
                    let t = m.content.as_text().map(|c| c.len() / 4).unwrap_or(0);
                    token_est += t;
                    token_est <= budget.history_max
                })
                .count();
            let start = sources
                .conversation_history
                .len()
                .saturating_sub(keep_count);
            sources.conversation_history[start..].to_vec()
        } else {
            sources.conversation_history.clone()
        };
        messages.extend(history);

        // 8. Sub-agent reports
        messages.extend(sources.subagent_reports);

        // 9. Tool results (budget-aware: keep most recent by estimated tokens)
        let tool_results = if let Some(ref budget) = self.budget {
            let mut token_est = 0usize;
            let keep_count = sources
                .tool_results
                .iter()
                .rev()
                .take_while(|m| {
                    let t = m.content.as_text().map(|c| c.len() / 4).unwrap_or(0);
                    token_est += t;
                    token_est <= budget.tool_results_max
                })
                .count();
            let start = sources.tool_results.len().saturating_sub(keep_count);
            sources.tool_results[start..].to_vec()
        } else {
            sources.tool_results.clone()
        };
        messages.extend(tool_results);

        // 10. User message
        if let Some(user_msg) = sources.user_message {
            messages.push(user_msg);
        }

        messages
    }
}
