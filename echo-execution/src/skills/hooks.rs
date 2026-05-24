//! Hook System -- lifecycle hooks for the Echo Agent framework.
//!
//! Hooks allow skills and users to extend the agent's behavior by running commands,
//! injecting prompts, making HTTP calls, or invoking MCP tools at specific points
//! in the agent lifecycle.
//!
//! ## Hook events
//!
//! | Event | When | Can modify |
//! |-------|------|-----------|
//! | `PreToolUse` | Before tool execution | Input, permission (allow/block) |
//! | `PostToolUse` | After tool execution succeeds | Output, continuation |
//! | `PostToolUseFailure` | After tool execution fails | Error feedback |
//! | `PermissionRequest` | Permission dialog appears | Auto-approve/deny |
//! | `PermissionDenied` | Permission denied | Retry signal |
//! | `SessionStart` | Session begins or resumes | Context injection |
//! | `SessionEnd` | Session terminates | Cleanup |
//! | `Stop` | Agent finishes responding | Continue reason |
//! | `StopFailure` | Agent encounters unrecoverable error | Alert/recovery |
//! | `Notification` | Agent needs user attention | Permission shortcut |
//! | `UserPromptSubmit` | User submits prompt | Context injection, block |
//! | `PreCompact` | Before context compression | Context injection |
//! | `PostCompact` | After context compression | Context injection |
//! | `ConfigChange` | Configuration file changes | Block/reload |
//! | `InstructionsLoaded` | Skills/instructions loaded | Post-load validation |
//! | `PostToolBatch` | After batch of parallel tool calls | Aggregation |
//! | `SubagentStart` | Before subagent dispatch | Context injection |
//! | `SubagentStop` | After subagent completes | Result injection |
//! | `TaskCreated` | Task created/scheduled | Context injection |
//! | `TaskCompleted` | Task completed | Result injection |
//!
//! ## Hook types
//!
//! | Type | Behavior |
//! |------|----------|
//! | `command` | Execute a shell command; stdin receives JSON context |
//! | `prompt` | Inject a prompt message for the LLM |
//! | `permission` | Return a permission decision directly (allow/deny/ask) |
//! | `http` | POST event data to a URL, parse response |
//! | `mcp_tool` | Call an MCP server tool |
//!
//! ## YAML format (SKILL.md frontmatter or echo-agent.yaml)
//!
//! ```yaml
//! hooks:
//!   PreToolUse:
//!     - matcher: "Bash"
//!       hooks:
//!         - type: command
//!           command: "${SKILL_DIR}/validate.sh"
//!           timeout: 5
//!     - matcher: "Write"
//!       hooks:
//!         - type: prompt
//!           prompt: "Check file permissions before writing"
//!   PostToolUse:
//!     - matcher: "Edit|Write"
//!       hooks:
//!         - type: command
//!           command: "jq -r '.tool_input.file_path' | xargs prettier --write"
//!   Stop:
//!     - hooks:
//!         - type: command
//!           command: "osascript -e 'display notification \"Done\"'"
//!   SessionStart:
//!     - matcher: "startup"
//!       hooks:
//!         - type: prompt
//!           prompt: "Remember to use bun, not npm."
//! ```

// ── Re-export core types from echo-core ─────────────────────────────────

pub use echo_core::hooks::{
    CompressHookStats, HookContext, HookEvent, HookEventCategory, HookResult, HookSource,
    UnifiedHookExecutorFn,
};

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use echo_core::tools::permission::{PermissionDecision, PermissionMode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::sandbox::{SandboxCommand, SandboxManager};
use crate::skills::minimal_hook_env_with_context;

// ── (HookEvent, HookContext, HookResult, CompressHookStats, HookSource are now in echo-core) ──

// ── Hook Action ────────────────────────────────────────────────────────

/// Default timeout for hook commands (seconds).
const fn default_hook_timeout() -> u64 {
    10
}

/// Maximum allowed hook timeout (seconds). Prevents runaway hooks.
const MAX_HOOK_TIMEOUT: u64 = 300;

/// Maximum allowed command string length (bytes). Prevents abuse via malformed YAML.
const MAX_COMMAND_LENGTH: usize = 32 * 1024; // 32 KB

/// A single hook action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HookAction {
    /// Execute a shell command. The command receives JSON context on stdin.
    Command {
        command: String,
        #[serde(default)]
        shell: Option<String>,
        #[serde(default = "default_hook_timeout")]
        timeout: u64,
    },
    /// Inject a prompt for the LLM to consider.
    Prompt { prompt: String },
    /// Return a permission decision directly.
    Permission {
        /// Decision: "allow" | "deny" | "ask"
        decision: String,
        /// Reason for deny
        #[serde(default)]
        reason: Option<String>,
        /// Suggestions for ask
        #[serde(default)]
        suggestions: Vec<String>,
    },
    /// POST event data to a URL and parse the response.
    Http {
        url: String,
        #[serde(default)]
        method: Option<String>,
        #[serde(default)]
        headers: Option<HashMap<String, String>>,
        #[serde(default = "default_hook_timeout")]
        timeout: u64,
    },
    /// Call an MCP server tool.
    #[serde(rename = "mcp_tool")]
    McpTool {
        server: String,
        tool: String,
        #[serde(default)]
        arguments: Option<Value>,
        #[serde(default = "default_hook_timeout")]
        timeout: u64,
    },
}

impl HookAction {
    /// Validate the hook action and return an error for unsafe/invalid config.
    ///
    /// Called during hook registration to catch misconfigurations early.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            HookAction::Command {
                command, timeout, ..
            } => {
                if command.is_empty() {
                    return Err("Command hook has empty command string".into());
                }
                if command.len() > MAX_COMMAND_LENGTH {
                    return Err(format!(
                        "Command hook exceeds max length ({} > {} bytes)",
                        command.len(),
                        MAX_COMMAND_LENGTH
                    ));
                }
                if *timeout > MAX_HOOK_TIMEOUT {
                    return Err(format!(
                        "Command hook timeout {}s exceeds maximum {}s",
                        timeout, MAX_HOOK_TIMEOUT
                    ));
                }
            }
            HookAction::Prompt { prompt } => {
                if prompt.is_empty() {
                    return Err("Prompt hook has empty prompt string".into());
                }
            }
            HookAction::Permission { decision, .. } => {
                if !matches!(decision.as_str(), "allow" | "deny" | "ask") {
                    return Err(format!(
                        "Permission hook has invalid decision '{}' (expected: allow, deny, ask)",
                        decision
                    ));
                }
            }
            HookAction::Http { url, timeout, .. } => {
                if url.is_empty() {
                    return Err("Http hook has empty url".into());
                }
                if *timeout > MAX_HOOK_TIMEOUT {
                    return Err(format!(
                        "Http hook timeout {}s exceeds maximum {}s",
                        timeout, MAX_HOOK_TIMEOUT
                    ));
                }
            }
            HookAction::McpTool {
                server,
                tool,
                timeout,
                ..
            } => {
                if server.is_empty() {
                    return Err("McpTool hook has empty server name".into());
                }
                if tool.is_empty() {
                    return Err("McpTool hook has empty tool name".into());
                }
                if *timeout > MAX_HOOK_TIMEOUT {
                    return Err(format!(
                        "McpTool hook timeout {}s exceeds maximum {}s",
                        timeout, MAX_HOOK_TIMEOUT
                    ));
                }
            }
        }
        Ok(())
    }
}

// ── Hook Rule ──────────────────────────────────────────────────────────

/// A hook rule: a matcher pattern + one or more actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRule {
    /// Matcher pattern.
    /// For tool events: matches tool name (exact, glob, or `|`-separated).
    /// For lifecycle events: matches the event hint (e.g. "startup", "permission_prompt").
    /// `"*"` or empty string matches everything.
    #[serde(default)]
    pub matcher: String,

    /// Actions to execute when the matcher matches.
    pub hooks: Vec<HookAction>,
}

