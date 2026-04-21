//! Skill Hooks -- intercept tool calls before/after execution.
//!
//! Hooks allow skills to extend the agent's behavior by running commands,
//! injecting prompts, or making HTTP calls at specific points in the tool
//! execution lifecycle.
//!
//! ## Hook events
//!
//! | Event | When | Can modify |
//! |-------|------|-----------|
//! | `PreToolUse` | Before tool execution | Input, permission (allow/block) |
//! | `PostToolUse` | After tool execution | Output, continuation |
//!
//! ## Hook types
//!
//! | Type | Behavior |
//! |------|----------|
//! | `command` | Execute a shell command, stdin receives JSON context |
//! | `prompt` | Inject a prompt message for the LLM to consider |
//! | `permission` | Return a permission decision (allow/deny/ask) |
//!
//! ## YAML format in SKILL.md frontmatter
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
//!         - type: permission
//!           decision: "ask"
//!           suggestions:
//!             - "Allow write"
//!             - "Deny"
//!   PostToolUse:
//!     - matcher: "Bash"
//!       hooks:
//!         - type: command
//!           command: "echo 'done'"
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use echo_core::tools::permission::{PermissionDecision, PermissionMode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::sandbox::{SandboxCommand, SandboxManager};
use crate::skills::minimal_hook_env;

// -- Hook types --

/// When a hook fires relative to tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
}

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
}

fn default_hook_timeout() -> u64 {
    10
}

/// A hook rule: a matcher pattern + one or more actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRule {
    /// Tool name pattern to match. `"*"` matches all tools.
    /// Can be an exact tool name or a glob-like prefix (e.g., `"Bash"` matches `"Bash"`).
    pub matcher: String,

    /// Actions to execute when the matcher matches.
    pub hooks: Vec<HookAction>,
}

/// Complete hooks definition from a skill's frontmatter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksDefinition {
    #[serde(default, alias = "PreToolUse")]
    pub pre_tool_use: Vec<HookRule>,

    #[serde(default, alias = "PostToolUse")]
    pub post_tool_use: Vec<HookRule>,
}

/// Result of executing a hook.
#[derive(Debug, Clone, Default)]
pub struct HookResult {
    /// If true, the tool call should be blocked.
    pub block: bool,
    /// Reason for blocking (if block is true).
    pub block_reason: Option<String>,
    /// Modified tool input (PreToolUse only).
    pub updated_input: Option<Value>,
    /// Messages to inject into context.
    pub messages: Vec<String>,
    /// If true, prevent further hooks from running.
    pub stop_propagation: bool,
    /// Permission decision (PreToolUse only, overrides normal permission check).
    pub permission_decision: Option<PermissionDecision>,
    /// Permission mode override (PreToolUse only).
    pub permission_mode_override: Option<PermissionMode>,
}

impl HookResult {
    /// Create an allow result
    pub fn allow() -> Self {
        Self {
            permission_decision: Some(PermissionDecision::Allow),
            ..Self::default()
        }
    }

    /// Create a deny result
    pub fn deny(reason: String) -> Self {
        Self {
            block: true,
            block_reason: Some(reason.clone()),
            permission_decision: Some(PermissionDecision::Deny { reason }),
            ..Self::default()
        }
    }

    /// Create an ask result
    pub fn ask(suggestions: Vec<String>) -> Self {
        Self {
            permission_decision: Some(PermissionDecision::Ask { suggestions }),
            ..Self::default()
        }
    }

    /// Check if hook made a permission decision
    pub fn has_permission_decision(&self) -> bool {
        self.permission_decision.is_some()
    }
}

// -- HookRegistry --

/// Registry of hooks from all activated skills.
#[derive(Debug, Clone, Default)]
pub struct HookRegistry {
    /// skill_name -> hooks definition
    skills: HashMap<String, RegisteredHook>,
    /// Optional sandbox manager for executing hook commands.
    sandbox: Option<Arc<SandboxManager>>,
}

#[derive(Debug, Clone)]
struct RegisteredHook {
    definition: HooksDefinition,
    skill_dir: String,
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

    /// Attach or replace the sandbox manager for executing hook commands.
    pub fn set_sandbox_manager(&mut self, manager: Arc<SandboxManager>) {
        self.sandbox = Some(manager);
    }

