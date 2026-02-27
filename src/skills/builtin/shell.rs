use crate::skills::Skill;
use crate::tools::Tool;
use crate::tools::shell::ShellTool;

/// Shell 命令执行技能
///
/// 为 Agent 提供受控的 shell 命令执行能力，内置三级安全策略：
/// - **Safe**：直接执行（ls / cat / git status / cargo check 等只读命令）
/// - **RequiresApproval**：拒绝并提示需要人工确认（rm / curl / npm 等）
/// - **Dangerous**：明确拒绝（sudo / dd / shutdown 等）
///
/// # 使用方式
/// ```rust
/// // 严格模式（默认）：只允许白名单命令
/// agent.add_skill(Box::new(ShellSkill::new()));
///
/// // 宽松模式：白名单之外的命令也可执行（不推荐用于生产）
/// agent.add_skill(Box::new(ShellSkill::permissive()));
/// ```
pub struct ShellSkill {
    permissive: bool,
}

impl ShellSkill {
    /// 创建严格模式（默认）：只允许白名单内的命令
    pub fn new() -> Self {
        Self { permissive: false }
    }

    /// 创建宽松模式：白名单之外的未知命令也可执行
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
        "受控 shell 命令执行能力：支持文件查看、目录操作、代码构建（git/cargo）、搜索等安全命令"
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        let tool = if self.permissive {
            ShellTool::new_permissive()
        } else {
            ShellTool::new()
        };
        vec![Box::new(tool)]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        Some(
            "\n\n## Shell 命令能力（Shell Skill）\n\
             你可以使用 `shell(command)` 工具执行受限的 shell 命令：\n\n\
             **安全（直接执行）：**\n\
             - 文件查看：`ls`、`cat`、`head`、`tail`、`wc`、`stat`\n\
             - 目录操作：`pwd`、`tree`、`find`、`du`\n\
             - 代码工具：`git status/log/diff/show`、`cargo check/build/test/clippy`\n\
             - 搜索：`grep`、`rg`（ripgrep）、`fd`\n\
             - 文本处理：`echo`、`cut`、`sort`、`uniq`、`diff`\n\n\
             **需要人工确认（会返回提示，不会执行）：**\n\
             - `rm`、`mv`、`cp`、`curl`、`wget`、`npm`、`pip` 等\n\n\
             **永久禁止（安全策略硬限制）：**\n\
             - `sudo`、`dd`、`chmod`、`reboot`、`shutdown` 等\n\n\
             **注意**：每次只执行一条命令；如需组合操作请用 `&&` 连接。"
                .to_string(),
        )
    }
}
