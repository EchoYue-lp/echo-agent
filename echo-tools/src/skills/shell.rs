use std::sync::Arc;

use echo_core::sandbox::SandboxExecutor;
use echo_core::tools::Tool;
use echo_core::tools::skill::Skill;

use crate::shell::ShellTool;

/// Controlled local command execution skill.
pub struct ShellSkill {
    permissive: bool,
}

impl ShellSkill {
    pub fn new() -> Self {
        Self { permissive: false }
    }

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
        "Controlled local command execution"
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
        Some("\n\n## Shell Capability\nUse the `shell` tool for local commands. The installed command policy decides whether a command is allowed, needs approval, or is denied.".to_string())
    }
}
