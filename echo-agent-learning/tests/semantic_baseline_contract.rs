use serde_yaml_ng::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TARGET_REVISION_ENV: &str = "ECHO_SEMANTIC_TARGET_REVISION";

fn repository_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "learning package has no repository parent".to_string())
}

fn yaml_field<'a>(value: &'a Value, field: &str) -> Result<&'a Value, String> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| format!("expected YAML mapping before field {field}"))?;
    mapping
        .get(Value::String(field.to_string()))
        .ok_or_else(|| format!("missing YAML field {field}"))
}

fn baseline_revision(root: &Path) -> Result<String, String> {
    let path = root.join(".echo-semantic/baseline.md");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let frontmatter = text
        .strip_prefix("---\n")
        .and_then(|body| body.split_once("\n---\n").map(|parts| parts.0))
        .ok_or_else(|| format!("{} has invalid frontmatter", path.display()))?;
    let data: Value = serde_yaml_ng::from_str(frontmatter)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    yaml_field(yaml_field(&data, "source_snapshot")?, "base_revision")?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "source_snapshot.base_revision must be a string".to_string())
}

fn git_output(root: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("failed to execute git {}: {error}", args.join(" ")))
}

fn git_success(root: &Path, args: &[&str]) -> Result<(), String> {
    let output = git_output(root, args)?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn git_revision(root: &Path, reference: &str) -> Result<String, String> {
    let commit = format!("{reference}^{{commit}}");
    let output = git_output(root, &["rev-parse", "--verify", &commit])?;
    if !output.status.success() {
        return Err(format!(
            "cannot resolve target revision {reference}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|error| format!("git revision was not UTF-8: {error}"))
}

fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> Result<bool, String> {
    let output = git_output(root, &["merge-base", "--is-ancestor", ancestor, descendant])?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        code => Err(format!(
            "git merge-base failed with {code:?}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

fn target_revision(root: &Path) -> Result<String, String> {
    if let Ok(reference) = env::var(TARGET_REVISION_ENV)
        && !reference.trim().is_empty()
    {
        return git_revision(root, reference.trim());
    }
    for reference in ["origin/main", "main"] {
        if let Ok(revision) = git_revision(root, reference) {
            return Ok(revision);
        }
    }
    Err(format!(
        "set {TARGET_REVISION_ENV} or provide a local origin/main or main ref"
    ))
}

fn write_and_commit(root: &Path, contents: &str, message: &str) -> Result<String, String> {
    fs::write(root.join("state.txt"), contents)
        .map_err(|error| format!("failed to write Git fixture: {error}"))?;
    git_success(root, &["add", "state.txt"])?;
    git_success(
        root,
        &["-c", "commit.gpgsign=false", "commit", "-m", message],
    )?;
    git_revision(root, "HEAD")
}

#[test]
fn semantic_baseline_revision_is_ancestor_of_target_main() -> Result<(), String> {
    let root = repository_root()?;
    let baseline = baseline_revision(&root)?;
    let target = target_revision(&root)?;
    if is_ancestor(&root, &baseline, &target)? {
        return Ok(());
    }
    Err(format!(
        "semantic baseline {baseline} is not an ancestor of target main {target}"
    ))
}

#[test]
fn squash_drops_feature_only_revision_from_target_history() -> Result<(), String> {
    let directory = tempfile::tempdir()
        .map_err(|error| format!("failed to create Git fixture directory: {error}"))?;
    let root = directory.path();
    git_success(root, &["init", "--initial-branch", "main"])?;
    git_success(root, &["config", "user.name", "Semantic Contract"])?;
    git_success(root, &["config", "user.email", "semantic@example.invalid"])?;

    let target = write_and_commit(root, "base\n", "base")?;
    git_success(root, &["switch", "-c", "feature"])?;
    let feature_revision = write_and_commit(root, "feature\n", "feature")?;
    if !is_ancestor(root, &feature_revision, &feature_revision)? {
        return Err("feature revision should be an ancestor of its own HEAD".to_string());
    }
    if is_ancestor(root, &feature_revision, &target)? {
        return Err("feature-only revision must not be an ancestor of target main".to_string());
    }

    git_success(root, &["switch", "main"])?;
    git_success(root, &["merge", "--squash", "feature"])?;
    let squash_revision = write_and_commit(root, "feature\n", "squash")?;
    if is_ancestor(root, &feature_revision, &squash_revision)? {
        return Err("feature-only revision survived squash ancestry".to_string());
    }
    if !is_ancestor(root, &target, &squash_revision)? {
        return Err("target main must remain an ancestor of the squash result".to_string());
    }
    Ok(())
}

#[test]
fn rust_ci_supplies_target_revision_and_full_history() -> Result<(), String> {
    let root = repository_root()?;
    let path = root.join(".github/workflows/rust-ci.yml");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let workflow: Value = serde_yaml_ng::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    let linux_test = yaml_field(yaml_field(&workflow, "jobs")?, "linux-test")?;
    let target = yaml_field(yaml_field(linux_test, "env")?, TARGET_REVISION_ENV)?
        .as_str()
        .ok_or_else(|| format!("{TARGET_REVISION_ENV} must be a string"))?;
    for required in ["github.event.pull_request.base.sha", "github.event.before"] {
        if !target.contains(required) {
            return Err(format!("{TARGET_REVISION_ENV} is missing {required}"));
        }
    }

    let steps = yaml_field(linux_test, "steps")?
        .as_sequence()
        .ok_or_else(|| "linux-test.steps must be a sequence".to_string())?;
    let checkout = steps
        .iter()
        .find(|step| {
            yaml_field(step, "uses")
                .ok()
                .and_then(Value::as_str)
                .is_some_and(|value| value.starts_with("actions/checkout@"))
        })
        .ok_or_else(|| "linux-test is missing actions/checkout".to_string())?;
    let fetch_depth = yaml_field(yaml_field(checkout, "with")?, "fetch-depth")?;
    if fetch_depth.as_u64() == Some(0) || fetch_depth.as_str() == Some("0") {
        return Ok(());
    }
    Err("linux-test checkout must use fetch-depth: 0".to_string())
}