// ── Hooks Definition ───────────────────────────────────────────────────

/// Complete hooks definition from a skill's frontmatter or user config.
///
/// Uses a `HashMap<HookEvent, Vec<HookRule>>` so that any event type
/// is automatically supported without modifying this struct.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksDefinition {
    /// Event -> rules mapping. Supports all `HookEvent` variants.
    #[serde(flatten)]
    pub rules: HashMap<HookEvent, Vec<HookRule>>,
}

impl HooksDefinition {
    /// Get rules for a specific event.
    pub fn rules_for(&self, event: HookEvent) -> &[HookRule] {
        self.rules.get(&event).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Check if any hooks are defined.
    pub fn is_empty(&self) -> bool {
        self.rules.values().all(|v| v.is_empty())
    }

    /// Add rules for a specific event.
    pub fn add_rules(&mut self, event: HookEvent, rules: Vec<HookRule>) {
        if rules.is_empty() {
            return;
        }
        // Validate all hook actions before accepting the rules
        for rule in &rules {
            for action in &rule.hooks {
                if let Err(e) = action.validate() {
                    warn!(?event, error = %e, "Invalid hook action skipped");
                }
            }
        }
        self.rules.entry(event).or_default().extend(rules);
    }

    /// Merge another definition into this one.
    pub fn merge(&mut self, other: HooksDefinition) {
        for (event, rules) in other.rules {
            self.add_rules(event, rules);
        }
    }
}

// ── MCP Tool Executor ─────────────────────────────────────────────────

/// Type-erased callback for executing MCP tool calls from hooks.
///
/// The agent layer injects this via [`HookRegistry::set_mcp_executor`]
/// so that [`HookAction::McpTool`] hooks can call into the agent's MCP manager
/// without echo-execution depending on echo-integration.
pub type McpExecutorFn = Arc<
    dyn Fn(String, String, Option<Value>) -> Pin<Box<dyn Future<Output = HookResult> + Send>>
        + Send
        + Sync,
>;

// ── Hook Registry ──────────────────────────────────────────────────────

/// Registry of hooks from all sources (skills and user config).
// We cannot derive Clone/Default because of `McpExecutorFn` (not Clone).
#[derive(Default)]
pub struct HookRegistry {
    /// Source -> hooks definition.
    sources: HashMap<HookSource, RegisteredHook>,
    /// Optional sandbox manager for executing hook commands.
    sandbox: Option<Arc<SandboxManager>>,
    /// Optional HTTP client for Http hook actions.
    http_client: Option<reqwest::Client>,
    /// Optional MCP tool executor for McpTool hook actions.
    mcp_executor: Option<McpExecutorFn>,
}

impl Clone for HookRegistry {
    fn clone(&self) -> Self {
        Self {
            sources: self.sources.clone(),
            sandbox: self.sandbox.clone(),
            http_client: self.http_client.clone(),
            mcp_executor: self.mcp_executor.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct RegisteredHook {
    definition: HooksDefinition,
    source_dir: String,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a sandbox manager for executing hook commands.
    pub fn with_sandbox_manager(mut self, manager: Arc<SandboxManager>) -> Self {
        self.sandbox = Some(manager);
        self
    }

    /// Attach or replace the sandbox manager.
    pub fn set_sandbox_manager(&mut self, manager: Arc<SandboxManager>) {
        self.sandbox = Some(manager);
    }

    /// Register hooks from a skill.
    pub fn register(&mut self, skill_name: &str, skill_dir: &str, definition: HooksDefinition) {
        if definition.is_empty() {
            return;
        }
        info!(
            skill = skill_name,
            rule_count = definition.rules.values().map(|v| v.len()).sum::<usize>(),
            "Registered skill hooks"
        );
        self.sources.insert(
            HookSource::Skill(skill_name.to_string()),
            RegisteredHook {
                definition,
                source_dir: skill_dir.to_string(),
            },
        );
    }

    /// Register hooks from user configuration.
    pub fn register_user_hooks(&mut self, definition: HooksDefinition) {
        if definition.is_empty() {
            return;
        }
        info!(
            rule_count = definition.rules.values().map(|v| v.len()).sum::<usize>(),
            "Registered user hooks from config"
        );
        self.sources.insert(
            HookSource::UserConfig,
            RegisteredHook {
                definition,
                source_dir: String::new(),
            },
        );
    }

    /// Unregister hooks from a specific source.
    pub fn unregister(&mut self, source: &HookSource) -> bool {
        self.sources.remove(source).is_some()
    }

    /// Check if any hooks are registered.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Check if any hooks are registered for a specific event.
    pub fn has_hooks_for(&self, event: HookEvent) -> bool {
        self.sources
            .values()
            .any(|r| !r.definition.rules_for(event).is_empty())
    }

    /// Set the HTTP client for Http hook actions.
    pub fn set_http_client(&mut self, client: reqwest::Client) {
        self.http_client = Some(client);
    }

    /// Set the MCP tool executor for McpTool hook actions.
    ///
    /// The executor receives (server_name, tool_name, arguments) and
    /// should call the corresponding MCP server tool, returning a [`HookResult`].
    pub fn set_mcp_executor(&mut self, executor: McpExecutorFn) {
        self.mcp_executor = Some(executor);
    }

    // -- Public execution methods --

    /// Execute all matching PreToolUse hooks.
    pub async fn run_pre_tool_use(
        &self,
        tool_name: &str,
        tool_input: &Value,
        session_id: &str,
    ) -> HookResult {
        let context = HookContext::for_pre_tool_use(tool_name, tool_input, session_id, "");
        self.run_hooks(&context).await
    }

    /// Execute all matching PostToolUse hooks.
    pub async fn run_post_tool_use(
        &self,
        tool_name: &str,
        tool_input: &Value,
        tool_output: &str,
        session_id: &str,
    ) -> HookResult {
        let context =
            HookContext::for_post_tool_use(tool_name, tool_input, tool_output, session_id, "");
        self.run_hooks(&context).await
    }

    /// Execute all matching PostToolUseFailure hooks.
    pub async fn run_post_tool_use_failure(
        &self,
        tool_name: &str,
        tool_input: &Value,
        tool_error: &str,
        session_id: &str,
    ) -> HookResult {
        let context = HookContext::for_post_tool_use_failure(
            tool_name, tool_input, tool_error, session_id, "",
        );
        self.run_hooks(&context).await
    }

    /// Execute hooks for a lifecycle event.
    pub async fn run_lifecycle_hooks(&self, context: &HookContext) -> HookResult {
        self.run_hooks(context).await
    }

    // -- Core execution engine --

    async fn run_hooks(&self, context: &HookContext) -> HookResult {
        let event = context.event;
        let mut combined = HookResult::default();

        // Sort sources: UserConfig first, then skills alphabetically
        let mut sorted_sources: Vec<&HookSource> = self.sources.keys().collect();
        sorted_sources.sort_by(|a, b| match (a, b) {
            (HookSource::UserConfig, _) => std::cmp::Ordering::Less,
            (_, HookSource::UserConfig) => std::cmp::Ordering::Greater,
            (HookSource::Skill(a), HookSource::Skill(b)) => a.cmp(b),
        });

        for source in sorted_sources {
            let Some(registered) = self.sources.get(source) else {
                continue;
            };
            let rules = registered.definition.rules_for(event);

            for rule in rules {
                if !matches_hook(&rule.matcher, context) {
                    continue;
                }

                debug!(
                    source = %source,
                    event = ?event,
                    matcher = &rule.matcher,
                    "Hook matched"
                );

                for action in &rule.hooks {
                    let result = execute_action(
                        action,
                        &registered.source_dir,
                        context,
                        self.sandbox.as_ref(),
                        self.http_client.as_ref(),
                        self.mcp_executor.as_ref(),
                    )
                    .await;

                    merge_result(&mut combined, result);

                    if combined.stop_propagation || combined.block {
                        return combined;
                    }
                }
            }
        }

        combined
    }
}

// ── Matcher ────────────────────────────────────────────────────────────

/// Unified matcher for all hook events.
fn matches_hook(matcher: &str, context: &HookContext) -> bool {
    // "*" or empty matcher matches everything
    if matcher == "*" || matcher.is_empty() {
        return true;
    }

    // Tool events: match against tool_name
    if context.event.is_tool_event() {
        if let Some(ref tool_name) = context.tool_name {
            if matches_tool_name(matcher, tool_name) {
                return true;
            }
        }
    }

    // Non-tool events (Lifecycle, Subagent, Task): match against context.matcher hint
    if let Some(ref hint) = context.matcher {
        // Exact match
        if matcher == hint.as_str() {
            return true;
        }
        // Pipe-separated alternatives (e.g., "Edit|Write" or "startup|resume")
        for part in matcher.split('|') {
            let part = part.trim();
            if part == hint.as_str() {
                return true;
            }
            // Try each part as a glob pattern
            if let Ok(pattern) = glob::Pattern::new(part) {
                if pattern.matches(hint) {
                    return true;
                }
            }
        }
        // Try full matcher as glob
        if let Ok(pattern) = glob::Pattern::new(matcher) {
            if pattern.matches(hint) {
                return true;
            }
        }
    }

    false
}

/// Match a tool name against a pattern (exact, glob, prefix with parens).
fn matches_tool_name(matcher: &str, tool_name: &str) -> bool {
    if matcher == tool_name {
        return true;
    }
    // Pipe-separated alternatives
    if matcher.contains('|') {
        return matcher
            .split('|')
            .any(|part| matches_tool_name(part.trim(), tool_name));
    }
    // Glob matching
    if let Ok(pattern) = glob::Pattern::new(matcher) {
        if pattern.matches(tool_name) {
            return true;
        }
    }
    // Prefix match for patterns like "Bash" matching "Bash(git:*)"
    if tool_name.starts_with(matcher)
        && tool_name.len() > matcher.len()
        && tool_name.as_bytes()[matcher.len()] == b'('
    {
        return true;
    }
    false
}

// ── Action Execution ───────────────────────────────────────────────────

async fn execute_action(
    action: &HookAction,
    source_dir: &str,
    context: &HookContext,
    sandbox: Option<&Arc<SandboxManager>>,
    http_client: Option<&reqwest::Client>,
    mcp_executor: Option<&McpExecutorFn>,
) -> HookResult {
    match action {
        HookAction::Command {
            command,
            shell,
            timeout,
        } => {
            execute_command_hook(
                command,
                shell.as_deref(),
                *timeout,
                source_dir,
                context,
                sandbox,
            )
            .await
        }
        HookAction::Prompt { prompt } => {
            let mut result = HookResult::default();
            result.messages.push(prompt.clone());
            result
        }
        HookAction::Permission {
            decision,
            reason,
            suggestions,
        } => {
            let mut result = HookResult::default();
            match decision.as_str() {
                "allow" => {
                    result.permission_decision = Some(PermissionDecision::Allow);
                }
                "deny" => {
                    let reason_text = reason.clone().unwrap_or_else(|| "Hook denied".to_string());
                    result.block = true;
                    result.block_reason = Some(reason_text.clone());
                    result.permission_decision = Some(PermissionDecision::Deny {
                        reason: reason_text,
                    });
                }
                "ask" => {
                    result.permission_decision = Some(PermissionDecision::Ask {
                        suggestions: suggestions.clone(),
                    });
                }
                _ => {
                    warn!(decision = %decision, "Unknown permission decision from hook");
                }
            }
            result.stop_propagation = true;
            result
        }
        HookAction::Http {
            url,
            method,
            headers,
            timeout,
        } => {
            execute_http_hook(
                url,
                method.as_deref(),
                headers.as_ref(),
                *timeout,
                context,
                http_client,
            )
            .await
        }
        HookAction::McpTool {
            server,
            tool,
            arguments,
            timeout,
        } => match mcp_executor {
            Some(executor) => {
                let fut = executor(server.clone(), tool.clone(), arguments.clone());
                if *timeout > 0 {
                    match tokio::time::timeout(Duration::from_secs(*timeout), fut).await {
                        Ok(result) => result,
                        Err(_) => {
                            warn!(
                                server = %server,
                                tool = %tool,
                                timeout_secs = *timeout,
                                "McpTool hook timed out"
                            );
                            HookResult::default()
                        }
                    }
                } else {
                    fut.await
                }
            }
            None => {
                warn!(
                    server = %server,
                    tool = %tool,
                    "McpTool hook action configured but no mcp_executor registered"
                );
                HookResult::default()
            }
        },
    }
}

// -- Command hook execution --

async fn execute_command_hook(
    command: &str,
    shell: Option<&str>,
    timeout_secs: u64,
    source_dir: &str,
    context: &HookContext,
    sandbox: Option<&Arc<SandboxManager>>,
) -> HookResult {
    // Variable substitution in command
    let command = command
        .replace("${SKILL_DIR}", source_dir)
        .replace("${CLAUDE_PLUGIN_ROOT}", source_dir);

    // Build JSON context for stdin (include hook_event_name for compatibility)
    let mut stdin_value = serde_json::to_value(context).unwrap_or_default();
    stdin_value["hook_event_name"] = json!(context.event.as_str());
    let stdin_json = stdin_value;

    let timeout = Duration::from_secs(timeout_secs);

    // -- Sandbox execution path --
    if let Some(manager) = sandbox {
        let (program, args) = build_hook_shell_command(&command, shell);
        let mut sandbox_cmd = SandboxCommand::program(&program, args).with_timeout(timeout);

        if !source_dir.is_empty() && Path::new(source_dir).exists() {
            sandbox_cmd = sandbox_cmd.with_working_dir(source_dir);
        }

        // Use minimal environment
        let env = minimal_hook_env_with_context(
            source_dir,
            &context.session_id,
            context.event.as_str(),
            &context.cwd,
        );
        for (k, v) in env {
            sandbox_cmd = sandbox_cmd.with_env(k, v);
        }

        // Pipe stdin JSON
        if let Ok(json_str) = serde_json::to_string(&stdin_json) {
            sandbox_cmd = sandbox_cmd.with_stdin(json_str);
        }

        return match manager.execute(sandbox_cmd).await {
            Ok(result) => {
                if !result.stderr.is_empty() {
                    debug!(
                        command = %command,
                        stderr = %result.stderr.trim(),
                        "Hook stderr (sandboxed)"
                    );
                }
                parse_hook_output(&result.stdout, result.exit_code)
            }
            Err(e) => {
                warn!(command = %command, error = %e, "Hook sandbox error");
                HookResult::default()
            }
        };
    }

    // -- Fallback: direct process execution (no sandbox) --
    let (program, args) = build_hook_shell_command(&command, shell);
    let mut cmd = tokio::process::Command::new(&program);
    for arg in &args {
        cmd.arg(arg);
    }
    cmd.kill_on_drop(true);

    if !source_dir.is_empty() && Path::new(source_dir).exists() {
        cmd.current_dir(source_dir);
    }

    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Use minimal environment
    let env = minimal_hook_env_with_context(
        source_dir,
        &context.session_id,
        context.event.as_str(),
        &context.cwd,
    );
    cmd.env_clear();
    for (k, v) in env {
        cmd.env(k, v);
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            warn!(command = %command, error = %e, "Failed to spawn hook command");
            return HookResult::default();
        }
    };

    let mut child = child;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let json_str = serde_json::to_string(&stdin_json).unwrap_or_default();
        let _ = stdin.write_all(json_str.as_bytes()).await;
        let _ = stdin.write_all(b"\n").await;
        drop(stdin);
    }

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            if !stderr.is_empty() {
                debug!(command = %command, stderr = %stderr.trim(), "Hook stderr");
            }

            parse_hook_output(&stdout, output.status.code().unwrap_or(-1))
        }
        Ok(Err(e)) => {
            warn!(command = %command, error = %e, "Hook command execution error");
            HookResult::default()
        }
        Err(_) => {
            warn!(
                command = %command,
                timeout_secs = timeout_secs,
                "Hook command timed out"
            );
            HookResult::default()
        }
    }
}

