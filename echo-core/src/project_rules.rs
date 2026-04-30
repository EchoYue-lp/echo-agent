//! Project-level rules file loading
//!
//! Equivalent to Claude Code's CLAUDE.md: automatically searches from the current
//! directory upward for `.echo-agent/AGENT.md` (or `.echo-agent/rules.md`),
//! injecting its content at the beginning of the system prompt.
//!
//! # Search Logic
//! 1. Start searching from working_dir
//! 2. Walk up to the filesystem root
//! 3. Stop at the first matching file
//! 4. Supports nesting: AGENT.md in subdirectories takes higher priority

use std::path::{Path, PathBuf};

/// Rule file candidate names (sorted by priority)
const RULES_FILES: &[&str] = &["AGENT.md", "RULES.md", "rules.md"];

/// Search directory name
const RULES_DIR: &str = ".echo-agent";

/// Load project rules
///
/// Searches upward from `working_dir` for `.echo-agent/AGENT.md`,
/// returning the file path and content if found.
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

/// Generate rule text to inject into the system prompt
///
/// Returns an annotated rule text block, or `None` if no rule file was found.
pub fn rules_injection(working_dir: &Path) -> Option<String> {
    let (path, content) = load_project_rules(working_dir)?;
    let path_display = path.display();
    Some(format!(
        "<!-- PROJECT RULES: {} -->\n{}\n<!-- END PROJECT RULES -->",
        path_display, content
    ))
}

/// Inject project rules before the existing system prompt
pub fn inject_rules(existing_prompt: &str, working_dir: &Path) -> String {
    match rules_injection(working_dir) {
        Some(rules) => format!("{}\n\n{}", rules, existing_prompt),
        None => existing_prompt.to_string(),
    }
}
