//! 受保护路径检查器
//!
//! 防止 Agent 误操作关键系统文件和目录。
//! 参考 Claude Code 的受保护路径设计：.git, .env, shell configs 等路径
//! 在任何权限模式下都受保护（包括 BypassPermissions）。

use regex::Regex;
use serde_json::Value;
use std::env;

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
        // macOS 文件系统不区分大小写，需要 case-insensitive 匹配
        let p_lower = path.to_lowercase();
        let pat_lower = pattern.to_lowercase();

        // 精确文件名匹配（如 ".env" / ".ENV"）
        if p_lower.ends_with(&pat_lower) {
            let suffix_start = p_lower.len() - pat_lower.len();
            if suffix_start == 0 || p_lower.as_bytes()[suffix_start - 1] == b'/' {
                return true;
            }
        }
        // 路径段匹配（如 ".git" 匹配 "/path/to/.git" 和 "/path/to/.git/config"）
        if p_lower.contains(&pat_lower) {
            // 检查是否是完整路径段
            let search = format!("/{pat_lower}");
            if p_lower.contains(&search) || p_lower.starts_with(&pat_lower) {
                return true;
            }
        }
        // 前缀匹配（如 ".env." 匹配 ".env.production"）
        if pattern.ends_with('.') && p_lower.contains(&pat_lower) {
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
                paths.extend(extract_paths_from_bash_command(cmd));
            }
        }
        _ => {
            // 通用：提取所有可能是路径的字符串值
            extract_paths_from_value(input, &mut paths);
        }
    }

    paths
}

