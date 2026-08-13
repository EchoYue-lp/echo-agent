//! Subagent builder — ergonomic chainable configuration

use std::path::PathBuf;

use super::types::{ExecutionMode, SubagentDefinition, SubagentKind};

/// Builder for creating [`SubagentDefinition`] instances with a fluent API.
///
/// # Example
///
/// ```rust
/// use echo_agent::agent::subagent::SubagentBuilder;
///
/// let def = SubagentBuilder::new("researcher")
///     .description("Researches topics thoroughly")
///     .fork_mode()
///     .model("qwen3")
///     .system_prompt("You are a research specialist...")
///     .tools(vec!["search", "read_file"])
///     .inherit_history(10)
///     .timeout(120)
///     .tag("research")
///     .can_delegate()
///     .build();
///
/// assert_eq!(def.name, "researcher");
/// ```
pub struct SubagentBuilder {
    definition: SubagentDefinition,
}

impl SubagentBuilder {
    /// Start building a subagent with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            definition: SubagentDefinition {
                name: name.into(),
                description: String::new(),
                kind: SubagentKind::BuiltIn,
                execution_mode: ExecutionMode::Sync,
                model: None,
                system_prompt: None,
                tool_filter: None,
                max_iterations: None,
                token_limit: None,
                inherit_history: None,
                inherit_memory: false,
                timeout_secs: 0,
                can_delegate: false,
                tags: Vec::new(),
                lightweight: false,
                isolate_worktree: false,
                isolate_workspace: false,
                team: None,
                is_background: false,
            },
        }
    }

    /// Set a human-readable description (shown to the LLM in tool descriptions).
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.definition.description = desc.into();
        self
    }

    /// Set the agent source kind.
    pub fn kind(mut self, kind: SubagentKind) -> Self {
        self.definition.kind = kind;
        self
    }

    /// Load from a custom `.md` definition file.
    pub fn custom(mut self, path: impl Into<PathBuf>) -> Self {
        self.definition.kind = SubagentKind::Custom { path: path.into() };
        self
    }

    /// Load from a plugin source.
    pub fn plugin(mut self, source: impl Into<String>) -> Self {
        self.definition.kind = SubagentKind::Plugin {
            source: source.into(),
        };
        self
    }

    /// Set execution mode to Sync (default).
    pub fn sync_mode(mut self) -> Self {
        self.definition.execution_mode = ExecutionMode::Sync;
        self
    }

    /// Set execution mode to Fork (inherits context, runs independently).
    pub fn fork_mode(mut self) -> Self {
        self.definition.execution_mode = ExecutionMode::Fork;
        // Sprint 6b: lowered 10 → 2. Fork subagents are focused subagents; a
        // 10-message prefix bloats their context with stale turns and dilutes
        // attention on the actual task. 2 trailing messages keep just enough
        // parent state (typically the last user turn + a tool result) without
        // the noise. Per-subagent `.inherit_history(n)` still overrides this,
        // and a `.md` frontmatter can set it per subagent.
        self.definition.inherit_history = Some(2);
        self.definition.inherit_memory = true;
        self
    }

    /// Set execution mode to Teammate (parallel, handle-based).
    pub fn teammate_mode(mut self) -> Self {
        self.definition.execution_mode = ExecutionMode::Teammate;
        self
    }

    /// Override the model for this subagent.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.definition.model = Some(model.into());
        self
    }

    /// Override the system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.definition.system_prompt = Some(prompt.into());
        self
    }

    /// Restrict available tools by name.
    pub fn tools(mut self, tools: Vec<impl Into<String>>) -> Self {
        self.definition.tool_filter = Some(tools.into_iter().map(Into::into).collect());
        self
    }

    /// Set max iterations.
    pub fn max_iterations(mut self, max: usize) -> Self {
        self.definition.max_iterations = Some(max);
        self
    }

    /// Set token limit.
    pub fn token_limit(mut self, limit: usize) -> Self {
        self.definition.token_limit = Some(limit);
        self
    }

    /// Set number of recent messages to inherit from parent (Fork mode).
    pub fn inherit_history(mut self, count: usize) -> Self {
        self.definition.inherit_history = Some(count);
        self
    }

    /// Enable memory inheritance.
    pub fn inherit_memory(mut self) -> Self {
        self.definition.inherit_memory = true;
        self
    }

    /// Set timeout in seconds (0 = no timeout).
    pub fn timeout(mut self, secs: u64) -> Self {
        self.definition.timeout_secs = secs;
        self
    }

    /// Allow this subagent to delegate to further subagents.
    pub fn can_delegate(mut self) -> Self {
        self.definition.can_delegate = true;
        self
    }

    /// Request that Fork-dispatched execution of this subagent run inside an
    /// isolated git worktree (Sprint 8). Mirrors Claude Code's
    /// `isolation: worktree`. Only effective for writer subagents; requires a
    /// `WorktreeFactory` configured on the executor (else a warning is logged
    /// and the subagent runs without isolation).
    pub fn isolate_worktree(mut self) -> Self {
        self.definition.isolate_worktree = true;
        self
    }

    /// Request that Fork-dispatched execution of this subagent run inside an
    /// isolated data workspace (Sprint 10) — a per-subagent disjoint working
    /// directory for data/research subagents emitting generated artifacts
    /// (CSVs/parquet/charts), without git coupling. Requires a
    /// `DataWorkspaceFactory` configured on the executor (else a warning is
    /// logged and the subagent runs without a workspace). A subagent should declare
    /// at most one of `.isolate_worktree()` / `.isolate_workspace()`.
    pub fn isolate_workspace(mut self) -> Self {
        self.definition.isolate_workspace = true;
        self
    }

    /// Sprint 11: declare this subagent as a team-mode dispatcher with the
    /// given `TeamSpec`. Also sets `execution_mode = Team` so the dispatch
    /// router routes to `dispatch_team`. The manager + subagents named in the
    /// spec must be separately registered subagents (resolved by name at
    /// dispatch time — late binding, D-11-team-2).
    pub fn team(mut self, spec: super::types::TeamSpec) -> Self {
        self.definition.execution_mode = ExecutionMode::Team;
        self.definition.team = Some(spec);
        self
    }

    /// Mark this role as preferring background dispatch (Phase 2).
    pub fn background(mut self) -> Self {
        self.definition.is_background = true;
        self
    }

    /// Add a tag for discovery/filtering.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.definition.tags.push(tag.into());
        self
    }

    /// Add multiple tags.
    pub fn tags(mut self, tags: Vec<impl Into<String>>) -> Self {
        self.definition
            .tags
            .extend(tags.into_iter().map(Into::into));
        self
    }

    /// Build the definition.
    pub fn build(self) -> SubagentDefinition {
        // Auto-fill description if empty
        let mut def = self.definition;
        if def.description.is_empty() {
            def.description = format!("Subagent '{}'", def.name);
        }
        def
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_sync() {
        let def = SubagentBuilder::new("subagent")
            .description("Does work")
            .sync_mode()
            .timeout(60)
            .build();

        assert_eq!(def.name, "subagent");
        assert_eq!(def.execution_mode, ExecutionMode::Sync);
        assert_eq!(def.timeout_secs, 60);
        assert!(def.inherit_history.is_none());
    }

    #[test]
    fn test_builder_fork() {
        let def = SubagentBuilder::new("researcher")
            .description("Researches")
            .fork_mode()
            .model("qwen3")
            .system_prompt("You are a researcher")
            .tools(vec!["search", "read"])
            .inherit_history(10)
            .timeout(120)
            .tag("research")
            .can_delegate()
            .build();

        assert_eq!(def.execution_mode, ExecutionMode::Fork);
        assert_eq!(def.model.as_deref(), Some("qwen3"));
        // fork_mode() default is 2 (Sprint 6b), then .inherit_history(10) overrides.
        assert_eq!(def.inherit_history, Some(10));
        assert!(def.can_delegate);
        assert_eq!(def.tags, vec!["research"]);
        assert_eq!(
            def.tool_filter.as_deref(),
            Some(["search".to_string(), "read".to_string()].as_slice())
        );
    }

    #[test]
    fn test_builder_fork_default_inherit_history_lowered() {
        // Sprint 6b: fork_mode() default inherit_history is 2 (was 10).
        let def = SubagentBuilder::new("w")
            .description("d")
            .fork_mode()
            .build();
        assert_eq!(def.inherit_history, Some(2));
    }

    #[test]
    fn test_builder_teammate() {
        let def = SubagentBuilder::new("tm").teammate_mode().build();

        assert_eq!(def.execution_mode, ExecutionMode::Teammate);
        assert!(def.inherit_history.is_none());
    }

    #[test]
    fn test_builder_auto_description() {
        let def = SubagentBuilder::new("auto").build();
        assert!(def.description.contains("auto"));
    }

    #[test]
    fn test_builder_custom_kind() {
        let def = SubagentBuilder::new("custom")
            .custom("/path/to/agent.md")
            .build();
        assert!(matches!(def.kind, SubagentKind::Custom { .. }));
    }

    #[test]
    fn test_builder_plugin_kind() {
        let def = SubagentBuilder::new("plugin")
            .plugin("remote://registry")
            .build();
        assert!(matches!(def.kind, SubagentKind::Plugin { .. }));
    }
}