// -- HTTP hook execution --

async fn execute_http_hook(
    url: &str,
    method: Option<&str>,
    headers: Option<&HashMap<String, String>>,
    timeout_secs: u64,
    context: &HookContext,
    client: Option<&reqwest::Client>,
) -> HookResult {
    let client = match client {
        Some(c) => c.clone(),
        None => reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .unwrap_or_default(),
    };

    let method = reqwest::Method::from_bytes(method.unwrap_or("POST").as_bytes())
        .unwrap_or(reqwest::Method::POST);

    let mut req = client.request(method, url).json(context);

    if let Some(h) = headers {
        for (k, v) in h {
            req = req.header(k, v);
        }
    }

    // Always respect the hook's timeout, even when an external client is provided.
    let send_fut = req.send();
    let result = if timeout_secs > 0 {
        match tokio::time::timeout(Duration::from_secs(timeout_secs), send_fut).await {
            Ok(res) => res,
            Err(_) => {
                warn!(url = %url, timeout_secs, "Http hook timed out");
                return HookResult::default();
            }
        }
    } else {
        send_fut.await
    };

    match result {
        Ok(resp) if resp.status().is_success() => {
            let text = resp.text().await.unwrap_or_default();
            parse_hook_output(&text, 0)
        }
        Ok(resp) => {
            warn!(status = %resp.status(), url = %url, "Http hook non-2xx response");
            HookResult::default()
        }
        Err(e) => {
            warn!(error = %e, url = %url, "Http hook request failed");
            HookResult::default()
        }
    }
}

