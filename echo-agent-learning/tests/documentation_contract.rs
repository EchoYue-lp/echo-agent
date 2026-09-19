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
    targets: Vec<WorkspaceTarget>,
}

#[derive(serde::Deserialize)]
struct WorkspaceTarget {
    name: String,
    kind: Vec<String>,
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

fn unresolved_local_links(
    root: &Path,
    files: &[PathBuf],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let link_pattern = Regex::new(r#"!?\[[^\]]*\]\(([^)]+)\)"#)?;
    let reference_pattern = Regex::new(r#"^\s*\[[^\]]+\]:\s*(\S+)"#)?;
    let mut broken = Vec::new();
    for source in files {
        let content = std::fs::read_to_string(source)?;
        let Some(parent) = source.parent() else {
            continue;
        };
        let relative = source.strip_prefix(root).unwrap_or(source).display();
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
    Ok(broken)
}

fn markdown_heading_profile(content: &str) -> Vec<usize> {
    let mut in_fence = false;
    let mut profile = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let level = trimmed
            .chars()
            .take_while(|character| *character == '#')
            .count();
        if level > 0 && trimmed.chars().nth(level) == Some(' ') {
            profile.push(level);
        }
    }
    profile
}

fn markdown_structure(content: &str) -> (usize, usize) {
    let mut table_rows = 0_usize;
    let mut fences = 0_usize;
    let mut in_fence = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fences = fences.saturating_add(1);
            in_fence = !in_fence;
        } else if !in_fence && trimmed.starts_with('|') && trimmed.ends_with('|') {
            table_rows = table_rows.saturating_add(1);
        }
    }
    (table_rows, fences)
}

fn has_expected_markdown_structure(
    content: &str,
    heading_profile: &[usize],
    structure: (usize, usize),
) -> bool {
    markdown_heading_profile(content) == heading_profile && markdown_structure(content) == structure
}

fn contains_targets_once_in_order(content: &str, targets: &[&str]) -> bool {
    let mut previous = None;
    for target in targets {
        if content.matches(target).count() != 1 {
            return false;
        }
        let Some(position) = content.find(target) else {
            return false;
        };
        if previous.is_some_and(|previous| previous >= position) {
            return false;
        }
        previous = Some(position);
    }
    true
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

fn command_option<'a>(tokens: &'a [String], flag: &str) -> Option<&'a str> {
    tokens.windows(2).find_map(|pair| {
        pair.first()
            .is_some_and(|value| value == flag)
            .then(|| pair.get(1).map(String::as_str))
            .flatten()
    })
}

fn contract_filter_defined(learning_root: &Path, filter: &str) -> Result<bool, std::io::Error> {
    let marker = format!("fn {filter}(");
    for entry in std::fs::read_dir(learning_root.join("tests/example_contracts"))? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) == Some("rs")
            && std::fs::read_to_string(path)?.contains(&marker)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[test]