    /// Register hooks from a skill.
    pub fn register(&mut self, skill_name: &str, skill_dir: &str, definition: HooksDefinition) {
        if definition.pre_tool_use.is_empty() && definition.post_tool_use.is_empty() {
            return;
        }
        info!(
            skill = skill_name,
            pre_hooks = definition.pre_tool_use.len(),
            post_hooks = definition.post_tool_use.len(),
            "Registered skill hooks"
        );
        self.skills.insert(
            skill_name.to_string(),
            RegisteredHook {
                definition,
                skill_dir: skill_dir.to_string(),
            },
        );
    }

    /// Check if any hooks are registered.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Execute all matching PreToolUse hooks.
    pub async fn run_pre_tool_use(
        &self,
        tool_name: &str,
        tool_input: &Value,
        session_id: &str,
    ) -> HookResult {
        self.run_hooks(
            HookEvent::PreToolUse,
            tool_name,
            tool_input,
            None,
            session_id,
        )
        .await
    }

    /// Execute all matching PostToolUse hooks.
    pub async fn run_post_tool_use(
        &self,
        tool_name: &str,
        tool_input: &Value,
        tool_output: &str,
        session_id: &str,
    ) -> HookResult {
        self.run_hooks(
            HookEvent::PostToolUse,
            tool_name,
            tool_input,
            Some(tool_output),
            session_id,
        )
        .await
    }