// ── Output Parsing ─────────────────────────────────────────────────────

/// Parse JSON output from a hook command.
///
/// Hooks can return JSON to control execution:
/// ```json
/// {
///   "decision": "block",
///   "reason": "Unsafe command detected",
///   "continue": false,
///   "permission_decision": "allow" | "deny" | "ask",
///   "permission_suggestions": ["Allow", "Deny"],
///   "continue_reason": "Check if tests pass",
///   "injected_context": "Remember to use bun",
///   "metadata": {}
/// }
/// ```
fn parse_hook_output(stdout: &str, exit_code: i32) -> HookResult {
    let mut result = HookResult::default();

    // Exit code semantics (aligned with Claude Code convention):
    //  exit 0: pass (no block intent)
    //  exit 1: no block (hook produced output but no explicit block)
    //  exit 2: block (explicit block signal)
    //  other non-zero: warning, no block
    match exit_code {
        0 | 1 => {} // no implicit block
        2 => {
            result.block = true;
            result.block_reason = Some("Hook exited with code 2 (explicit block)".to_string());
        }
        _ => {
            // Other non-zero: log warning, don't block
            warn!(
                exit_code = exit_code,
                "Hook exited with unexpected code, treating as warning (not block)"
            );
        }
    }

    // Try to parse JSON output
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return result;
    }

    if let Ok(json) = serde_json::from_str::<Value>(trimmed) {
        // Parse decision field
        if let Some(decision) = json.get("decision").and_then(|v| v.as_str()) {
            match decision {
                "block" => {
                    result.block = true;
                    if let Some(reason) = json.get("reason").and_then(|v| v.as_str()) {
                        result.block_reason = Some(reason.to_string());
                    }
                }
                "allow" => {
                    result.block = false;
                    result.block_reason = None;
                }
                _ => {}
            }
        }

        // Parse permission_decision field
        if let Some(perm_decision) = json.get("permission_decision").and_then(|v| v.as_str()) {
            match perm_decision {
                "allow" => {
                    result.permission_decision = Some(PermissionDecision::Allow);
                }
                "deny" => {
                    let reason = json
                        .get("permission_reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Hook denied permission")
                        .to_string();
                    result.permission_decision = Some(PermissionDecision::Deny { reason });
                }
                "ask" => {
                    let suggestions = json
                        .get("permission_suggestions")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    result.permission_decision = Some(PermissionDecision::Ask { suggestions });
                }
                _ => {}
            }
        }

        // Parse permission_mode_override field
        if let Some(mode) = json.get("permission_mode").and_then(|v| v.as_str()) {
            result.permission_mode_override = match mode {
                "default" => Some(PermissionMode::Default),
                "plan" => Some(PermissionMode::Plan),
                "auto" => Some(PermissionMode::Auto),
                "acceptEdits" => Some(PermissionMode::AcceptEdits),
                "bypassPermissions" => Some(PermissionMode::BypassPermissions),
                _ => None,
            };
        }

        if json.get("continue") == Some(&Value::Bool(false)) {
            result.stop_propagation = true;
        }

        if let Some(updated) = json.get("updatedInput") {
            result.updated_input = Some(updated.clone());
        }

        // Parse lifecycle-specific fields
        if let Some(reason) = json.get("continue_reason").and_then(|v| v.as_str()) {
            result.continue_reason = Some(reason.to_string());
        }

        if let Some(ctx) = json.get("injected_context").and_then(|v| v.as_str()) {
            result.injected_context = Some(ctx.to_string());
        }

        // Parse retry field (PermissionDenied hooks)
        if json.get("retry") == Some(&Value::Bool(true)) {
            result.retry = true;
        }

        if let Some(meta) = json.get("metadata") {
            if !meta.is_null() {
                result.metadata = Some(meta.clone());
            }
        }
    } else if exit_code == 0 {
        // Non-JSON stdout on exit 0: treat as injected context
        result.injected_context = Some(trimmed.to_string());
    }

    result
}

// ── Shell Command Builder ──────────────────────────────────────────────