fn root_readmes_match_workspace_package_topology() -> Result<(), Box<dyn std::error::Error>> {
    let (workspace_root, packages) = workspace_packages()?;
    if packages.len() != 9 {
        return Err(std::io::Error::other(format!(
            "expected the root package plus seven split framework members and one learning package, found {} packages",
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
    if (framework_count, sdk_count, learning_count) != (8, 0, 1) {
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
                "**{framework_count} framework/runtime packages + {learning_count} learning package**"
            ),
        ),
        (
            "README.zh.md",
            "## Workspace 结构",
            format!(
                "**{framework_count} 个框架/运行时 package + {learning_count} 个学习 package**"
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
fn root_readme_learning_commands_reference_cargo_targets() -> Result<(), Box<dyn std::error::Error>>
{
    let (workspace_root, packages) = workspace_packages()?;
    let learning_package = packages
        .iter()
        .find(|package| package.name == "echo-agent-learning")
        .ok_or_else(|| std::io::Error::other("cargo metadata is missing echo-agent-learning"))?;
    let example_targets = learning_package
        .targets
        .iter()
        .filter(|target| target.kind.iter().any(|kind| kind == "example"))
        .map(|target| target.name.clone())
        .collect::<BTreeSet<_>>();
    let test_targets = learning_package
        .targets
        .iter()
        .filter(|target| target.kind.iter().any(|kind| kind == "test"))
        .map(|target| target.name.clone())
        .collect::<BTreeSet<_>>();
    let learning_root = learning_package
        .manifest_path
        .parent()
        .ok_or_else(|| std::io::Error::other("learning manifest has no parent directory"))?;
    let mut violations = Vec::new();

    for path in ["README.md", "README.zh.md"] {
        let content = std::fs::read_to_string(workspace_root.join(path))?;
        let mut learning_commands = 0_usize;
        for line in content.lines().map(str::trim) {
            if !line.starts_with("cargo ") {
                continue;
            }
            let tokens = shlex::split(line).ok_or_else(|| {
                std::io::Error::other(format!("{path} contains an invalid shell command: {line}"))
            })?;
            let package =
                command_option(&tokens, "-p").or_else(|| command_option(&tokens, "--package"));
            if package != Some("echo-agent-learning") {
                continue;
            }
            learning_commands = learning_commands.saturating_add(1);
            match tokens.get(1).map(String::as_str) {
                Some("run") => {
                    let Some(target) = command_option(&tokens, "--example") else {
                        violations.push(format!(
                            "{path} learning run command has no --example target: {line}"
                        ));
                        continue;
                    };
                    if !example_targets.contains(target) {
                        violations.push(format!(
                            "{path} references missing learning example target {target}"
                        ));
                    }
                }
                Some("test") => {
                    let Some(target) = command_option(&tokens, "--test") else {
                        violations.push(format!(
                            "{path} learning test command has no --test target: {line}"
                        ));
                        continue;
                    };
                    if !test_targets.contains(target) {
                        violations.push(format!(
                            "{path} references missing learning test target {target}"
                        ));
                        continue;
                    }
                    if target == "example_contracts" {
                        let filters = tokens
                            .iter()
                            .filter(|token| token.starts_with("contract_"))
                            .collect::<Vec<_>>();
                        let filter = match filters.as_slice() {
                            [filter] => filter.as_str(),
                            _ => {
                                violations.push(format!(
                                    "{path} example_contracts command must have one contract_* filter: {line}"
                                ));
                                continue;
                            }
                        };
                        if !contract_filter_defined(learning_root, filter)? {
                            violations.push(format!(
                                "{path} references missing example contract filter {filter}"
                            ));
                        }
                    }
                }
                action => violations.push(format!(
                    "{path} uses unsupported learning cargo action {action:?}: {line}"
                )),
            }
        }
        if learning_commands == 0 {
            violations.push(format!("{path} contains no echo-agent-learning commands"));
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "README learning command target drift:\n{}",
            violations.join("\n")
        ))
        .into())
    }
}

#[test]
fn foundational_framework_docs_are_routed_and_structurally_paired()
-> Result<(), Box<dyn std::error::Error>> {
    let (workspace_root, _) = workspace_packages()?;
    let pairs = [
        (
            "docs/en/architecture.md",
            "docs/zh/architecture.md",
            [
                "echo_agent",
                "echo-core",
                "echo-agent-learning",
                "echo-agent-sdk",
            ]
            .as_slice(),
            [1, 2, 2, 2, 2, 2, 2, 2, 2].as_slice(),
            (31, 2),
        ),
        (
            "docs/en/concepts.md",
            "docs/zh/concepts.md",
            [
                "Agent",
                "Session",
                "Conversation",
                "Invocation",
                "Turn",
                "Task",
                "Plan",
                "Subagent",
                "Context",
                "Checkpoint",
                "Journal",
                "Projection",
                "Trace",
                "Delivery",
                "Revision",
            ]
            .as_slice(),
            [1, 2, 2, 2, 2, 2, 2, 2, 2, 2].as_slice(),
            (43, 2),
        ),
        (
            "docs/en/lifecycles.md",
            "docs/zh/lifecycles.md",
            [
                "Agent Turn",
                "Context",
                "Task",
                "Subagent",
                "Tool",
                "Permission",
                "Observation",
                "Delivery",
                "Extension",
                "SDK",
            ]
            .as_slice(),
            [1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2].as_slice(),
            (70, 12),
        ),
    ];
    let mut violations = Vec::new();
    let mut link_sources = Vec::new();
    for (english_path, chinese_path, required_terms, heading_profile, structure) in pairs {
        let english_path = workspace_root.join(english_path);
        let chinese_path = workspace_root.join(chinese_path);
        if !english_path.is_file() {
            violations.push(format!(
                "missing foundational document {}",
                english_path.display()
            ));
        }
        if !chinese_path.is_file() {
            violations.push(format!(
                "missing foundational document {}",
                chinese_path.display()
            ));
        }
        if !english_path.is_file() || !chinese_path.is_file() {
            continue;
        }
        let english = std::fs::read_to_string(&english_path)?;
        let chinese = std::fs::read_to_string(&chinese_path)?;
        if !has_expected_markdown_structure(&english, heading_profile, structure)
            || !has_expected_markdown_structure(&chinese, heading_profile, structure)
        {
            violations.push(format!(
                "foundational structure does not match the reviewed contract: {}={:?}/{:?}, {}={:?}/{:?}, expected={heading_profile:?}/{structure:?}",
                english_path.display(),
                markdown_heading_profile(&english),
                markdown_structure(&english),
                chinese_path.display(),
                markdown_heading_profile(&chinese),
                markdown_structure(&chinese)
            ));
        }
        for term in required_terms {
            if !english.contains(term) || !chinese.contains(term) {
                violations.push(format!(
                    "foundational pair {} / {} is missing stable term {term}",
                    english_path.display(),
                    chinese_path.display()
                ));
            }
        }
        link_sources.push(english_path);
        link_sources.push(chinese_path);
    }

    let navigation = [
        (
            "README.md",
            "## Architecture",
            [
                "docs/en/architecture.md",
                "docs/en/concepts.md",
                "docs/en/lifecycles.md",
            ],
        ),
        (
            "README.zh.md",
            "## 架构",
            [
                "docs/zh/architecture.md",
                "docs/zh/concepts.md",
                "docs/zh/lifecycles.md",
            ],
        ),
        (
            "docs/en/README.md",
            "## Start Here",
            ["./architecture.md", "./concepts.md", "./lifecycles.md"],
        ),
        (
            "docs/zh/README.md",
            "## 从这里开始",
            ["./architecture.md", "./concepts.md", "./lifecycles.md"],
        ),
    ];
    for (path, heading, targets) in navigation {
        let path = workspace_root.join(path);
        let content = std::fs::read_to_string(&path)?;
        let section = markdown_section(&content, heading)?;
        let link_targets = targets
            .iter()
            .map(|target| format!("]({target})"))
            .collect::<Vec<_>>();
        let link_target_refs = link_targets.iter().map(String::as_str).collect::<Vec<_>>();
        if !contains_targets_once_in_order(&section, &link_target_refs) {
            violations.push(format!(
                "{} does not contain the foundational links exactly once in order {targets:?}",
                path.display()
            ));
        }
        link_sources.push(path);
    }
    violations.extend(unresolved_local_links(&workspace_root, &link_sources)?);

    if violations.is_empty() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "foundational framework documentation contract failed:\n{}",
            violations.join("\n")
        ))
        .into())
    }
}

