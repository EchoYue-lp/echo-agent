//! Contracts for the learning package's docs, examples, and public facade.

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(serde::Deserialize)]
struct WorkspaceMetadata {
    packages: Vec<WorkspacePackage>,
    workspace_members: Vec<String>,
}

#[derive(serde::Deserialize)]
struct WorkspacePackage {
    id: String,
    name: String,
    manifest_path: PathBuf,
    features: BTreeMap<String, Vec<String>>,
}

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

fn workspace_packages() -> Result<(PathBuf, Vec<WorkspacePackage>), Box<dyn std::error::Error>> {
    let learning_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = learning_root.parent().ok_or_else(|| {
        std::io::Error::other("learning package has no workspace parent directory")
    })?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .current_dir(workspace_root)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
        .into());
    }
    let metadata: WorkspaceMetadata = serde_json::from_slice(&output.stdout)?;
    let member_ids = metadata
        .workspace_members
        .into_iter()
        .collect::<BTreeSet<_>>();
    let packages = metadata
        .packages
        .into_iter()
        .filter(|package| member_ids.contains(&package.id))
        .collect::<Vec<_>>();
    if packages.len() != member_ids.len() {
        return Err(std::io::Error::other(format!(
            "cargo metadata returned {} workspace members but {} matching packages",
            member_ids.len(),
            packages.len()
        ))
        .into());
    }
    Ok((workspace_root.to_path_buf(), packages))
}

fn markdown_section(content: &str, heading: &str) -> Result<String, std::io::Error> {
    let mut found = false;
    let mut section = String::new();
    for line in content.lines() {
        if !found {
            found = line.trim() == heading;
            continue;
        }
        if line.starts_with("## ") {
            break;
        }
        section.push_str(line);
        section.push('\n');
    }
    if found {
        Ok(section)
    } else {
        Err(std::io::Error::other(format!(
            "missing Markdown section: {heading}"
        )))
    }
}

fn feature_table_entries(section: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let feature_row = Regex::new(r"^\|\s*`([^`]+)`\s*\|")?;
    let entries = section
        .lines()
        .filter_map(|line| {
            feature_row
                .captures(line)
                .and_then(|captures| captures.get(1))
                .map(|value| value.as_str().to_string())
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        Err(std::io::Error::other("feature table contains no feature rows").into())
    } else {
        Ok(entries)
    }
}

#[test]
fn root_readmes_match_workspace_package_topology() -> Result<(), Box<dyn std::error::Error>> {
    let (workspace_root, packages) = workspace_packages()?;
    if packages.len() != 11 {
        return Err(std::io::Error::other(format!(
            "expected the root package plus ten workspace members, found {} packages",
            packages.len()
        ))
        .into());
    }

    let framework_count = packages
        .iter()
        .filter(|package| {
            package.name != "echo-agent-learning" && !package.name.starts_with("echo-sdk-")
        })
        .count();
    let sdk_count = packages
        .iter()
        .filter(|package| package.name.starts_with("echo-sdk-"))
        .count();
    let learning_count = packages
        .iter()
        .filter(|package| package.name == "echo-agent-learning")
        .count();
    if (framework_count, sdk_count, learning_count) != (8, 2, 1) {
        return Err(std::io::Error::other(format!(
            "unexpected package groups: framework/runtime={framework_count}, sdk={sdk_count}, learning={learning_count}"
        ))
        .into());
    }

    let mut package_directories = BTreeSet::new();
    for package in &packages {
        let manifest_parent = package.manifest_path.parent().ok_or_else(|| {
            std::io::Error::other(format!(
                "workspace package {} has no manifest parent",
                package.name
            ))
        })?;
        let relative = manifest_parent.strip_prefix(&workspace_root).map_err(|_| {
            std::io::Error::other(format!(
                "workspace package {} is outside {}",
                package.name,
                workspace_root.display()
            ))
        })?;
        if !relative.as_os_str().is_empty() {
            package_directories.insert(relative.display().to_string());
        }
    }
    let readmes = [
        (
            "README.md",
            "## Workspace Structure",
            format!(
                "**{framework_count} framework/runtime packages + {sdk_count} SDK packages + {learning_count} learning package**"
            ),
        ),
        (
            "README.zh.md",
            "## Workspace 结构",
            format!(
                "**{framework_count} 个框架/运行时 package + {sdk_count} 个 SDK package + {learning_count} 个学习 package**"
            ),
        ),
    ];
    let mut violations = Vec::new();
    for (path, heading, package_summary) in readmes {
        let content = std::fs::read_to_string(workspace_root.join(path))?;
        let topology = markdown_section(&content, heading)?;
        for directory in &package_directories {
            let marker = format!("{directory}/");
            if topology.matches(&marker).count() != 1 {
                violations.push(format!(
                    "{path} workspace topology must list {marker} exactly once"
                ));
            }
        }
        if !content.contains(&package_summary) {
            violations.push(format!(
                "{path} must report the Cargo-derived package groups as {package_summary}"
            ));
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "workspace documentation topology drift:\n{}",
            violations.join("\n")
        ))
        .into())
    }
}

#[test]
fn root_readme_feature_tables_match_cargo_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let (workspace_root, packages) = workspace_packages()?;
    let root_manifest = workspace_root.join("Cargo.toml");
    let root_package = packages
        .iter()
        .find(|package| package.manifest_path == root_manifest)
        .ok_or_else(|| std::io::Error::other("cargo metadata is missing the root package"))?;
    let expected = root_package
        .features
        .keys()
        .filter(|feature| feature.as_str() != "default")
        .cloned()
        .collect::<BTreeSet<_>>();
    if expected.len() != 27 || expected.contains("tasks") {
        return Err(std::io::Error::other(format!(
            "unexpected root feature set: expected 27 public rows without tasks, found {expected:?}"
        ))
        .into());
    }

    let readmes = [
        ("README.md", "### Feature Flags"),
        ("README.zh.md", "## Feature Flags"),
    ];
    let mut violations = Vec::new();
    for (path, heading) in readmes {
        let content = std::fs::read_to_string(workspace_root.join(path))?;
        let section = markdown_section(&content, heading)?;
        let entries = feature_table_entries(&section)?;
        let actual = entries.iter().cloned().collect::<BTreeSet<_>>();
        if actual.len() != entries.len() {
            violations.push(format!("{path} feature table contains duplicate rows"));
        }
        let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
        let extra = actual.difference(&expected).cloned().collect::<Vec<_>>();
        if !missing.is_empty() || !extra.is_empty() {
            violations.push(format!(
                "{path} feature table differs from Cargo metadata: missing={missing:?}, extra={extra:?}"
            ));
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "README feature table drift:\n{}",
            violations.join("\n")
        ))
        .into())
    }
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