    async fn run_hooks(
        &self,
        event: HookEvent,
        tool_name: &str,
        tool_input: &Value,
        tool_output: Option<&str>,
        session_id: &str,
    ) -> HookResult {
        let mut combined = HookResult::default();
        let mut skill_names: Vec<&str> = self.skills.keys().map(String::as_str).collect();
        skill_names.sort_unstable();

        for skill_name in skill_names {
            let Some(registered) = self.skills.get(skill_name) else {
                continue;
            };
            let rules = match event {
                HookEvent::PreToolUse => &registered.definition.pre_tool_use,
                HookEvent::PostToolUse => &registered.definition.post_tool_use,
            };

            for rule in rules {
                if !matches_tool(&rule.matcher, tool_name) {
                    continue;
                }

                debug!(
                    skill = %skill_name,
                    event = ?event,
                    tool = tool_name,
                    matcher = &rule.matcher,
                    "Hook matched"
                );

                for action in &rule.hooks {
                    let result = execute_action(
                        action,
                        &registered.skill_dir,
                        tool_name,
                        tool_input,
                        tool_output,
                        session_id,
                        self.sandbox.as_ref(),
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

// -- Matcher --

fn matches_tool(matcher: &str, tool_name: &str) -> bool {
    if matcher == "*" {
        return true;
    }
    if matcher == tool_name {
        return true;
    }
    // Try glob matching: e.g. "Bash(*)" matches "Bash(git:*)"
    if let Ok(pattern) = glob::Pattern::new(matcher) {
        if pattern.matches(tool_name) {
            return true;
        }
    }
    // Fallback: prefix match for patterns like "Bash" matching "Bash(git:*)"
    if tool_name.starts_with(matcher)
        && tool_name.len() > matcher.len()
        && tool_name.as_bytes()[matcher.len()] == b'('
    {
        return true;
    }
    false
}

// -- Action execution --

async fn execute_action(
    action: &HookAction,
    skill_dir: &str,
    tool_name: &str,
    tool_input: &Value,
    tool_output: Option<&str>,
    session_id: &str,
    sandbox: Option<&Arc<SandboxManager>>,
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
                skill_dir,
                tool_name,
                tool_input,
                tool_output,
                session_id,
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
    }
}

async fn execute_command_hook(
    command: &str,
    shell: Option<&str>,
    timeout_secs: u64,
    skill_dir: &str,
    tool_name: &str,
    tool_input: &Value,
    tool_output: Option<&str>,
    session_id: &str,
    sandbox: Option<&Arc<SandboxManager>>,
) -> HookResult {
    // Variable substitution in command
    let command = command
        .replace("${SKILL_DIR}", skill_dir)
        .replace("${CLAUDE_PLUGIN_ROOT}", skill_dir);

    // Build JSON context for stdin
    let stdin_json = json!({
        "hook_event_name": if tool_output.is_some() { "PostToolUse" } else { "PreToolUse" },
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_output": tool_output,
    });

    let timeout = Duration::from_secs(timeout_secs);

    // -- Sandbox execution path --
    if let Some(manager) = sandbox {
        let (program, args) = build_hook_shell_command(&command, shell);
        let mut sandbox_cmd = SandboxCommand::program(&program, args).with_timeout(timeout);

        if !skill_dir.is_empty() && Path::new(skill_dir).exists() {
            sandbox_cmd = sandbox_cmd.with_working_dir(skill_dir);
        }

        // Use minimal environment
        let env = minimal_hook_env(skill_dir, session_id);
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
                    debug!(command = %command, stderr = %result.stderr.trim(), "Hook stderr (sandboxed)");
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

    if !skill_dir.is_empty() && Path::new(skill_dir).exists() {
        cmd.current_dir(skill_dir);
    }

    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Use minimal environment to avoid leaking sensitive variables
    let env = minimal_hook_env(skill_dir, session_id);
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

/// Parse JSON output from a hook command.
///
/// Hooks can return JSON to control execution:
/// ```json
/// {
///   "decision": "block",
///   "reason": "Unsafe command detected",
///   "continue": false,
///   "permission_decision": "allow" | "deny" | "ask",
///   "permission_suggestions": ["Allow", "Deny"]
/// }
/// ```
fn parse_hook_output(stdout: &str, exit_code: i32) -> HookResult {
    let mut result = HookResult::default();

    // Non-zero exit = block by default
    if exit_code != 0 {
        result.block = true;
        result.block_reason = Some(format!("Hook exited with code {}", exit_code));
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
    }

    result
}

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
    // A deny from any hook takes precedence; otherwise ask wins over allow
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
            // Both are allow -- either is fine
            (Some(PermissionDecision::Allow), PermissionDecision::Allow) => false,
            // No existing decision
            (None, _) => true,
            // Allow replacing non-allow with allow (lower priority)
            _ => false,
        };
        if should_replace {
            combined.permission_decision = Some(new_decision);
        }
    }
    // permission_mode_override is intentionally "last non-none wins".
    // This keeps hook composition predictable without mixing it into the
    // deny > ask > allow decision priority matrix above.
    if incoming.permission_mode_override.is_some() {
        combined.permission_mode_override = incoming.permission_mode_override;
    }
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_tool_exact() {
        assert!(matches_tool("Bash", "Bash"));
        assert!(!matches_tool("Bash", "Read"));
    }

    #[test]
    fn test_matches_tool_wildcard() {
        assert!(matches_tool("*", "Bash"));
        assert!(matches_tool("*", "Read"));
        assert!(matches_tool("*", "Write"));
    }

    #[test]
    fn test_matches_tool_prefix() {
        assert!(matches_tool("Bash", "Bash(git:*)"));
        assert!(!matches_tool("Bash", "BashExtra"));
    }

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
    fn test_parse_hook_output_nonzero_exit() {
        let result = parse_hook_output("", 1);
        assert!(result.block);
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
    fn test_hook_definition_deserialize() {
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
"#;
        let def: HooksDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.pre_tool_use.len(), 1);
        assert_eq!(def.pre_tool_use[0].matcher, "Bash");
        assert_eq!(def.pre_tool_use[0].hooks.len(), 2);
        assert_eq!(def.post_tool_use.len(), 1);
    }

    #[test]
    fn test_hook_registry_empty() {
        let registry = HookRegistry::new();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_hook_registry_register() {
        let mut registry = HookRegistry::new();
        let def = HooksDefinition {
            pre_tool_use: vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "test".into(),
                }],
            }],
            post_tool_use: vec![],
        };
        registry.register("test-skill", "/tmp/test", def);
        assert!(!registry.is_empty());
    }

    #[tokio::test]
    async fn test_hook_registry_no_match() {
        let mut registry = HookRegistry::new();
        let def = HooksDefinition {
            pre_tool_use: vec![HookRule {
                matcher: "Write".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "check".into(),
                }],
            }],
            post_tool_use: vec![],
        };
        registry.register("test", "/tmp", def);

