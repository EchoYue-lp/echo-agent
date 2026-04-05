//! 受保护路径检查器
//!
//! 防止 Agent 误操作关键系统文件和目录。
//! 参考 Claude Code 的受保护路径设计：.git, .env, shell configs 等路径
//! 在任何权限模式下都受保护（包括 BypassPermissions）。

use serde_json::Value;

/// 默认受保护路径模式
const DEFAULT_PROTECTED_PATTERNS: &[&str] = &[
    ".git",
    ".env",
    ".env.",
    ".claude",
    ".claude/",
    ".vscode",
    ".idea",
    ".husky",
    ".zshrc",
    ".bashrc",
    ".bash_profile",
    ".profile",
    ".ssh",
    "authorized_keys",
    "id_rsa",
    "id_ed25519",
];

/// 受保护路径检查结果
#[derive(Debug, Clone)]
pub enum ProtectedPathResult {
    /// 路径安全
    Safe,
    /// 路径受保护
    Protected {
        /// 匹配的保护模式
        matched_pattern: String,
        /// 触发的路径
        path: String,
    },
}

/// 受保护路径检查器
#[derive(Debug, Clone)]
pub struct ProtectedPathChecker {
    /// 受保护的路径前缀模式
    patterns: Vec<String>,
}

impl ProtectedPathChecker {
    /// 创建带默认保护路径的检查器
    pub fn new() -> Self {
        Self {
            patterns: DEFAULT_PROTECTED_PATTERNS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    /// 添加自定义保护模式
    pub fn with_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.patterns.push(pattern.into());
        self
    }

    /// 批量添加自定义保护模式
    pub fn with_patterns(mut self, patterns: Vec<String>) -> Self {
        self.patterns.extend(patterns);
        self
    }

    /// 禁用所有默认保护（谨慎使用）
    pub fn disabled() -> Self {
        Self {
            patterns: Vec::new(),
        }
    }

    /// 检查工具参数中是否包含受保护路径
    ///
    /// 从 tool_input 中提取路径参数并匹配保护模式。
    pub fn check(&self, tool_name: &str, input: &Value) -> ProtectedPathResult {
        if self.patterns.is_empty() {
            return ProtectedPathResult::Safe;
        }

        // 从参数中提取路径字符串
        let paths = extract_paths(tool_name, input);

        for path in paths {
            // 规范化路径
            let normalized = path.replace('\\', "/");
            for pattern in &self.patterns {
                if Self::path_matches_pattern(pattern, &normalized) {
                    return ProtectedPathResult::Protected {
                        matched_pattern: pattern.clone(),
                        path: path.clone(),
                    };
                }
            }
        }

        ProtectedPathResult::Safe
    }

    /// 检查单个路径是否匹配保护模式
    fn path_matches_pattern(pattern: &str, path: &str) -> bool {
        // 精确文件名匹配（如 ".env"）
        if path.ends_with(pattern) {
            let suffix_start = path.len() - pattern.len();
            if suffix_start == 0 || path.as_bytes()[suffix_start - 1] == b'/' {
                return true;
            }
        }
        // 路径段匹配（如 ".git" 匹配 "/path/to/.git" 和 "/path/to/.git/config"）
        if path.contains(pattern) {
            // 检查是否是完整路径段
            let search = format!("/{pattern}");
            if path.contains(&search) || path.starts_with(pattern) {
                return true;
            }
        }
        // 前缀匹配（如 ".env." 匹配 ".env.production"）
        if pattern.ends_with('.') && path.contains(pattern) {
            return true;
        }
        false
    }
}

impl Default for ProtectedPathChecker {
    fn default() -> Self {
        Self::new()
    }
}

/// 从工具输入中提取路径字符串
fn extract_paths(tool_name: &str, input: &Value) -> Vec<String> {
    let mut paths = Vec::new();

    // 根据工具类型提取路径
    match tool_name {
        "Read" | "Write" | "Edit" => {
            if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                paths.push(path.to_string());
            }
            if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                paths.push(path.to_string());
            }
        }
        "Bash" => {
            // 从命令中提取可能涉及的路径
            if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                // 简单提取：查找以 / 或 . 开头的路径，以及可能的裸路径
                for word in cmd.split_whitespace() {
                    if word.starts_with('/')
                        || word.starts_with("./")
                        || word.starts_with('~')
                        || word.starts_with('.')
                    {
                        paths.push(word.to_string());
                    }
                }
            }
        }
        _ => {
            // 通用：提取所有可能是路径的字符串值
            extract_paths_from_value(input, &mut paths);
        }
    }

    paths
}

/// 从 JSON Value 中递归提取可能是路径的字符串
fn extract_paths_from_value(value: &Value, paths: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            // 简单启发式：包含 / 或以 . 开头的字符串可能是路径
            if s.contains('/') || s.starts_with('.') {
                paths.push(s.clone());
            }
        }
        Value::Object(map) => {
            for (key, v) in map {
                if matches!(key.as_str(), "path" | "file_path" | "directory" | "dir" | "dest" | "destination") {
                    if let Some(s) = v.as_str() {
                        paths.push(s.to_string());
                    }
                }
                extract_paths_from_value(v, paths);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                extract_paths_from_value(v, paths);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_safe_path() {
        let checker = ProtectedPathChecker::new();
        let result = checker.check("Read", &json!({"path": "/home/user/project/src/main.rs"}));
        assert!(matches!(result, ProtectedPathResult::Safe));
    }

    #[test]
    fn test_git_protected() {
        let checker = ProtectedPathChecker::new();
        let result = checker.check("Write", &json!({"path": "/project/.git/config"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }

    #[test]
    fn test_env_protected() {
        let checker = ProtectedPathChecker::new();
        let result = checker.check("Write", &json!({"path": ".env"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }

    #[test]
    fn test_env_production_protected() {
        let checker = ProtectedPathChecker::new();
        let result = checker.check("Write", &json!({"path": ".env.production"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }

    #[test]
    fn test_ssh_protected() {
        let checker = ProtectedPathChecker::new();
        let result = checker.check("Read", &json!({"path": "/home/user/.ssh/id_rsa"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }

    #[test]
    fn test_bash_with_protected_path() {
        let checker = ProtectedPathChecker::new();
        let result = checker.check("Bash", &json!({"command": "rm -rf .git"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }

    #[test]
    fn test_disabled_checker() {
        let checker = ProtectedPathChecker::disabled();
        let result = checker.check("Write", &json!({"path": ".env"}));
        assert!(matches!(result, ProtectedPathResult::Safe));
    }

    #[test]
    fn test_custom_pattern() {
        let checker = ProtectedPathChecker::new().with_pattern("secret/");
        let result = checker.check("Write", &json!({"path": "/project/secret/keys.pem"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }
}
