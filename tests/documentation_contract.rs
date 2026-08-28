//! Contracts for repository documentation entry points and local links.

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
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
    let example_pattern =
        Regex::new(r#"`((?:examples|tests/example_contracts)/[^`]+\.rs)`|`(demo[^`]+\.rs)`"#)?;
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
                let resolved = if target.contains('/') {
                    root.join(target)
                } else {
                    root.join("examples").join(target)
                };
                if !resolved.exists() {
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
fn framework_docs_do_not_publish_product_paths() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for relative in ["docs/en/07-skills.md", "docs/zh/07-skills.md"] {
        let content = std::fs::read_to_string(root.join(relative))?;
        assert!(!content.contains("~/.echo-agent/skills/"));
        assert!(!content.contains("echo-agent-cli/docs/system-deep-dive"));
        assert!(!content.contains("~/.eko/skills/"));
        assert!(content.contains("<application-data>/skills/"));
    }

    for relative in [
        "docs/en/25-self-improvement.md",
        "docs/zh/25-self-improvement.md",
    ] {
        let content = std::fs::read_to_string(root.join(relative))?;
        assert!(!content.contains(".echo-agent/AGENTS.md"));
        assert!(!content.contains(".eko/learned-rules.md"));
        assert!(!content.contains(".eko/skills/_drafts/"));
        assert!(content.contains("<application-data>/learned-rules.md"));
        assert!(content.contains("<application-data>/skills/_drafts/"));
    }
    Ok(())
}

#[test]
fn facade_consumer_guides_do_not_depend_on_split_crates() -> Result<(), Box<dyn std::error::Error>>
{
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let guides = [
        ("docs/en/15-im-channels.md", "channels"),
        ("docs/zh/15-im-channels.md", "channels"),
        ("docs/en/34-git-isolation.md", "git"),
        ("docs/zh/34-git-isolation.md", "git"),
        ("docs/en/37-code-search.md", "files"),
        ("docs/zh/37-code-search.md", "files"),
    ];

    for (relative, feature) in guides {
        let content = std::fs::read_to_string(root.join(relative))?;
        assert!(!content.contains("echo_channels"), "{relative}");
        assert!(!content.contains("echo_providers"), "{relative}");
        assert!(
            !content
                .lines()
                .any(|line| line.trim_start().starts_with("echo_tools =")),
            "{relative}"
        );
        assert!(
            content.contains(&format!("features = [\"{feature}\"]")),
            "{relative}"
        );
        if feature == "channels" {
            assert!(
                !content.contains("OPENAI_API_KEY\").unwrap_or_default()"),
                "{relative}"
            );
            assert!(
                content.contains("OPENAI_API_KEY is required for the IM channel provider"),
                "{relative}"
            );
        }
    }
    Ok(())
}

fn framework_example_sources() -> Result<BTreeMap<String, PathBuf>, Box<dyn std::error::Error>> {
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
                return Err(std::io::Error::other(format!(
                    "duplicate framework example source: {name}"
                ))
                .into());
            }
        }
    }
    Ok(sources)
}

#[test]
fn examples_readme_classifies_every_root_source_once() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = std::fs::read_to_string(root.join("examples/README.md"))?;
    let list_entry =
        Regex::new(r#"(?m)^- `(?:tests/example_contracts/)?(demo\d+_[a-z0-9_]+\.rs)`$"#)?;
    let sources = framework_example_sources()?;
    let mut classified = BTreeSet::new();
    let mut section_counts = BTreeMap::<String, usize>::new();
    let mut duplicates = Vec::new();
    let mut current_section = String::new();

    for line in readme.lines() {
        if let Some(section) = line.strip_prefix("## ") {
            current_section = section.to_string();
        }
        if let Some(captures) = list_entry.captures(line) {
            let Some(name) = captures.get(1).map(|value| value.as_str()) else {
                continue;
            };
            if !classified.insert(name.to_string()) {
                duplicates.push(name.to_string());
            }
            section_counts
                .entry(current_section.clone())
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
        }
    }

    let source_names = sources.keys().cloned().collect::<BTreeSet<_>>();
    let missing = source_names
        .difference(&classified)
        .cloned()
        .collect::<Vec<_>>();
    let unknown = classified
        .difference(&source_names)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        sources.len(),
        64,
        "unexpected framework example source count"
    );
    assert!(
        duplicates.is_empty(),
        "duplicate example dispositions: {duplicates:?}"
    );
    assert!(
        missing.is_empty(),
        "unclassified example sources: {missing:?}"
    );
    assert!(
        unknown.is_empty(),
        "unknown classified example sources: {unknown:?}"
    );
    for (section, expected) in [
        ("Root Composition And Teaching", 29),
        ("Executable Contract Tests", 21),
        ("Conditional", 14),
    ] {
        assert_eq!(
            section_counts.get(section),
            Some(&expected),
            "unexpected disposition count for {section}"
        );
    }
    Ok(())
}

