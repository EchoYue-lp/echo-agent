//! Shell 命令执行工具
//!
//! ⚠️ 安全策略：仅允许白名单中的安全命令执行

use super::{Tool, ToolParameters, ToolResult};
use crate::error::{Result, ToolError};
use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::LazyLock;
use tokio::process::Command;

static ALLOWED_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // ===== 文件查看 =====
        "ls", "cat", "head", "tail", "less", "more", "file", "stat", "wc",
        // ===== 目录操作（只读）=====
        "pwd", "tree", "find", "du", // ===== 代码相关 =====
        "git", "cargo", "rustc", "clippy", "rustfmt", // ===== 搜索与查找 =====
        "grep", "rg", "ag", "fd", // ===== 文本处理（只读）=====
        "echo", "printf", "sed", "awk", "cut", "sort", "uniq", "diff",
        // ===== 系统信息（只读）=====
        "which", "whereis", "env", "date", "uname",
    ])
});

static REQUIRE_APPROVAL_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // ===== 文件删除/修改（需要确认）=====
        "rm", "rmdir", "mv", "cp", // ===== 网络操作（需要确认）=====
        "curl", "wget", "nc", // ===== 进程操作（需要确认）=====
        "kill", "killall", "pkill", // ===== 包管理（需要确认）=====
        "apt", "apt-get", "yum", "dnf", "brew", "pip", "pip3", "npm", "yarn", "pnpm",
        // ===== 脚本执行（需要确认）=====
        "bash", "sh", "zsh", "fish", "python", "python3", "node", "perl", "ruby", "php",
    ])
});

static DANGEROUS_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // ===== 极度危险（数据破坏）=====
        "dd", "shred", "mkfs", "fdisk", // ===== 权限提升 =====
        "sudo", "su", // ===== 权限修改 =====
        "chmod", "chown", "chgrp", // ===== 系统操作 =====
        "reboot", "shutdown", "halt", "poweroff", "init",
        // ===== 高危网络操作 =====
        "nmap",
    ])
});

static GIT_SAFE_SUBCOMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // 只读操作
        "status", "log", "show", "diff", "branch", "tag", "ls-files", "ls-tree", "remote", "config",
        // 需要人工确认的修改操作
        "add", "commit", "checkout", "switch", "stash",
    ])
});

static CARGO_SAFE_SUBCOMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // 只读/构建操作
        "check", "build", "test", "clippy", "fmt", "tree", "search", "metadata",
        // 需要确认的操作
        "clean", "update",
    ])
});

/// 命令安全性检查结果
#[derive(Debug, Clone, PartialEq)]
pub enum CommandSafety {
    /// 安全，可以执行
    Safe,
    /// 需要额外确认
    RequiresApproval(String),
    /// 危险，拒绝执行
    Dangerous(String),
}

