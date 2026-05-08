use echo_core::sandbox::SandboxExecutor;
use echo_core::tools::Tool;
use echo_core::tools::skill::Skill;
use echo_tools::shell::ShellTool;
use std::sync::Arc;

/// Shell command execution skill
///
/// Provides the Agent with controlled shell command execution capabilities,
/// with a three-tier security policy:
/// - **Safe**: direct execution (ls / cat / git status / cargo check, etc.)
/// - **RequiresApproval**: reject and prompt for human confirmation (rm / curl / npm, etc.)
/// - **Dangerous**: explicitly denied (sudo / dd / shutdown, etc.)
///
/// # Usage
/// ```rust,ignore
/// use echo_agent::prelude::{ShellSkill, AgentConfig, ReactAgent};
///
/// let config = AgentConfig::new("qwen3-max", "shell", "You are a shell assistant");
/// let mut agent = ReactAgent::new(config);
/// // Strict mode (default): only allow whitelisted commands
/// agent.add_skill(Box::new(ShellSkill::new()));
///
/// // Permissive mode: commands outside the whitelist can also be executed (not recommended for production)
/// agent.add_skill(Box::new(ShellSkill::permissive()));
/// ```
pub struct ShellSkill {
    permissive: bool,
}

impl ShellSkill {
    /// Create strict mode (default): only allow whitelisted commands
    pub fn new() -> Self {
        Self { permissive: false }
    }

    /// Create permissive mode: unknown commands outside the whitelist can also be executed
    pub fn permissive() -> Self {
        Self { permissive: true }
    }
}

impl Default for ShellSkill {
    fn default() -> Self {
        Self::new()
    }
}

impl Skill for ShellSkill {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Controlled shell command execution capability: supports file viewing, directory operations, code building (git/cargo), searching, and other safe commands"
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        let tool = if self.permissive {
            ShellTool::new_permissive()
        } else {
            ShellTool::new()
        };
        vec![Box::new(tool)]
    }

    fn tools_with_sandbox(&self, sandbox: Option<Arc<dyn SandboxExecutor>>) -> Vec<Box<dyn Tool>> {
        let mut tool = if self.permissive {
            ShellTool::new_permissive()
        } else {
            ShellTool::new()
        };
        if let Some(sandbox) = sandbox {
            tool = tool.with_sandbox(sandbox);
        }
        vec![Box::new(tool)]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        Some(
            "\n\n## Shell Command Capability (Shell Skill)\n\
             You can use the `shell(command)` tool to execute restricted shell commands:\n\n\
             **Safe (direct execution):**\n\
             - File viewing: `ls`, `cat`, `head`, `tail`, `wc`, `stat`\n\
             - Directory operations: `pwd`, `tree`, `find`, `du`\n\
             - Code tools: `git status/log/diff/show`, `cargo check/build/test/clippy`\n\
             - Search: `grep`, `rg` (ripgrep), `fd`\n\
             - Text processing: `echo`, `cut`, `sort`, `uniq`, `diff`\n\n\
             **Requires human approval (will show a prompt, will not execute):**\n\
             - `rm`, `mv`, `cp`, `curl`, `wget`, `npm`, `pip`, etc.\n\n\
             **Permanently forbidden (hard security policy):**\n\
             - `sudo`, `dd`, `chmod`, `reboot`, `shutdown`, etc.\n\n\
             **Note**: Only execute one command at a time; use `&&` to chain combined operations."
                .to_string(),
        )
    }
}