        let result = registry.run_pre_tool_use("Read", &json!({}), "").await;
        assert!(result.messages.is_empty());
    }

    #[tokio::test]
    async fn test_hook_registry_prompt_match() {
        let mut registry = HookRegistry::new();
        let def = HooksDefinition {
            pre_tool_use: vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "Verify the command is safe".into(),
                }],
            }],
            post_tool_use: vec![],
        };
        registry.register("security", "/tmp", def);

        let result = registry
            .run_pre_tool_use("Bash", &json!({"command": "ls"}), "")
            .await;
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0], "Verify the command is safe");
    }

    #[tokio::test]
    async fn test_hook_command_execution() {
        if cfg!(target_os = "windows") {
            return;
        }
        let mut registry = HookRegistry::new();
        let def = HooksDefinition {
            pre_tool_use: vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Command {
                    command: r#"echo '{"decision":"allow"}'"#.into(),
                    shell: None,
                    timeout: 5,
                }],
            }],
            post_tool_use: vec![],
        };
        registry.register("test", "/tmp", def);

        let result = registry
            .run_pre_tool_use("Bash", &json!({"command": "ls"}), "")
            .await;
        assert!(!result.block);
    }

    #[test]
    fn test_hook_result_permission_decision() {
        let result = HookResult::allow();
        assert!(result.has_permission_decision());
        assert!(result.permission_decision.unwrap().is_allowed());

        let result = HookResult::deny("test reason".to_string());
        assert!(result.has_permission_decision());
        assert!(result.permission_decision.unwrap().is_denied());

        let result = HookResult::ask(vec!["Option A".to_string()]);
        assert!(result.has_permission_decision());
        assert!(result.permission_decision.unwrap().requires_approval());
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
        let decision = result.permission_decision.unwrap();
        assert!(decision.is_denied());
    }

    #[test]
    fn test_parse_hook_output_permission_decision_ask() {
        let result = parse_hook_output(
            r#"{"permission_decision": "ask", "permission_suggestions": ["Allow", "Deny"]}"#,
            0,
        );
        assert!(result.has_permission_decision());
        assert!(result.permission_decision.unwrap().requires_approval());
    }

    #[test]
    fn test_parse_hook_output_permission_mode() {
        let result = parse_hook_output(r#"{"permission_mode": "auto"}"#, 0);
        assert_eq!(result.permission_mode_override, Some(PermissionMode::Auto));

        let result = parse_hook_output(r#"{"permission_mode": "plan"}"#, 0);
        assert_eq!(result.permission_mode_override, Some(PermissionMode::Plan));
    }

    #[tokio::test]
    async fn test_hook_action_permission() {
        let mut registry = HookRegistry::new();
        let def = HooksDefinition {
            pre_tool_use: vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Permission {
                    decision: "deny".into(),
                    reason: Some("unsafe command".into()),
                    suggestions: vec![],
                }],
            }],
            post_tool_use: vec![],
        };
        registry.register("security", "/tmp", def);

        let result = registry
            .run_pre_tool_use("Bash", &json!({"command": "rm -rf"}), "")
            .await;
        assert!(result.block);
        assert!(result.has_permission_decision());
    }

    #[tokio::test]
    async fn test_hook_command_receives_session_id() {
        if cfg!(target_os = "windows") {
            return;
        }
        let mut registry = HookRegistry::new();
        let def = HooksDefinition {
            pre_tool_use: vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Command {
                    command: r#"printf '{"updatedInput":{"session_id":"%s"}}' "$SESSION_ID""#
                        .into(),
                    shell: None,
                    timeout: 5,
                }],
            }],
            post_tool_use: vec![],
        };
        registry.register("session-skill", "/tmp", def);

        let result = registry
            .run_pre_tool_use("Bash", &json!({"command": "ls"}), "sess-123")
            .await;
        assert_eq!(
            result.updated_input,
            Some(json!({"session_id": "sess-123"}))
        );
    }

    #[tokio::test]
    async fn test_hook_registry_runs_in_deterministic_skill_name_order() {
        let mut registry = HookRegistry::new();
        registry.register(
            "z-skill",
            "/tmp",
            HooksDefinition {
                pre_tool_use: vec![HookRule {
                    matcher: "Bash".into(),
                    hooks: vec![HookAction::Prompt {
                        prompt: "from-z".into(),
                    }],
                }],
                post_tool_use: vec![],
            },
        );
        registry.register(
            "a-skill",
            "/tmp",
            HooksDefinition {
                pre_tool_use: vec![HookRule {
                    matcher: "Bash".into(),
                    hooks: vec![HookAction::Prompt {
                        prompt: "from-a".into(),
                    }],
                }],
                post_tool_use: vec![],
            },
        );

        let result = registry
            .run_pre_tool_use("Bash", &json!({"command": "ls"}), "")
            .await;
        assert_eq!(result.messages, vec!["from-a", "from-z"]);
    }

    #[test]
    fn test_merge_result_permission_priority_deny_wins() {
        // deny > ask > allow
        let mut combined = HookResult::default();
        combined.permission_decision = Some(PermissionDecision::Allow);

        // Ask should override allow
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

        // Deny should override ask
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

        // Once deny is set, allow cannot override it
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
    fn test_matches_tool_glob() {
        assert!(matches_tool("Bash(*)", "Bash(git:*)"));
        assert!(matches_tool("Bash", "Bash(git:*)"));
        assert!(matches_tool("*", "Read"));
        assert!(!matches_tool("Bash", "Read"));
        assert!(!matches_tool("Write", "Bash(git:*)"));
    }
}