/// Shell 命令执行工具（带安全检查）
pub struct ShellTool {
    /// 是否启用严格模式（默认 true）
    strict_mode: bool,
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellTool {
    /// 创建新的 Shell 工具（默认严格模式）
    pub fn new() -> Self {
        Self { strict_mode: true }
    }

    /// 创建非严格模式的 Shell 工具（不推荐！）
    pub fn new_permissive() -> Self {
        Self { strict_mode: false }
    }

    /// 检查命令是否安全
    pub fn check_command_safety(&self, command: &str) -> CommandSafety {
        let parts: Vec<&str> = command.split_whitespace().collect();
        if parts.is_empty() {
            return CommandSafety::Dangerous("空命令".to_string());
        }

        let base_cmd = parts[0];

        // 1. 检查是否在危险命令黑名单中（明确拒绝）
        if DANGEROUS_COMMANDS.contains(base_cmd) {
            return CommandSafety::Dangerous(format!(
                "命令 '{}' 在危险命令黑名单中，已拒绝执行",
                base_cmd
            ));
        }

        // 2. 检查是否需要人工确认
        if REQUIRE_APPROVAL_COMMANDS.contains(base_cmd) {
            return CommandSafety::RequiresApproval(format!(
                "命令 '{}' 可能造成系统变更，需要人工确认",
                base_cmd
            ));
        }

        // 3. 严格模式：必须在白名单中
        if self.strict_mode && !ALLOWED_COMMANDS.contains(base_cmd) {
            return CommandSafety::Dangerous(format!(
                "命令 '{}' 不在安全白名单中，已拒绝执行",
                base_cmd
            ));
        }

        // 4. 特殊命令的子命令检查
        match base_cmd {
            "git" => self.check_git_command(&parts),
            "cargo" => self.check_cargo_command(&parts),
            "sed" | "awk" => {
                // 文本处理命令可能包含危险操作
                CommandSafety::RequiresApproval(format!(
                    "'{}' 命令可能修改文件，需要确认",
                    base_cmd
                ))
            }
            _ => CommandSafety::Safe,
        }
    }

    /// 检查 git 子命令
    fn check_git_command(&self, parts: &[&str]) -> CommandSafety {
        if parts.len() < 2 {
            return CommandSafety::Safe;
        }

        let subcommand = parts[1];

        // 检查 git 操作
        match subcommand {
            // 网络操作（需要确认）
            "push" | "pull" | "fetch" | "clone" => CommandSafety::RequiresApproval(format!(
                "git {} 涉及网络操作，需要确认",
                subcommand
            )),
            // 强制重置（危险，拒绝）
            "reset" => {
                if parts.contains(&"--hard") {
                    CommandSafety::Dangerous(
                        "git reset --hard 会丢失数据，已拒绝。如需执行请手动操作".to_string(),
                    )
                } else {
                    CommandSafety::RequiresApproval(
                        "git reset 会修改 Git 状态，需要确认".to_string(),
                    )
                }
            }
            // 清理未跟踪文件（需要确认）
            "clean" => {
                CommandSafety::RequiresApproval("git clean 会删除未跟踪文件，需要确认".to_string())
            }
            // 安全的子命令
            cmd if GIT_SAFE_SUBCOMMANDS.contains(cmd) => {
                if cmd == "commit" || cmd == "add" || cmd == "checkout" {
                    CommandSafety::RequiresApproval(format!("git {} 会修改仓库，需要确认", cmd))
                } else {
                    CommandSafety::Safe
                }
            }
            // 未知子命令（需要确认）
            _ => CommandSafety::RequiresApproval(format!(
                "git {} 不在已知安全列表中，需要确认",
                subcommand
            )),
        }
    }

    /// 检查 cargo 子命令
    fn check_cargo_command(&self, parts: &[&str]) -> CommandSafety {
        if parts.len() < 2 {
            return CommandSafety::Safe;
        }

        let subcommand = parts[1];

        match subcommand {
            // 包安装/发布（需要确认）
            "install" | "uninstall" | "publish" => CommandSafety::RequiresApproval(format!(
                "cargo {} 涉及包安装/发布，需要确认",
                subcommand
            )),
            // 运行程序（需要确认）
            "run" => CommandSafety::RequiresApproval("cargo run 会执行程序，需要确认".to_string()),
            // 已知安全命令
            cmd if CARGO_SAFE_SUBCOMMANDS.contains(cmd) => {
                if cmd == "clean" || cmd == "update" {
                    CommandSafety::RequiresApproval(format!("cargo {} 会修改项目，需要确认", cmd))
                } else {
                    CommandSafety::Safe
                }
            }
            // 未知子命令（需要确认）
            _ => CommandSafety::RequiresApproval(format!(
                "cargo {} 不在已知安全列表中，需要确认",
                subcommand
            )),
        }
    }
}

impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "执行受限的 shell 命令（仅允许安全的只读操作和代码相关命令）。参数：command - 要执行的命令"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "要执行的 shell 命令（仅限白名单中的安全命令）"
                }
            },
            "required": ["command"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let command = parameters
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("command".to_string()))?;

            match self.check_command_safety(command) {
                CommandSafety::Safe => {}
                CommandSafety::RequiresApproval(reason) => {
                    return Ok(ToolResult::error(format!(
                        "⚠️  需要人工确认：{}\n命令：{}\n\n请使用 human_loop 模块进行确认后再执行。",
                        reason, command
                    )));
                }
                CommandSafety::Dangerous(reason) => {
                    return Ok(ToolResult::error(format!(
                        "🚫 安全拒绝：{}\n命令：{}\n\n如需执行此类操作，请手动在终端中执行。",
                        reason, command
                    )));
                }
            }

            #[cfg(target_os = "windows")]
            let (shell, shell_arg) = ("cmd", "/C");
            #[cfg(not(target_os = "windows"))]
            let (shell, shell_arg) = ("sh", "-c");

            match Command::new(shell)
                .arg(shell_arg)
                .arg(command)
                .output()
                .await
            {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                    if output.status.success() {
                        Ok(ToolResult::success(stdout))
                    } else {
                        Ok(ToolResult::error(format!(
                            "命令执行失败，退出码: {:?}\n标准输出: {}\n错误输出: {}",
                            output.status.code(),
                            stdout,
                            stderr
                        )))
                    }
                }
                Err(e) => Ok(ToolResult::error(format!("无法执行命令: {}", e))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_safe_commands() {
        let tool = ShellTool::new();

        // 安全命令
        assert_eq!(tool.check_command_safety("ls -la"), CommandSafety::Safe);
        assert_eq!(tool.check_command_safety("pwd"), CommandSafety::Safe);
        assert_eq!(
            tool.check_command_safety("cat README.md"),
            CommandSafety::Safe
        );
        assert_eq!(tool.check_command_safety("git status"), CommandSafety::Safe);
        assert_eq!(
            tool.check_command_safety("cargo check"),
            CommandSafety::Safe
        );
    }

    #[test]
    fn test_require_approval_commands() {
        let tool = ShellTool::new();

        // 需要确认的命令
        match tool.check_command_safety("rm -rf /tmp/test") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("rm 命令应该需要确认"),
        }

        match tool.check_command_safety("curl http://example.com") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("curl 命令应该需要确认"),
        }

        match tool.check_command_safety("npm install package") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("npm 命令应该需要确认"),
        }

        match tool.check_command_safety("python script.py") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("python 命令应该需要确认"),
        }
    }

    #[test]
    fn test_dangerous_commands() {
        let tool = ShellTool::new();

        // 极度危险的命令（明确拒绝）
        match tool.check_command_safety("dd if=/dev/zero of=/dev/sda") {
            CommandSafety::Dangerous(_) => {}
            _ => panic!("dd 命令应该被拒绝"),
        }

        match tool.check_command_safety("sudo apt install") {
            CommandSafety::Dangerous(_) => {}
            _ => panic!("sudo 命令应该被拒绝"),
        }

        match tool.check_command_safety("chmod 777 /etc/passwd") {
            CommandSafety::Dangerous(_) => {}
            _ => panic!("chmod 命令应该被拒绝"),
        }

        match tool.check_command_safety("reboot") {
            CommandSafety::Dangerous(_) => {}
            _ => panic!("reboot 命令应该被拒绝"),
        }
    }

    #[test]
    fn test_git_commands() {
        let tool = ShellTool::new();

        // Git 安全命令
        assert_eq!(tool.check_command_safety("git log"), CommandSafety::Safe);
        assert_eq!(tool.check_command_safety("git diff"), CommandSafety::Safe);
        assert_eq!(tool.check_command_safety("git status"), CommandSafety::Safe);

        // Git 需要确认的命令
        match tool.check_command_safety("git commit -m 'test'") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("git commit 应该需要确认"),
        }

        match tool.check_command_safety("git push origin main") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("git push 应该需要确认"),
        }

        match tool.check_command_safety("git add .") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("git add 应该需要确认"),
        }

        match tool.check_command_safety("git clean -fd") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("git clean 应该需要确认"),
        }

        // Git 危险命令
        match tool.check_command_safety("git reset --hard HEAD~1") {
            CommandSafety::Dangerous(_) => {}
            _ => panic!("git reset --hard 应该被拒绝"),
        }
    }

    #[test]
    fn test_cargo_commands() {
        let tool = ShellTool::new();

        // Cargo 安全命令
        assert_eq!(
            tool.check_command_safety("cargo check"),
            CommandSafety::Safe
        );
        assert_eq!(tool.check_command_safety("cargo test"), CommandSafety::Safe);
        assert_eq!(
            tool.check_command_safety("cargo clippy"),
            CommandSafety::Safe
        );
        assert_eq!(
            tool.check_command_safety("cargo build"),
            CommandSafety::Safe
        );

        // Cargo 需要确认的命令
        match tool.check_command_safety("cargo run") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("cargo run 应该需要确认"),
        }

        match tool.check_command_safety("cargo install some-package") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("cargo install 应该需要确认"),
        }

        match tool.check_command_safety("cargo clean") {
            CommandSafety::RequiresApproval(_) => {}
            _ => panic!("cargo clean 应该需要确认"),
        }
    }

    #[test]
    fn test_unknown_command_in_strict_mode() {
        let tool = ShellTool::new(); // 默认严格模式

        match tool.check_command_safety("unknown_command") {
            CommandSafety::Dangerous(_) => {}
            _ => panic!("严格模式下应该拒绝未知命令"),
        }
    }

    #[tokio::test]
    async fn test_shell_tool_execution() {
        let tool = ShellTool::new();

        // 测试安全命令
        let mut params = HashMap::new();
        params.insert("command".to_string(), serde_json::json!("echo hello"));
        let result = tool.execute(params).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello"));

        // 测试需要确认的命令
        let mut params = HashMap::new();
        params.insert("command".to_string(), serde_json::json!("rm test.txt"));
        let result = tool.execute(params).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("确认"));

        // 测试危险命令
        let mut params = HashMap::new();
        params.insert("command".to_string(), serde_json::json!("sudo reboot"));
        let result = tool.execute(params).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("拒绝"));
    }
}