#[test]
fn foundational_structure_contract_rejects_symmetric_deletion_and_navigation_reorder()
-> Result<(), std::io::Error> {
    let complete = "# Title\n\n## Section\n\n| A | B |\n| --- | --- |\n```text\nflow\n```\n";
    if !has_expected_markdown_structure(complete, &[1, 2], (2, 2)) {
        return Err(std::io::Error::other(
            "synthetic complete document did not satisfy the structure contract",
        ));
    }
    if has_expected_markdown_structure("# Title\n", &[1, 2], (2, 2)) {
        return Err(std::io::Error::other(
            "symmetric section/table/diagram deletion passed the structure contract",
        ));
    }
    let reordered = "[Lifecycles](lifecycles) [Concepts](concepts) [Architecture](architecture)";
    if contains_targets_once_in_order(reordered, &["architecture", "concepts", "lifecycles"]) {
        return Err(std::io::Error::other(
            "reordered foundational navigation passed the order contract",
        ));
    }
    Ok(())
}

#[test]
fn task_and_workflow_authorities_are_documented_against_public_entries()
-> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| std::io::Error::other("learning package has no workspace root"))?;
    let entry_contracts = [
        (
            "echo-orchestration/src/tasks/revisioned.rs",
            [
                "pub struct RevisionedTaskGraph",
                "pub struct TaskRevisionService",
            ]
            .as_slice(),
        ),
        (
            "echo-orchestration/src/tasks/runtime_service.rs",
            ["pub struct RuntimeTaskService"].as_slice(),
        ),
        (
            "echo-orchestration/src/workflow/graph.rs",
            ["pub struct Graph", "async fn execute_loop"].as_slice(),
        ),
        (
            "echo-orchestration/src/workflow/dag.rs",
            ["pub struct DagWorkflow", "impl Workflow for DagWorkflow"].as_slice(),
        ),
    ];
    let mut violations = Vec::new();
    for (path, markers) in entry_contracts {
        let source = std::fs::read_to_string(root.join(path))?;
        for marker in markers {
            if !source.contains(marker) {
                violations.push(format!("{path} no longer defines {marker}"));
            }
        }
    }

    let adr_path = "docs/adr/0059-task-workflow-dag-authority.md";
    let adr = std::fs::read_to_string(root.join(adr_path))?;
    for marker in [
        "RevisionedTaskGraph",
        "TaskRevisionService",
        "RuntimeTaskService",
        "Graph::execute_loop",
        "DagWorkflow::run",
        "CheckpointStore",
    ] {
        if !adr.contains(marker) {
            violations.push(format!("{adr_path} omits public authority {marker}"));
        }
    }
    let mut docs = Vec::new();
    for language in ["en", "zh"] {
        for chapter in ["09-tasks", "17-graph-workflow"] {
            let path = root.join(format!("docs/{language}/{chapter}.md"));
            let content = std::fs::read_to_string(&path)?;
            if !content.contains("../adr/0059-task-workflow-dag-authority.md") {
                violations.push(format!("{} does not link {adr_path}", path.display()));
            }
            docs.push(path);
        }
    }
    docs.push(root.join(adr_path));
    violations.extend(unresolved_local_links(root, &docs)?);

    if violations.is_empty() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "Task/Workflow authority documentation drift:\n{}",
            violations.join("\n")
        ))
        .into())
    }
}

