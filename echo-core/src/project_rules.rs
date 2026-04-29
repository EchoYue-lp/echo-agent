//! 项目级规则文件加载
//!
//! 对标 Claude Code 的 CLAUDE.md：自动从当前目录向父级搜索
//! `.echo-agent/AGENT.md`（或 `.echo-agent/rules.md`），将其内容注入到
//! system prompt 开头。
//!
//! # 搜索逻辑
//! 1. 从 working_dir 开始搜索
//! 2. 逐级向上到文件系统根目录
//! 3. 找到第一个匹配文件后停止
//! 4. 支持嵌套：子目录的 AGENT.md 优先级更高

use std::path::{Path, PathBuf};

/// 规则文件候选名称（按优先级排序）
const RULES_FILES: &[&str] = &["AGENT.md", "RULES.md", "rules.md"];

/// 搜索目录名称
const RULES_DIR: &str = ".echo-agent";

/// 加载项目规则
///
/// 从 `working_dir` 开始向上搜索 `.echo-agent/AGENT.md`，
/// 返回找到的文件路径和内容。
pub fn load_project_rules(working_dir: &Path) -> Option<(PathBuf, String)> {
    let canonical = working_dir
        .canonicalize()
        .unwrap_or_else(|_| working_dir.to_path_buf());

    for ancestor in canonical.ancestors() {
        let rules_dir = ancestor.join(RULES_DIR);
        if !rules_dir.is_dir() {
            continue;
        }
        for filename in RULES_FILES {
            let rules_file = rules_dir.join(filename);
            match std::fs::read_to_string(&rules_file) {
                Ok(content) if !content.trim().is_empty() => {
                    return Some((rules_file, content));
                }
                _ => continue,
            }
        }
    }

    None
}

/// 生成注入到 system prompt 的规则文本
///
/// 返回带标注的规则文本块，或 `None` 表示没有找到规则文件。
pub fn rules_injection(working_dir: &Path) -> Option<String> {
    let (path, content) = load_project_rules(working_dir)?;
    let path_display = path.display();
    Some(format!(
        "<!-- PROJECT RULES: {} -->\n{}\n<!-- END PROJECT RULES -->",
        path_display, content
    ))
}

/// 将项目规则注入到现有 system prompt 的前面
pub fn inject_rules(existing_prompt: &str, working_dir: &Path) -> String {
    match rules_injection(working_dir) {
        Some(rules) => format!("{}\n\n{}", rules, existing_prompt),
        None => existing_prompt.to_string(),
    }
}