fn example_manifest_block(
    manifest: &str,
    name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let pattern = Regex::new(&format!(
        r#"(?ms)^\[\[example\]\]\nname = "{}"\n(?P<body>.*?)(?:^\[\[|\z)"#,
        regex::escape(name)
    ))?;
    let block = pattern
        .captures(manifest)
        .and_then(|captures| captures.name("body"))
        .map(|body| body.as_str().to_string())
        .ok_or_else(|| std::io::Error::other(format!("missing [[example]] entry for {name}")))?;
    Ok(block)
}

#[test]
fn example_feature_requirements_match_sources() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml"))?;

    let guard = example_manifest_block(&manifest, "demo19_guard")?;
    assert!(guard.contains(r#"required-features = ["content-guard"]"#));

    let external_skills = example_manifest_block(&manifest, "demo08_external_skills")?;
    assert!(!external_skills.contains("required-features"));

    let harness = std::fs::read_to_string(root.join("tests/example_contracts.rs"))?;
    assert!(
        harness.contains(
            "#[cfg(all(feature = \"testing\", feature = \"data\", feature = \"media\"))]"
        )
    );
    assert!(harness.contains("example_contracts/demo43_data_tools.rs"));

    for moved in [
        "demo04_subagent",
        "demo12_resilience",
        "demo24_topology",
        "demo30_mcp_server",
        "demo31_memory_tools",
        "demo34_workflow_stream",
        "demo37_declarative_workflow",
        "demo39_workflow",
        "demo43_data_tools",
        "demo50_eval",
        "demo51_self_improvement",
        "demo53_adaptive_compression",
        "demo54_headless",
        "demo55_lsp_tools",
        "demo57_data_pipeline",
        "demo60_data_quality",
        "demo62_prompt_templates",
        "demo64_tool_pipeline",
        "demo65_context_assembler",
        "demo66_context_selector",
        "demo67_progress",
    ] {
        assert!(
            !manifest.contains(&format!("name = \"{moved}\"")),
            "moved contract must not remain a Cargo example target: {moved}"
        );
    }
    Ok(())
}

#[test]
fn framework_examples_avoid_forbidden_panics_and_byte_slices()
-> Result<(), Box<dyn std::error::Error>> {
    let byte_slice = Regex::new(r#"\[[^\]\n]*\.\.[^\]\n]*\]"#)?;
    let split_crate = Regex::new(
        r#"\b(?:echo_core|echo_execution|echo_integration|echo_macros|echo_orchestration|echo_state|echo_tools)::"#,
    )?;
    let json_index = Regex::new(r#"\b(?:json|value|response|resp|summary|item\.value)\s*\["#)?;
    let text_byte_count = Regex::new(r#"\b(?:content|text|output|code|word|prompt|id)\.len\(\)"#)?;
    let worker_term = Regex::new(r#"(?i)\bworkers?\b"#)?;
    let mut violations = Vec::new();

    for (name, path) in framework_example_sources()? {
        let source = std::fs::read_to_string(path)?;
        for (line_index, line) in source.lines().enumerate() {
            let forbidden = [
                ".unwrap()",
                ".unwrap_err()",
                ".expect(",
                ".expect_err(",
                "panic!(",
                "unreachable!(",
                "todo!(",
            ];
            for token in forbidden {
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
            if worker_term.is_match(line) {
                violations.push(format!(
                    "{name}:{} uses the retired worker terminology",
                    line_index + 1
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "framework example contract violations:\n{}",
        violations.join("\n")
    );
    Ok(())
}