#[test]
fn learning_markdown_has_resolvable_local_links() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_markdown_files(root, &mut files)?;
    let broken = unresolved_local_links(root, &files)?;

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
fn plugin_publication_docs_and_demo_share_the_coordinator_receipt_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let learning_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = learning_root.parent().ok_or_else(|| {
        std::io::Error::other("learning package has no workspace parent directory")
    })?;
    let demo = std::fs::read_to_string(learning_root.join("examples/demo56_plugin_system.rs"))?;
    for entry in [
        "PluginCoordinator::new(registry, PluginIntegrator::new())",
        "coordinator.reconcile(&mut agent).await?",
        "coordinator.disable(&mut agent, &plugin_id).await?",
        "coordinator.shutdown(&mut agent).await?",
        "McpServerId::plugin",
    ] {
        assert!(demo.contains(entry), "plugin demo misses {entry}");
    }
    for language in ["en", "zh"] {
        let guide =
            std::fs::read_to_string(root.join(format!("docs/{language}/32-plugin-system.md")))?;
        for entry in [
            "PluginCoordinator",
            "coordinator.reconcile(&mut agent)",
            "coordinator.retry(&mut agent)",
            "ActualPending",
        ] {
            assert!(
                guide.contains(entry),
                "{language} plugin guide misses {entry}"
            );
        }
    }
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
