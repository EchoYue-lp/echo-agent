//! Contracts for repository documentation entry points and local links.

use regex::Regex;
use std::path::{Path, PathBuf};

fn collect_markdown_files(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let ignored = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| matches!(name, ".git" | ".worktrees" | "target"));
            if !ignored {
                collect_markdown_files(&path, files)?;
            }
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("md") {
            files.push(path);
        }
    }
    Ok(())
}

fn local_link_target(raw: &str) -> Option<&str> {
    let raw = raw.trim();
    if raw.starts_with('#')
        || raw.starts_with("mailto:")
        || raw.starts_with("data:")
        || raw.contains("://")
    {
        return None;
    }
    let target = raw
        .strip_prefix('<')
        .and_then(|value| value.split_once('>').map(|(path, _)| path))
        .unwrap_or_else(|| raw.split_whitespace().next().unwrap_or(raw));
    let target = target.split(['#', '?']).next().unwrap_or_default().trim();
    (!target.is_empty() && !Path::new(target).is_absolute()).then_some(target)
}

#[test]
fn repository_markdown_has_resolvable_local_links() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let link_pattern = Regex::new(r#"!?\[[^\]]*\]\(([^)]+)\)"#)?;
    let reference_pattern = Regex::new(r#"^\s*\[[^\]]+\]:\s*(\S+)"#)?;
    let example_pattern = Regex::new(r#"`(examples/[^`]+\.rs)`|`(demo[^`]+\.rs)`"#)?;
    let mut files = Vec::new();
    collect_markdown_files(root, &mut files)?;
    let mut broken = Vec::new();

    for source in files {
        let content = std::fs::read_to_string(&source)?;
        let Some(parent) = source.parent() else {
            continue;
        };
        let relative = source
            .strip_prefix(root)
            .unwrap_or(source.as_path())
            .display();

        for (line_index, line) in content.lines().enumerate() {
            let inline_targets = link_pattern
                .captures_iter(line)
                .filter_map(|captures| captures.get(1).map(|value| value.as_str()));
            let reference_targets = reference_pattern
                .captures(line)
                .and_then(|captures| captures.get(1).map(|value| value.as_str()))
                .into_iter();
            for raw in inline_targets.chain(reference_targets) {
                let Some(target) = local_link_target(raw) else {
                    continue;
                };
                if !parent.join(target).exists() {
                    broken.push(format!("{relative}:{} -> {target}", line_index + 1));
                }
            }

            for captures in example_pattern.captures_iter(line) {
                let target = captures
                    .get(1)
                    .or_else(|| captures.get(2))
                    .map(|value| value.as_str());
                let Some(target) = target else {
                    continue;
                };
                if target.contains('{') || target.contains('}') {
                    continue;
                }
                let target = target.strip_prefix("examples/").unwrap_or(target);
                if !root.join("examples").join(target).exists() {
                    broken.push(format!("{relative}:{} -> {target}", line_index + 1));
                }
            }
        }
    }

    if broken.is_empty() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "broken local documentation links:\n{}",
            broken.join("\n")
        ))
        .into())
    }
}

#[test]
fn framework_docs_do_not_publish_stale_eko_paths() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for relative in ["docs/en/07-skills.md", "docs/zh/07-skills.md"] {
        let content = std::fs::read_to_string(root.join(relative))?;
        assert!(!content.contains("~/.echo-agent/skills/"));
        assert!(!content.contains("echo-agent-cli/docs/system-deep-dive"));
        assert!(content.contains("~/.eko/skills/"));
    }

    for relative in [
        "docs/en/25-self-improvement.md",
        "docs/zh/25-self-improvement.md",
    ] {
        let content = std::fs::read_to_string(root.join(relative))?;
        assert!(!content.contains(".echo-agent/AGENTS.md"));
        assert!(content.contains(".eko/learned-rules.md"));
        assert!(content.contains(".eko/skills/_drafts/"));
    }
    Ok(())
}