fn build_hook_shell_command(command: &str, shell: Option<&str>) -> (String, Vec<String>) {
    let shell_type = shell.unwrap_or("bash");

    if shell_type == "powershell" {
        let program = if which_exists("pwsh") {
            "pwsh"
        } else if cfg!(target_os = "windows") {
            "powershell"
        } else {
            "sh"
        };

        if program == "pwsh" || program == "powershell" {
            (
                program.to_string(),
                vec![
                    "-NoProfile".to_string(),
                    "-NonInteractive".to_string(),
                    "-Command".to_string(),
                    command.to_string(),
                ],
            )
        } else {
            (
                "sh".to_string(),
                vec!["-c".to_string(), command.to_string()],
            )
        }
    } else if cfg!(target_os = "windows") {
        #[cfg(target_os = "windows")]
        {
            if let Some(bash) = crate::skills::external::prompt_exec::find_git_bash_path() {
                (bash, vec!["-c".to_string(), command.to_string()])
            } else {
                (
                    "cmd".to_string(),
                    vec!["/C".to_string(), command.to_string()],
                )
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            (
                "bash".to_string(),
                vec!["-c".to_string(), command.to_string()],
            )
        }
    } else {
        (
            "bash".to_string(),
            vec!["-c".to_string(), command.to_string()],
        )
    }
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

// ── Result Merging ─────────────────────────────────────────────────────

fn merge_result(combined: &mut HookResult, incoming: HookResult) {
    if incoming.block {
        combined.block = true;
        combined.block_reason = incoming.block_reason.or(combined.block_reason.take());
    }
    if incoming.updated_input.is_some() {
        combined.updated_input = incoming.updated_input;
    }
    combined.messages.extend(incoming.messages);
    if incoming.stop_propagation {
        combined.stop_propagation = true;
    }

    // Permission decision with priority: deny > ask > allow
    if let Some(new_decision) = incoming.permission_decision {
        let should_replace = match (&combined.permission_decision, &new_decision) {
            // If we already have deny, keep it
            (Some(PermissionDecision::Deny { .. }), _) => false,
            // New decision is deny -- always take it
            (_, PermissionDecision::Deny { .. }) => true,
            // If we already have ask, keep it (ask > allow)
            (Some(PermissionDecision::Ask { .. }), _) => false,
            // New decision is ask -- take it over allow
            (_, PermissionDecision::Ask { .. }) => true,
            // If we already have RequireApproval, keep it
            (Some(PermissionDecision::RequireApproval), _) => false,
            // New decision is RequireApproval -- take it over allow
            (_, PermissionDecision::RequireApproval) => true,
            // Both are allow -- either is fine
            (Some(PermissionDecision::Allow), PermissionDecision::Allow) => false,
            // No existing decision -- take the new one
            (None, _) => true,
        };
        if should_replace {
            combined.permission_decision = Some(new_decision);
        }
    }

    // permission_mode_override: last non-none wins
    if incoming.permission_mode_override.is_some() {
        combined.permission_mode_override = incoming.permission_mode_override;
    }

    // continue_reason: non-None overrides None
    if incoming.continue_reason.is_some() {
        combined.continue_reason = incoming.continue_reason;
    }

    // injected_context: concatenate with newline
    if let Some(ctx) = incoming.injected_context {
        combined.injected_context = Some(match combined.injected_context.take() {
            Some(existing) => format!("{}\n{}", existing, ctx),
            None => ctx,
        });
    }

    // retry: OR semantics (any true → combined true)
    if incoming.retry {
        combined.retry = true;
    }

    // metadata: deep merge
    if let Some(meta) = incoming.metadata {
        combined.metadata = Some(match combined.metadata.take() {
            Some(existing) => {
                let mut merged = existing;
                if let (Value::Object(a), Value::Object(b)) = (&mut merged, &meta) {
                    for (k, v) in b {
                        a.insert(k.clone(), v.clone());
                    }
                }
                merged
            }
            None => meta,
        });
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // -- HookEvent tests --

    #[test]
    fn test_hook_event_is_tool_event() {
        assert!(HookEvent::PreToolUse.is_tool_event());
        assert!(HookEvent::PostToolUse.is_tool_event());
        assert!(HookEvent::PostToolUseFailure.is_tool_event());
        assert!(HookEvent::PermissionRequest.is_tool_event());
        assert!(HookEvent::PermissionDenied.is_tool_event());
        assert!(!HookEvent::SessionStart.is_tool_event());
        assert!(!HookEvent::Stop.is_tool_event());
        assert!(!HookEvent::Notification.is_tool_event());
        assert!(!HookEvent::StopFailure.is_tool_event());
    }

    #[test]
    fn test_hook_event_supports_matcher() {
        assert!(HookEvent::PreToolUse.supports_matcher());
        assert!(HookEvent::SessionStart.supports_matcher());
        assert!(HookEvent::Notification.supports_matcher());
        assert!(HookEvent::SubagentStart.supports_matcher());
        assert!(HookEvent::TaskCreated.supports_matcher());
        assert!(HookEvent::PostToolBatch.supports_matcher());
        assert!(HookEvent::UserPromptSubmit.supports_matcher());
        assert!(!HookEvent::StopFailure.supports_matcher());
    }

    #[test]
    fn test_hook_event_serde() {
        let event = HookEvent::PreToolUse;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "\"PreToolUse\"");

        let event = HookEvent::SessionStart;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "\"SessionStart\"");

        let parsed: HookEvent = serde_json::from_str("\"PostToolUseFailure\"").unwrap();
        assert_eq!(parsed, HookEvent::PostToolUseFailure);
    }

    // -- HookContext tests --

    #[test]
    fn test_hook_context_for_pre_tool_use() {
        let ctx =
            HookContext::for_pre_tool_use("Bash", &json!({"command": "ls"}), "sess-1", "agent");
        assert_eq!(ctx.event, HookEvent::PreToolUse);
        assert_eq!(ctx.tool_name.as_deref(), Some("Bash"));
        assert_eq!(ctx.session_id, "sess-1");
        assert!(ctx.tool_output.is_none());
        assert!(ctx.matcher.is_none());
    }

    #[test]
    fn test_hook_context_for_session_start() {
        let ctx = HookContext::for_session_start("startup", "sess-1", "agent");
        assert_eq!(ctx.event, HookEvent::SessionStart);
        assert_eq!(ctx.matcher.as_deref(), Some("startup"));
        assert!(ctx.tool_name.is_none());
    }

    #[test]
    fn test_hook_context_for_stop() {
        let ctx = HookContext::for_stop(None, "sess-1", "agent", false);
        assert_eq!(ctx.event, HookEvent::Stop);
        assert!(ctx.matcher.is_none());
        assert!(ctx.tool_name.is_none());
    }

    #[test]
    fn test_hook_context_for_notification() {
        let ctx = HookContext::for_notification("permission_prompt", "sess-1", "agent");
        assert_eq!(ctx.event, HookEvent::Notification);
        assert_eq!(ctx.matcher.as_deref(), Some("permission_prompt"));
    }

    #[test]
    fn test_hook_context_serialization() {
        let ctx =
            HookContext::for_pre_tool_use("Bash", &json!({"command": "ls"}), "sess-1", "agent");
        let json_str = serde_json::to_string(&ctx).unwrap();
        assert!(json_str.contains("\"event\":\"PreToolUse\""));
        assert!(json_str.contains("\"tool_name\":\"Bash\""));
        // None fields should be omitted
        assert!(!json_str.contains("\"tool_output\""));
        assert!(!json_str.contains("\"matcher\""));
    }

    // -- matches_hook tests --

    #[test]
    fn test_matches_hook_wildcard() {
        let ctx = HookContext::for_pre_tool_use("Bash", &json!({}), "", "");
        assert!(matches_hook("*", &ctx));
        assert!(matches_hook("", &ctx));
    }

    #[test]
    fn test_matches_hook_tool_name_exact() {
        let ctx = HookContext::for_pre_tool_use("Bash", &json!({}), "", "");
        assert!(matches_hook("Bash", &ctx));
        assert!(!matches_hook("Read", &ctx));
    }

    #[test]
    fn test_matches_hook_tool_name_pipe_separated() {
        let ctx = HookContext::for_pre_tool_use("Edit", &json!({}), "", "");
        assert!(matches_hook("Edit|Write", &ctx));
        assert!(matches_hook("Write|Edit", &ctx));
        assert!(!matches_hook("Bash|Read", &ctx));
    }

    #[test]
    fn test_matches_hook_tool_name_prefix() {
        let ctx = HookContext::for_pre_tool_use("Bash(git:*)", &json!({}), "", "");
        assert!(matches_hook("Bash", &ctx));
    }

    #[test]
    fn test_matches_hook_lifecycle_matcher() {
        let ctx = HookContext::for_session_start("startup", "", "");
        assert!(matches_hook("startup", &ctx));
        assert!(!matches_hook("resume", &ctx));
    }

    #[test]
    fn test_matches_hook_lifecycle_pipe_separated() {
        let ctx = HookContext::for_session_start("resume", "", "");
        assert!(matches_hook("startup|resume", &ctx));
    }

    #[test]
    fn test_matches_hook_lifecycle_glob() {
        let ctx = HookContext::for_notification("permission_prompt", "", "");
        assert!(matches_hook("permission*", &ctx));
    }

    #[test]
    fn test_matches_hook_no_matcher_event() {
        let ctx = HookContext::for_stop(None, "", "", false);
        // Stop doesn't support matcher, so only "*" and "" match
        assert!(matches_hook("*", &ctx));
        assert!(matches_hook("", &ctx));
        assert!(!matches_hook("something", &ctx));
    }

    #[test]
    fn test_matches_hook_subagent_start() {
        let ctx = HookContext::for_subagent_start("coder", "sync", "implement X", "", "");
        assert!(matches_hook("coder", &ctx));
        assert!(matches_hook("*", &ctx));
        assert!(matches_hook("", &ctx));
        assert!(!matches_hook("planner", &ctx));
        // Pipe-separated alternatives
        assert!(matches_hook("planner|coder|reviewer", &ctx));
    }

    #[test]
    fn test_matches_hook_subagent_stop() {
        let ctx = HookContext::for_subagent_stop("coder", "sync", "success", "", "");
        assert!(matches_hook("coder", &ctx));
        assert!(!matches_hook("planner", &ctx));
    }

    #[test]
    fn test_matches_hook_task_created() {
        let ctx = HookContext::for_task_created("t-1", "build API", "", "");
        assert!(matches_hook("build API", &ctx));
        assert!(matches_hook("*", &ctx));
        assert!(!matches_hook("deploy", &ctx));
    }

    #[test]
    fn test_matches_hook_task_completed() {
        let ctx = HookContext::for_task_completed("t-1", "build API", "success", "", "");
        assert!(matches_hook("build API", &ctx));
        assert!(!matches_hook("deploy", &ctx));
    }

    #[test]
    fn test_matches_hook_subagent_glob() {
        let ctx = HookContext::for_subagent_start("code-reviewer", "sync", "review", "", "");
        assert!(matches_hook("code*", &ctx));
        assert!(!matches_hook("test*", &ctx));
    }

    // -- HooksDefinition tests --

    #[test]
    fn test_hooks_definition_empty() {
        let def = HooksDefinition::default();
        assert!(def.is_empty());
    }

    #[test]
    fn test_hooks_definition_add_rules() {
        let mut def = HooksDefinition::default();
        def.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "test".into(),
                }],
            }],
        );
        assert!(!def.is_empty());
        assert_eq!(def.rules_for(HookEvent::PreToolUse).len(), 1);
        assert_eq!(def.rules_for(HookEvent::PostToolUse).len(), 0);
    }

    #[test]
    fn test_hooks_definition_merge() {
        let mut def1 = HooksDefinition::default();
        def1.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "from-1".into(),
                }],
            }],
        );

        let mut def2 = HooksDefinition::default();
        def2.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Read".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "from-2".into(),
                }],
            }],
        );
        def2.add_rules(
            HookEvent::SessionStart,
            vec![HookRule {
                matcher: "startup".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "welcome".into(),
                }],
            }],
        );

        def1.merge(def2);
        assert_eq!(def1.rules_for(HookEvent::PreToolUse).len(), 2);
        assert_eq!(def1.rules_for(HookEvent::SessionStart).len(), 1);
    }

    #[test]
    fn test_hooks_definition_deserialize_yaml() {
        let yaml = r#"
PreToolUse:
  - matcher: "Bash"
    hooks:
      - type: command
        command: "echo test"
        timeout: 5
      - type: prompt
        prompt: "Be careful"
PostToolUse:
  - matcher: "*"
    hooks:
      - type: command
        command: "echo done"
SessionStart:
  - matcher: "startup"
    hooks:
      - type: prompt
        prompt: "Welcome!"
"#;
        let def: HooksDefinition = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(!def.is_empty());
        assert_eq!(def.rules_for(HookEvent::PreToolUse).len(), 1);
        assert_eq!(def.rules_for(HookEvent::PreToolUse)[0].hooks.len(), 2);
        assert_eq!(def.rules_for(HookEvent::PostToolUse).len(), 1);
        assert_eq!(def.rules_for(HookEvent::SessionStart).len(), 1);
    }

    #[test]
    fn test_hooks_definition_deserialize_with_http_action() {
        let yaml = r#"
PostToolUse:
  - matcher: "Bash"
    hooks:
      - type: http
        url: "https://audit.example.com/tool-usage"
        timeout: 3
"#;
        let def: HooksDefinition = serde_yaml_ng::from_str(yaml).unwrap();
        let rules = def.rules_for(HookEvent::PostToolUse);
        assert_eq!(rules.len(), 1);
        assert!(
            matches!(&rules[0].hooks[0], HookAction::Http { url, .. } if url == "https://audit.example.com/tool-usage")
        );
    }

    #[test]
    fn test_hooks_definition_deserialize_with_mcp_tool_action() {
        let yaml = r#"
Notification:
  - matcher: "permission_prompt"
    hooks:
      - type: mcp_tool
        server: "slack"
        tool: "send_message"
        arguments:
          channel: "agent-approvals"
"#;
        let def: HooksDefinition = serde_yaml_ng::from_str(yaml).unwrap();
        let rules = def.rules_for(HookEvent::Notification);
        assert_eq!(rules.len(), 1);
        assert!(
            matches!(&rules[0].hooks[0], HookAction::McpTool { server, .. } if server == "slack")
        );
    }

    // -- HookResult tests --

    #[test]
    fn test_hook_result_allow() {
        let result = HookResult::allow();
        assert!(result.has_permission_decision());
        assert!(result.permission_decision.unwrap().is_allowed());
    }

    #[test]
    fn test_hook_result_deny() {
        let result = HookResult::deny("test reason".to_string());
        assert!(result.has_permission_decision());
        assert!(result.permission_decision.unwrap().is_denied());
        assert!(result.block);
    }

    #[test]
    fn test_hook_result_ask() {
        let result = HookResult::ask(vec!["Option A".to_string()]);
        assert!(result.has_permission_decision());
        assert!(result.permission_decision.unwrap().requires_approval());
    }

    #[test]
    fn test_hook_result_should_continue() {
        let mut result = HookResult::default();
        assert!(!result.should_continue());
        result.continue_reason = Some("Check tests".to_string());
        assert!(result.should_continue());
    }

    // -- parse_hook_output tests --

    #[test]
    fn test_parse_hook_output_empty() {
        let result = parse_hook_output("", 0);
        assert!(!result.block);
    }

    #[test]
    fn test_parse_hook_output_block() {
        let result = parse_hook_output(r#"{"decision": "block", "reason": "unsafe"}"#, 0);
        assert!(result.block);
        assert_eq!(result.block_reason, Some("unsafe".into()));
    }

    #[test]
    fn test_parse_hook_output_allow() {
        let result = parse_hook_output(r#"{"decision": "allow"}"#, 1);
        assert!(!result.block);
    }

    #[test]
    fn test_parse_hook_output_nonzero_exit_code_1_no_block() {
        // exit 1 = no block (hook produced output but no block intent)
        let result = parse_hook_output("", 1);
        assert!(!result.block);
    }

    #[test]
    fn test_parse_hook_output_exit_code_2_block() {
        // exit 2 = explicit block signal
        let result = parse_hook_output("", 2);
        assert!(result.block);
        assert_eq!(
            result.block_reason,
            Some("Hook exited with code 2 (explicit block)".to_string())
        );
    }

    #[test]
    fn test_parse_hook_output_exit_code_other_no_block() {
        // Other non-zero = warning, no block
        let result = parse_hook_output("", 3);
        assert!(!result.block);
        let result = parse_hook_output("", 127);
        assert!(!result.block);
    }

    #[test]
    fn test_parse_hook_output_retry_field() {
        let result = parse_hook_output(r#"{"retry": true}"#, 0);
        assert!(result.retry);
    }

    #[test]
    fn test_parse_hook_output_retry_false_by_default() {
        let result = parse_hook_output("", 0);
        assert!(!result.retry);
    }

    #[test]
    fn test_parse_hook_output_updated_input() {
        let result = parse_hook_output(r#"{"updatedInput": {"command": "safe-command"}}"#, 0);
        assert!(!result.block);
        assert_eq!(
            result.updated_input,
            Some(json!({"command": "safe-command"}))
        );
    }

    #[test]
    fn test_parse_hook_output_continue_reason() {
        let result = parse_hook_output(r#"{"continue_reason": "Run tests before stopping"}"#, 0);
        assert_eq!(
            result.continue_reason,
            Some("Run tests before stopping".to_string())
        );
    }

    #[test]
    fn test_parse_hook_output_injected_context() {
        let result = parse_hook_output(r#"{"injected_context": "Remember to use bun"}"#, 0);
        assert_eq!(
            result.injected_context,
            Some("Remember to use bun".to_string())
        );
    }

    #[test]
    fn test_parse_hook_output_non_json_as_context() {
        let result = parse_hook_output("Remember: use bun, not npm", 0);
        assert_eq!(
            result.injected_context,
            Some("Remember: use bun, not npm".to_string())
        );
    }

    #[test]
    fn test_parse_hook_output_permission_decision_allow() {
        let result = parse_hook_output(r#"{"permission_decision": "allow"}"#, 0);
        assert!(result.has_permission_decision());
        assert!(result.permission_decision.unwrap().is_allowed());
    }

    #[test]
    fn test_parse_hook_output_permission_decision_deny() {
        let result = parse_hook_output(
            r#"{"permission_decision": "deny", "permission_reason": "unsafe"}"#,
            0,
        );
        assert!(result.has_permission_decision());
        assert!(result.permission_decision.unwrap().is_denied());
    }

    #[test]
    fn test_parse_hook_output_permission_mode() {
        let result = parse_hook_output(r#"{"permission_mode": "auto"}"#, 0);
        assert_eq!(result.permission_mode_override, Some(PermissionMode::Auto));
    }

    // -- merge_result tests --

    #[test]
    fn test_merge_result_permission_priority_deny_wins() {
        let mut combined = HookResult {
            permission_decision: Some(PermissionDecision::Allow),
            ..HookResult::default()
        };

        let incoming = HookResult {
            permission_decision: Some(PermissionDecision::Ask {
                suggestions: vec!["Allow".to_string()],
            }),
            ..HookResult::default()
        };
        merge_result(&mut combined, incoming);
        assert!(matches!(
            combined.permission_decision.clone().unwrap(),
            PermissionDecision::Ask { .. }
        ));

        let incoming2 = HookResult {
            permission_decision: Some(PermissionDecision::Deny {
                reason: "unsafe".to_string(),
            }),
            ..HookResult::default()
        };
        merge_result(&mut combined, incoming2);
        assert!(matches!(
            combined.permission_decision.clone().unwrap(),
            PermissionDecision::Deny { .. }
        ));

        let incoming3 = HookResult {
            permission_decision: Some(PermissionDecision::Allow),
            ..HookResult::default()
        };
        merge_result(&mut combined, incoming3);
        assert!(matches!(
            combined.permission_decision.clone().unwrap(),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn test_merge_result_continue_reason() {
        let mut combined = HookResult::default();
        merge_result(
            &mut combined,
            HookResult {
                continue_reason: Some("Check tests".to_string()),
                ..HookResult::default()
            },
        );
        assert_eq!(combined.continue_reason, Some("Check tests".to_string()));

        // Later non-None overrides
        merge_result(
            &mut combined,
            HookResult {
                continue_reason: Some("Different reason".to_string()),
                ..HookResult::default()
            },
        );
        assert_eq!(
            combined.continue_reason,
            Some("Different reason".to_string())
        );
    }

    #[test]
    fn test_merge_result_injected_context_concatenates() {
        let mut combined = HookResult::default();
        merge_result(
            &mut combined,
            HookResult {
                injected_context: Some("First".to_string()),
                ..HookResult::default()
            },
        );
        assert_eq!(combined.injected_context, Some("First".to_string()));

        merge_result(
            &mut combined,
            HookResult {
                injected_context: Some("Second".to_string()),
                ..HookResult::default()
            },
        );
        assert_eq!(combined.injected_context, Some("First\nSecond".to_string()));
    }

    #[test]
    fn test_merge_result_metadata_deep_merge() {
        let mut combined = HookResult {
            metadata: Some(json!({"a": 1, "b": 2})),
            ..HookResult::default()
        };
        merge_result(
            &mut combined,
            HookResult {
                metadata: Some(json!({"b": 3, "c": 4})),
                ..HookResult::default()
            },
        );
        let meta = combined.metadata.unwrap();
        assert_eq!(meta["a"], 1);
        assert_eq!(meta["b"], 3); // overwritten
        assert_eq!(meta["c"], 4);
    }

    #[test]
    fn test_merge_result_permission_mode_override_last_wins() {
        let mut combined = HookResult {
            permission_mode_override: Some(PermissionMode::Auto),
            ..HookResult::default()
        };
        merge_result(
            &mut combined,
            HookResult {
                permission_mode_override: Some(PermissionMode::Plan),
                ..HookResult::default()
            },
        );
        assert_eq!(
            combined.permission_mode_override,
            Some(PermissionMode::Plan)
        );

        merge_result(
            &mut combined,
            HookResult {
                permission_mode_override: None,
                ..HookResult::default()
            },
        );
        assert_eq!(
            combined.permission_mode_override,
            Some(PermissionMode::Plan)
        );
    }

    #[test]
    fn test_merge_result_retry_or_semantics() {
        let mut combined = HookResult::default();
        assert!(!combined.retry);

        merge_result(
            &mut combined,
            HookResult {
                retry: true,
                ..HookResult::default()
            },
        );
        assert!(combined.retry);

        // Once true, stays true even if incoming is false
        merge_result(
            &mut combined,
            HookResult {
                retry: false,
                ..HookResult::default()
            },
        );
        assert!(combined.retry);
    }

    // -- HookRegistry tests --

    #[test]
    fn test_hook_registry_empty() {
        let registry = HookRegistry::new();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_hook_registry_register_skill() {
        let mut registry = HookRegistry::new();
        let mut def = HooksDefinition::default();
        def.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "test".into(),
                }],
            }],
        );
        registry.register("test-skill", "/tmp/test", def);
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_hook_registry_register_user_hooks() {
        let mut registry = HookRegistry::new();
        let mut def = HooksDefinition::default();
        def.add_rules(
            HookEvent::Stop,
            vec![HookRule {
                matcher: String::new(),
                hooks: vec![HookAction::Prompt {
                    prompt: "Don't stop".into(),
                }],
            }],
        );
        registry.register_user_hooks(def);
        assert!(!registry.is_empty());
        assert!(registry.has_hooks_for(HookEvent::Stop));
        assert!(!registry.has_hooks_for(HookEvent::PreToolUse));
    }

    #[test]
    fn test_hook_registry_unregister() {
        let mut registry = HookRegistry::new();
        let mut def = HooksDefinition::default();
        def.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "test".into(),
                }],
            }],
        );
        registry.register("test-skill", "/tmp/test", def);
        assert!(!registry.is_empty());

        let removed = registry.unregister(&HookSource::Skill("test-skill".to_string()));
        assert!(removed);
        assert!(registry.is_empty());
    }

    #[tokio::test]
    async fn test_hook_registry_no_match() {
        let mut registry = HookRegistry::new();
        let mut def = HooksDefinition::default();
        def.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Write".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "check".into(),
                }],
            }],
        );
        registry.register("test", "/tmp", def);

        let result = registry.run_pre_tool_use("Read", &json!({}), "").await;
        assert!(result.messages.is_empty());
    }

    #[tokio::test]
    async fn test_hook_registry_prompt_match() {
        let mut registry = HookRegistry::new();
        let mut def = HooksDefinition::default();
        def.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "Verify the command is safe".into(),
                }],
            }],
        );
        registry.register("security", "/tmp", def);

        let result = registry
            .run_pre_tool_use("Bash", &json!({"command": "ls"}), "")
            .await;
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0], "Verify the command is safe");
    }

    #[tokio::test]
    async fn test_hook_registry_command_execution() {
        if cfg!(target_os = "windows") {
            return;
        }
        let mut registry = HookRegistry::new();
        let mut def = HooksDefinition::default();
        def.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Command {
                    command: r#"echo '{"decision":"allow"}'"#.into(),
                    shell: None,
                    timeout: 5,
                }],
            }],
        );
        registry.register("test", "/tmp", def);

        let result = registry
            .run_pre_tool_use("Bash", &json!({"command": "ls"}), "")
            .await;
        assert!(!result.block);
    }

    #[tokio::test]
    async fn test_hook_registry_lifecycle_hooks() {
        let mut registry = HookRegistry::new();
        let mut def = HooksDefinition::default();
        def.add_rules(
            HookEvent::SessionStart,
            vec![HookRule {
                matcher: "startup".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "Welcome!".into(),
                }],
            }],
        );
        registry.register("test", "/tmp", def);

        let ctx = HookContext::for_session_start("startup", "sess-1", "agent");
        let result = registry.run_lifecycle_hooks(&ctx).await;
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0], "Welcome!");

        // Non-matching matcher
        let ctx = HookContext::for_session_start("resume", "sess-1", "agent");
        let result = registry.run_lifecycle_hooks(&ctx).await;
        assert!(result.messages.is_empty());
    }

    #[tokio::test]
    async fn test_hook_registry_stop_hook_with_continue() {
        let mut registry = HookRegistry::new();
        let mut def = HooksDefinition::default();
        def.add_rules(
            HookEvent::Stop,
            vec![HookRule {
                matcher: String::new(),
                hooks: vec![HookAction::Command {
                    command: r#"echo '{"continue_reason": "Run tests first"}'"#.into(),
                    shell: None,
                    timeout: 5,
                }],
            }],
        );
        registry.register("test", "/tmp", def);

        let ctx = HookContext::for_stop(None, "sess-1", "agent", false);
        let result = registry.run_lifecycle_hooks(&ctx).await;
        assert_eq!(result.continue_reason, Some("Run tests first".to_string()));
    }

    #[tokio::test]
    async fn test_hook_registry_user_config_priority() {
        let mut registry = HookRegistry::new();

        // Register skill hook first
        let mut skill_def = HooksDefinition::default();
        skill_def.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "from-skill".into(),
                }],
            }],
        );
        registry.register("z-skill", "/tmp", skill_def);

        // Register user config
        let mut user_def = HooksDefinition::default();
        user_def.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "from-user".into(),
                }],
            }],
        );
        registry.register_user_hooks(user_def);

        let result = registry.run_pre_tool_use("Bash", &json!({}), "").await;
        // User config runs first, then skill
        assert_eq!(result.messages, vec!["from-user", "from-skill"]);
    }

    #[tokio::test]
    async fn test_hook_registry_runs_in_deterministic_skill_name_order() {
        let mut registry = HookRegistry::new();
        let mut z_def = HooksDefinition::default();
        z_def.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "from-z".into(),
                }],
            }],
        );
        registry.register("z-skill", "/tmp", z_def);

        let mut a_def = HooksDefinition::default();
        a_def.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "from-a".into(),
                }],
            }],
        );
        registry.register("a-skill", "/tmp", a_def);

        let result = registry
            .run_pre_tool_use("Bash", &json!({"command": "ls"}), "")
            .await;
        assert_eq!(result.messages, vec!["from-a", "from-z"]);
    }

    #[tokio::test]
    async fn test_hook_command_receives_session_id() {
        if cfg!(target_os = "windows") {
            return;
        }
        let mut registry = HookRegistry::new();
        let mut def = HooksDefinition::default();
        def.add_rules(
            HookEvent::PreToolUse,
            vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Command {
                    command: r#"printf '{"updatedInput":{"session_id":"%s"}}' "$SESSION_ID""#
                        .into(),
                    shell: None,
                    timeout: 5,
                }],
            }],
        );
        registry.register("session-skill", "/tmp", def);

        let result = registry
            .run_pre_tool_use("Bash", &json!({"command": "ls"}), "sess-123")
            .await;
        assert_eq!(
            result.updated_input,
            Some(json!({"session_id": "sess-123"}))
        );
    }

    #[test]
    fn test_matches_tool_name_glob() {
        assert!(matches_tool_name("Bash(*)", "Bash(git:*)"));
        assert!(matches_tool_name("Bash", "Bash(git:*)"));
        assert!(matches_tool_name("*", "Read"));
        assert!(!matches_tool_name("Bash", "Read"));
        assert!(!matches_tool_name("Write", "Bash(git:*)"));
    }

    // -- HookAction serde tests --

    #[test]
    fn test_hook_action_http_deserialize() {
        let json = r#"{"type":"http","url":"https://example.com","timeout":5}"#;
        let action: HookAction = serde_json::from_str(json).unwrap();
        assert!(
            matches!(action, HookAction::Http { url, timeout: 5, .. } if url == "https://example.com")
        );
    }

    #[test]
    fn test_hook_action_mcp_tool_deserialize() {
        let json = r#"{"type":"mcp_tool","server":"slack","tool":"send_message","arguments":{"channel":"test"}}"#;
        let action: HookAction = serde_json::from_str(json).unwrap();
        assert!(
            matches!(action, HookAction::McpTool { server, tool, .. } if server == "slack" && tool == "send_message")
        );
    }

    // -- HookAction validation tests --

    #[test]
    fn test_hook_action_validate_command_ok() {
        let action = HookAction::Command {
            command: "echo hello".into(),
            shell: None,
            timeout: 10,
        };
        assert!(action.validate().is_ok());
    }

    #[test]
    fn test_hook_action_validate_command_empty() {
        let action = HookAction::Command {
            command: "".into(),
            shell: None,
            timeout: 10,
        };
        assert!(action.validate().is_err());
    }

    #[test]
    fn test_hook_action_validate_command_timeout_exceeds_max() {
        let action = HookAction::Command {
            command: "echo hi".into(),
            shell: None,
            timeout: 99999,
        };
        assert!(action.validate().is_err());
    }

    #[test]
    fn test_hook_action_validate_prompt_empty() {
        let action = HookAction::Prompt { prompt: "".into() };
        assert!(action.validate().is_err());
    }

    #[test]
    fn test_hook_action_validate_permission_invalid() {
        let action = HookAction::Permission {
            decision: "maybe".into(),
            reason: None,
            suggestions: vec![],
        };
        assert!(action.validate().is_err());
    }

    #[test]
    fn test_hook_action_validate_permission_valid() {
        for dec in &["allow", "deny", "ask"] {
            let action = HookAction::Permission {
                decision: (*dec).into(),
                reason: None,
                suggestions: vec![],
            };
            assert!(
                action.validate().is_ok(),
                "decision '{}' should be valid",
                dec
            );
        }
    }

    #[test]
    fn test_hook_action_validate_http_empty_url() {
        let action = HookAction::Http {
            url: "".into(),
            method: None,
            headers: None,
            timeout: 10,
        };
        assert!(action.validate().is_err());
    }

    #[test]
    fn test_hook_action_validate_http_timeout_exceeds_max() {
        let action = HookAction::Http {
            url: "https://example.com".into(),
            method: None,
            headers: None,
            timeout: 99999,
        };
        assert!(action.validate().is_err());
    }

    #[test]
    fn test_hook_action_validate_mcp_empty_server() {
        let action = HookAction::McpTool {
            server: "".into(),
            tool: "send".into(),
            arguments: None,
            timeout: 10,
        };
        assert!(action.validate().is_err());
    }

    #[test]
    fn test_hook_action_validate_mcp_timeout_exceeds_max() {
        let action = HookAction::McpTool {
            server: "s".into(),
            tool: "t".into(),
            arguments: None,
            timeout: 99999,
        };
        assert!(action.validate().is_err());
    }
}