/// 从 Bash 命令中提取路径，处理环境变量、命令替换等
fn extract_paths_from_bash_command(cmd: &str) -> Vec<String> {
    let mut paths = Vec::new();

    // 首先，展开环境变量和波浪号
    let expanded = expand_env_vars_and_tilde(cmd);

    // 提取引号内的字符串（单引号和双引号），它们可能包含路径
    let quoted_re = Regex::new(r#""([^"]*)"|'([^']*)'"#).unwrap();
    for cap in quoted_re.captures_iter(&expanded) {
        if let Some(quoted) = cap.get(1).or_else(|| cap.get(2)) {
            let quoted_str = quoted.as_str();
            // 如果引号内的字符串看起来像路径，添加到列表
            if looks_like_path(quoted_str) {
                paths.push(quoted_str.to_string());
            }
        }
    }

    // 使用正则表达式提取所有看起来像路径的字符串
    // 匹配：以 /, ./, ~/, . 开头的字符串，或者包含 / 的字符串
    // 排除明显的非路径 token（如命令选项）
    let path_re =
        Regex::new(r#"(?:^|\s)((?:/|\./|~/|\.)[^\s"'`$(){}|;&<>]*|/[^\s"'`$(){}|;&<>]+)"#).unwrap();

    for cap in path_re.captures_iter(&expanded) {
        if let Some(matched) = cap.get(1) {
            let path = matched.as_str().trim();
            // 过滤掉明显的非路径（如单个点、双点）和已处理的引号内容
            if !path.is_empty() && path != "." && path != ".." && !paths.contains(&path.to_string())
            {
                paths.push(path.to_string());
            }
        }
    }

    // 另外，专门处理命令替换中的路径：$(echo /.git) 或 `echo /.git`
    // 使用 expanded 字符串，因为命令替换可能包含需要展开的环境变量
    extract_paths_from_command_substitution(&expanded, &mut paths);

    paths
}

/// 检查字符串是否看起来像路径
fn looks_like_path(s: &str) -> bool {
    s.contains('/') || s.starts_with('.') || s.starts_with('~') || s.starts_with('/')
}

/// 展开环境变量 ($VAR, ${VAR}) 和波浪号 (~)
fn expand_env_vars_and_tilde(cmd: &str) -> String {
    let mut result = cmd.to_string();

    // 展开波浪号 ~（仅单独的 ~ 或 ~/）
    if result.contains('~')
        && let Ok(home) = env::var("HOME")
    {
        // 替换 ~/ 但不替换 ~username（更复杂，需要 getpwnam）
        result = result.replace("~/", &format!("{}/", home));
        // 单独的 ~ 后面没有 /，通常表示家目录
        result = result.replace(" ~ ", &format!(" {} ", home));
        if result == "~" {
            result = home;
        }
    }

    // 展开环境变量 $VAR 和 ${VAR}，使用正则确保完整匹配
    let re = Regex::new(r#"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)"#).unwrap();

    result = re
        .replace_all(&result, |caps: &regex::Captures| {
            let var_name = caps.get(1).or_else(|| caps.get(2)).unwrap().as_str();
            env::var(var_name).unwrap_or_else(|_| caps[0].to_string())
        })
        .to_string();

    result
}

/// 从命令替换中提取路径：$(command) 或 `command`
fn extract_paths_from_command_substitution(cmd: &str, paths: &mut Vec<String>) {
    // 使用栈来避免无限递归，最大深度为5
    let mut stack = vec![(cmd.to_string(), 0)];
    let re = Regex::new(r#"\$\(([^)]+)\)|`([^`]+)`"#).unwrap();

    while let Some((current_cmd, depth)) = stack.pop() {
        if depth >= 5 {
            continue; // 防止无限递归
        }

        for cap in re.captures_iter(&current_cmd) {
            if let Some(subcmd) = cap.get(1).or_else(|| cap.get(2)) {
                let subcmd_str = subcmd.as_str();
                // 展开子命令中的环境变量
                let expanded_subcmd = expand_env_vars_and_tilde(subcmd_str);
                // 从命令替换中提取路径（不递归，避免重复处理）
                let sub_paths = extract_direct_paths_from_command(&expanded_subcmd);
                paths.extend(sub_paths);

                // 将命令替换内容推入栈进行进一步处理（如果有嵌套）
                // 使用展开后的字符串以便进一步处理环境变量
                stack.push((expanded_subcmd, depth + 1));
            }
        }
    }
}

/// 从命令字符串中直接提取路径，不递归处理命令替换
fn extract_direct_paths_from_command(cmd: &str) -> Vec<String> {
    let mut paths = Vec::new();

    // 简单分词，按空白分割
    for word in cmd.split_whitespace() {
        // 移除周围的引号
        let word = word.trim_matches(|c| c == '"' || c == '\'');
        if looks_like_path(word) {
            paths.push(word.to_string());
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
                if matches!(
                    key.as_str(),
                    "path" | "file_path" | "directory" | "dir" | "dest" | "destination"
                ) && let Some(s) = v.as_str()
                {
                    paths.push(s.to_string());
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

    #[test]
    fn test_case_insensitive() {
        let checker = ProtectedPathChecker::new();
        // macOS 上 .ENV 应匹配 .env
        let result = checker.check("Write", &json!({"path": "/project/.ENV"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));

        let result = checker.check("Write", &json!({"path": "/project/.Git/config"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }

    #[test]
    fn test_command_substitution_bypass() {
        let checker = ProtectedPathChecker::new();
        // bash -c "rm $(echo /.git)" 应该被检测到
        let result = checker.check("Bash", &json!({"command": "rm $(echo /.git)"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }

    #[test]
    fn test_backtick_command_substitution() {
        let checker = ProtectedPathChecker::new();
        // rm `echo /.git` 应该被检测到
        let result = checker.check("Bash", &json!({"command": "rm `echo /.git`"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }

    #[test]
    fn test_nested_command_substitution() {
        let checker = ProtectedPathChecker::new();
        // bash -c "rm $(echo $(echo /.git))" 嵌套命令替换
        let result = checker.check("Bash", &json!({"command": "rm $(echo $(echo /.git))"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }

    #[test]
    fn test_quoted_command_substitution() {
        let checker = ProtectedPathChecker::new();
        // rm "$(echo '/.git')" 引号内的命令替换
        let result = checker.check("Bash", &json!({"command": "rm \"$(echo '/.git')\""}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }

    #[test]
    fn test_env_var_in_command_substitution() {
        // 在测试中设置环境变量需要 unsafe 块
        unsafe {
            std::env::set_var("TEST_PATH", "/.git");
        }
        let checker = ProtectedPathChecker::new();
        // rm $(echo $TEST_PATH) 环境变量在命令替换中
        let result = checker.check("Bash", &json!({"command": "rm $(echo $TEST_PATH)"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }

    #[test]
    fn test_home_dir_expansion() {
        let checker = ProtectedPathChecker::new();
        // rm ~/.ssh/id_rsa 波浪号展开
        let result = checker.check("Bash", &json!({"command": "rm ~/.ssh/id_rsa"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }

    #[test]
    fn test_env_var_expansion() {
        // 在测试中设置环境变量需要 unsafe 块
        unsafe {
            std::env::set_var("HOME", "/home/user");
        }
        let checker = ProtectedPathChecker::new();
        // rm $HOME/.ssh/id_rsa 环境变量展开
        let result = checker.check("Bash", &json!({"command": "rm $HOME/.ssh/id_rsa"}));
        assert!(matches!(result, ProtectedPathResult::Protected { .. }));
    }
}
