//! Contracts for the learning package's docs, examples, and public facade.

use regex::Regex;
use std::collections::BTreeMap;
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

fn demo_sources() -> Result<BTreeMap<String, PathBuf>, Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = BTreeMap::new();
    for directory in [root.join("examples"), root.join("tests/example_contracts")] {
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            let Some(name) = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if path.extension().and_then(|extension| extension.to_str()) == Some("rs")
                && name.starts_with("demo")
                && sources.insert(name.clone(), path).is_some()
            {
                return Err(std::io::Error::other(format!("duplicate demo source: {name}")).into());
            }
        }
    }
    Ok(sources)
}

#[test]
fn learning_markdown_has_resolvable_local_links() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let link_pattern = Regex::new(r#"!?\[[^\]]*\]\(([^)]+)\)"#)?;
    let reference_pattern = Regex::new(r#"^\s*\[[^\]]+\]:\s*(\S+)"#)?;
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
        }
    }

    if broken.is_empty() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "broken learning documentation links:\n{}",
            broken.join("\n")
        ))
        .into())
    }
}

#[test]
fn example_manifest_lists_every_demo_once() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = std::fs::read_to_string(root.join("examples/README.md"))?;
    let sources = demo_sources()?;
    assert_eq!(sources.len(), 45 + 21, "unexpected demo source count");
    for name in sources.keys() {
        let listed = readme
            .lines()
            .filter(|line| line.trim_start().starts_with("- `") && line.contains(name))
            .count();
        assert_eq!(
            listed, 1,
            "demo must be listed exactly once in examples README: {name}"
        );
    }
    Ok(())
}

#[test]
fn contract_harness_lists_every_deterministic_demo() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let harness = std::fs::read_to_string(root.join("tests/example_contracts.rs"))?;
    for entry in std::fs::read_dir(root.join("tests/example_contracts"))? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.starts_with("demo") && name.ends_with(".rs") {
            assert!(
                harness.contains(name),
                "contract harness does not include {name}"
            );
        }
    }
    Ok(())
}

#[test]
fn demos_use_only_the_public_facade_and_safe_string_access()
-> Result<(), Box<dyn std::error::Error>> {
    let byte_slice = Regex::new(r#"\[[^\]\n]*\.\.[^\]\n]*\]"#)?;
    let split_crate = Regex::new(
        r#"\b(?:echo_core|echo_execution|echo_integration|echo_macros|echo_orchestration|echo_state|echo_tools)::"#,
    )?;
    let json_index = Regex::new(r#"\b(?:json|value|response|resp|summary|item\.value)\s*\["#)?;
    let text_byte_count = Regex::new(r#"\b(?:content|text|output|code|word|prompt|id)\.len\(\)"#)?;
    let deprecated_execution_role_term = Regex::new(r#"(?i)\bworkers?\b"#)?;
    let mut violations = Vec::new();

    for (name, path) in demo_sources()? {
        let source = std::fs::read_to_string(path)?;
        for (line_index, line) in source.lines().enumerate() {
            for token in [
                ".unwrap()",
                ".unwrap_err()",
                ".expect(",
                ".expect_err(",
                "panic!(",
                "unreachable!(",
                "todo!(",
            ] {
                if line.contains(token) {
                    violations.push(format!("{name}:{} uses {token}", line_index + 1));
                }
            }
            if byte_slice.is_match(line) {
                violations.push(format!(
                    "{name}:{} uses unchecked range slicing",
                    line_index + 1
                ));
            }
            if split_crate.is_match(line) {
                violations.push(format!(
                    "{name}:{} bypasses the echo_agent facade",
                    line_index + 1
                ));
            }
            if json_index.is_match(line) {
                violations.push(format!(
                    "{name}:{} directly indexes structured JSON",
                    line_index + 1
                ));
            }
            if text_byte_count.is_match(line) {
                violations.push(format!(
                    "{name}:{} counts user-visible text as bytes",
                    line_index + 1
                ));
            }
            if deprecated_execution_role_term.is_match(line) {
                violations.push(format!(
                    "{name}:{} uses retired execution-role terminology",
                    line_index + 1
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "learning demo contract violations:\n{}",
        violations.join("\n")
    );
    Ok(())
}

#[test]
fn package_identity_is_consolidated() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml"))?;
    assert!(manifest.contains("name = \"echo-agent-learning\""));
    assert!(!manifest.contains("echo-agent-examples"));
    assert!(!manifest.contains("echo-rust-learning"));
    assert!(root.join("docs/zh/README.md").is_file());
    assert!(root.join("examples/README.md").is_file());
    Ok(())
}
