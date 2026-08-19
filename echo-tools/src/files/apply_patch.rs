//! Transactional file editing using the canonical `*** Begin Patch` format.

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{
    Tool, ToolFailure, ToolFailureCategory, ToolParameters, ToolResult, ToolResultKind,
    ToolRiskLevel, ToolSideEffect,
};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use similar::udiff;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

const BEGIN_PATCH: &str = "*** Begin Patch";
const END_PATCH: &str = "*** End Patch";
const ADD_FILE: &str = "*** Add File: ";
const DELETE_FILE: &str = "*** Delete File: ";
const UPDATE_FILE: &str = "*** Update File: ";
const MOVE_TO: &str = "*** Move to: ";
const END_OF_FILE: &str = "*** End of File";

/// Apply a multi-file patch relative to the active workspace.
pub struct ApplyPatchTool {
    base_dir: Option<PathBuf>,
}

impl ApplyPatchTool {
    pub fn new() -> Self {
        Self { base_dir: None }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
        }
    }
}

impl Default for ApplyPatchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply one transactional patch across files. The patch must start with '*** Begin Patch' and end with '*** End Patch'. Use '*** Add File: path', '*** Update File: path', '*** Delete File: path', optional '*** Move to: path', and @@ context hunks. Added lines start with +; removed lines with -; unchanged context lines with a space."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Complete apply_patch document"
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "Validate and return the diff without changing files"
                }
            },
            "required": ["patch"],
            "additionalProperties": false
        })
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read, ToolPermission::Write]
    }

    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::Standard
    }

    fn allows_parallel_batch_execution(&self) -> bool {
        false
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let patch = parameters
                .get("patch")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::MissingParameter("patch".to_string()))?
                .to_string();
            let dry_run = parameters
                .get("dry_run")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let base_dir = self.base_dir.clone();
            let working_dir = ctx.working_dir.clone();
            tokio::task::spawn_blocking(move || {
                execute_patch(&patch, dry_run, base_dir.as_deref(), working_dir.as_deref())
            })
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                tool: "apply_patch".to_string(),
                message: format!("Patch task failed: {error}"),
            })?
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PatchAction {
    Add {
        path: String,
        contents: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        chunks: Vec<UpdateChunk>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct UpdateChunk {
    context: Option<String>,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    end_of_file: bool,
}

impl UpdateChunk {
    fn has_lines(&self) -> bool {
        !self.old_lines.is_empty() || !self.new_lines.is_empty()
    }
}

struct PatchParser<'a> {
    lines: Vec<&'a str>,
    index: usize,
}

impl<'a> PatchParser<'a> {
    fn new(patch: &'a str) -> Self {
        let mut lines = patch
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .collect::<Vec<_>>();
        if lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        Self { lines, index: 0 }
    }

    fn parse(mut self) -> std::result::Result<Vec<PatchAction>, String> {
        if self.current_trimmed() != Some(BEGIN_PATCH) {
            return Err(format!("patch must start with '{BEGIN_PATCH}'"));
        }
        self.index = self.index.saturating_add(1);
        let mut actions = Vec::new();
        loop {
            let Some(line) = self.current_trimmed() else {
                return Err(format!("patch must end with '{END_PATCH}'"));
            };
            if line == END_PATCH {
                self.index = self.index.saturating_add(1);
                break;
            }
            if let Some(path) = line.strip_prefix(ADD_FILE) {
                actions.push(self.parse_add(path)?);
            } else if let Some(path) = line.strip_prefix(DELETE_FILE) {
                actions.push(self.parse_delete(path)?);
            } else if let Some(path) = line.strip_prefix(UPDATE_FILE) {
                actions.push(self.parse_update(path)?);
            } else {
                return Err(self.error(format!("expected a file action, found '{line}'")));
            }
        }
        if self.index != self.lines.len() {
            return Err(self.error("unexpected content after end marker"));
        }
        if actions.is_empty() {
            return Err("patch must contain at least one file action".to_string());
        }
        Ok(actions)
    }

    fn parse_add(&mut self, path: &str) -> std::result::Result<PatchAction, String> {
        let path = validate_patch_path_text(path)?;
        self.index = self.index.saturating_add(1);
        let mut lines = Vec::new();
        while let Some(line) = self.current() {
            if is_action_or_end(line) {
                break;
            }
            let Some(content) = line.strip_prefix('+') else {
                return Err(self.error("every added file line must start with '+'"));
            };
            lines.push(content.to_string());
            self.index = self.index.saturating_add(1);
        }
        if lines.is_empty() {
            return Err(self.error("add file action must contain at least one '+' line"));
        }
        let mut contents = lines.join("\n");
        contents.push('\n');
        Ok(PatchAction::Add { path, contents })
    }

    fn parse_delete(&mut self, path: &str) -> std::result::Result<PatchAction, String> {
        let path = validate_patch_path_text(path)?;
        self.index = self.index.saturating_add(1);
        Ok(PatchAction::Delete { path })
    }

    fn parse_update(&mut self, path: &str) -> std::result::Result<PatchAction, String> {
        let path = validate_patch_path_text(path)?;
        self.index = self.index.saturating_add(1);
        let move_to = self
            .current_trimmed()
            .and_then(|line| line.strip_prefix(MOVE_TO))
            .map(validate_patch_path_text)
            .transpose()?;
        if move_to.is_some() {
            self.index = self.index.saturating_add(1);
        }

        let mut chunks = Vec::new();
        let mut current: Option<UpdateChunk> = None;
        while let Some(line) = self.current() {
            if is_action_or_end(line) {
                break;
            }
            let trimmed = line.trim();
            if trimmed == END_OF_FILE {
                let Some(chunk) = current.as_mut() else {
                    return Err(self.error("end-of-file marker requires an update hunk"));
                };
                chunk.end_of_file = true;
                self.index = self.index.saturating_add(1);
                if self.current().is_some_and(|next| !is_action_or_end(next)) {
                    return Err(self.error("end-of-file marker must end the file action"));
                }
                continue;
            }
            if let Some(context) = trimmed.strip_prefix("@@") {
                if let Some(chunk) = current.take() {
                    validate_chunk(&chunk, self.index)?;
                    chunks.push(chunk);
                }
                current = Some(UpdateChunk {
                    context: (!context.trim().is_empty()).then(|| context.trim().to_string()),
                    ..Default::default()
                });
                self.index = self.index.saturating_add(1);
                continue;
            }

            let chunk = current.get_or_insert_with(UpdateChunk::default);
            if let Some(content) = line.strip_prefix(' ') {
                chunk.old_lines.push(content.to_string());
                chunk.new_lines.push(content.to_string());
            } else if let Some(content) = line.strip_prefix('-') {
                chunk.old_lines.push(content.to_string());
            } else if let Some(content) = line.strip_prefix('+') {
                chunk.new_lines.push(content.to_string());
            } else {
                return Err(self.error("update lines must start with ' ', '+', '-', or '@@'"));
            }
            self.index = self.index.saturating_add(1);
        }
        if let Some(chunk) = current.take() {
            validate_chunk(&chunk, self.index)?;
            chunks.push(chunk);
        }
        if chunks.is_empty() && move_to.is_none() {
            return Err(self.error("update action must contain a hunk or move destination"));
        }
        Ok(PatchAction::Update {
            path,
            move_to,
            chunks,
        })
    }

    fn current(&self) -> Option<&'a str> {
        self.lines.get(self.index).copied()
    }

    fn current_trimmed(&self) -> Option<&'a str> {
        self.current().map(str::trim)
    }

    fn error(&self, message: impl AsRef<str>) -> String {
        format!(
            "invalid patch at line {}: {}",
            self.index.saturating_add(1),
            message.as_ref()
        )
    }
}

fn validate_chunk(chunk: &UpdateChunk, index: usize) -> std::result::Result<(), String> {
    if chunk.has_lines() {
        Ok(())
    } else {
        Err(format!(
            "invalid patch near line {}: update hunk is empty",
            index.saturating_add(1)
        ))
    }
}

fn is_action_or_end(line: &str) -> bool {
    let line = line.trim();
    line == END_PATCH
        || line.starts_with(ADD_FILE)
        || line.starts_with(DELETE_FILE)
        || line.starts_with(UPDATE_FILE)
}

fn validate_patch_path_text(path: &str) -> std::result::Result<String, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("patch file path must not be empty".to_string());
    }
    let parsed = Path::new(path);
    if parsed.is_absolute()
        || parsed.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "patch path '{path}' must be relative and must not contain '..'"
        ));
    }
    if !parsed
        .components()
        .any(|component| matches!(component, Component::Normal(_)))
    {
        return Err(format!("patch path '{path}' does not name a file"));
    }
    Ok(path.to_string())
}

/// Return paths that must already exist for a patch to be valid.
///
/// The ReAct read-before-edit stage uses this without duplicating patch grammar.
pub fn existing_file_paths(patch: &str) -> std::result::Result<Vec<String>, String> {
    PatchParser::new(patch).parse().map(|actions| {
        actions
            .into_iter()
            .filter_map(|action| match action {
                PatchAction::Delete { path } | PatchAction::Update { path, .. } => Some(path),
                PatchAction::Add { .. } => None,
            })
            .collect()
    })
}

#[derive(Clone)]
enum PlannedMutation {
    Write {
        path: PathBuf,
        before: Option<Vec<u8>>,
        after: Vec<u8>,
    },
    Delete {
        path: PathBuf,
        before: Vec<u8>,
    },
}

impl PlannedMutation {
    fn path(&self) -> &Path {
        match self {
            Self::Write { path, .. } | Self::Delete { path, .. } => path,
        }
    }

    fn original(&self) -> Option<&[u8]> {
        match self {
            Self::Write { before, .. } => before.as_deref(),
            Self::Delete { before, .. } => Some(before),
        }
    }
}

struct PatchPlan {
    mutations: Vec<PlannedMutation>,
    unified_diff: String,
    changed_files: usize,
}

fn execute_patch(
    patch: &str,
    dry_run: bool,
    base_dir: Option<&Path>,
    working_dir: Option<&Path>,
) -> Result<ToolResult> {
    let actions = match PatchParser::new(patch).parse() {
        Ok(actions) => actions,
        Err(error) => return Ok(ToolResult::invalid_arguments(error)),
    };
    let root = match resolve_root(base_dir, working_dir) {
        Ok(root) => root,
        Err(error) => return Ok(ToolResult::invalid_arguments(error)),
    };
    let plan = match build_plan(actions, &root) {
        Ok(plan) => plan,
        Err(error) => return Ok(ToolResult::invalid_arguments(error)),
    };
    if dry_run {
        return Ok(patch_result(&plan, true, Vec::new()));
    }

    let checkpoints = match create_checkpoints(&plan.mutations) {
        Ok(checkpoints) => checkpoints,
        Err(error) => {
            return Ok(ToolResult::failure(ToolFailureCategory::Permanent, error));
        }
    };
    match commit_plan(&plan.mutations, &root) {
        Ok(()) => Ok(patch_result(&plan, false, checkpoints)),
        Err(failure) if failure.rollback_errors.is_empty() => Ok(ToolResult::failure(
            ToolFailureCategory::Permanent,
            format!(
                "Patch failed and all applied changes were rolled back: {}",
                failure.error
            ),
        )),
        Err(failure) => Ok(ToolResult::error(format!(
            "Patch failed: {}. Rollback also failed: {}",
            failure.error,
            failure.rollback_errors.join("; ")
        ))
        .with_failure(
            ToolFailure::new(ToolFailureCategory::PartialSideEffect)
                .with_side_effect(ToolSideEffect::Possible)
                .with_postcondition("Inspect every path in the returned patch before retrying"),
        )),
    }
}

fn resolve_root(
    base_dir: Option<&Path>,
    working_dir: Option<&Path>,
) -> std::result::Result<PathBuf, String> {
    let root = match base_dir.or(working_dir) {
        Some(root) => root.to_path_buf(),
        None => std::env::current_dir()
            .map_err(|error| format!("cannot resolve current directory: {error}"))?,
    };
    let canonical = std::fs::canonicalize(&root)
        .map_err(|error| format!("cannot resolve patch root '{}': {error}", root.display()))?;
    if !canonical.is_dir() {
        return Err(format!(
            "patch root is not a directory: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn resolve_action_path(root: &Path, path: &str) -> std::result::Result<PathBuf, String> {
    validate_patch_path_text(path)?;
    let relative = Path::new(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value),
            Component::CurDir => None,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => None,
        })
        .collect::<PathBuf>();
    let candidate = root.join(relative);
    let mut ancestor = candidate.clone();
    let mut suffix = Vec::<OsString>::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            return Err(format!("cannot resolve patch path '{path}'"));
        };
        suffix.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            return Err(format!("cannot resolve patch path '{path}'"));
        };
        ancestor = parent.to_path_buf();
    }
    let canonical_ancestor = std::fs::canonicalize(&ancestor)
        .map_err(|error| format!("cannot resolve patch path '{path}': {error}"))?;
    if !canonical_ancestor.starts_with(root) {
        return Err(format!(
            "patch path '{path}' resolves outside the patch root"
        ));
    }
    let mut resolved = canonical_ancestor;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn build_plan(actions: Vec<PatchAction>, root: &Path) -> std::result::Result<PatchPlan, String> {
    let mut claimed = HashSet::new();
    let mut mutations = Vec::new();
    let mut unified_diff = String::new();
    let changed_files = actions.len();

    for action in actions {
        match action {
            PatchAction::Add { path, contents } => {
                let target = resolve_action_path(root, &path)?;
                claim_path(&mut claimed, &target, &path)?;
                if target.exists() {
                    return Err(format!(
                        "cannot add '{path}': target already exists; use an update action"
                    ));
                }
                append_diff(&mut unified_diff, "/dev/null", &path, "", &contents);
                mutations.push(PlannedMutation::Write {
                    path: target,
                    before: None,
                    after: contents.into_bytes(),
                });
            }
            PatchAction::Delete { path } => {
                let target = resolve_action_path(root, &path)?;
                claim_path(&mut claimed, &target, &path)?;
                let before = read_existing_file(&target, &path)?;
                let before_text = decode_text(&before, &path)?;
                append_diff(&mut unified_diff, &path, "/dev/null", &before_text, "");
                mutations.push(PlannedMutation::Delete {
                    path: target,
                    before,
                });
            }
            PatchAction::Update {
                path,
                move_to,
                chunks,
            } => {
                let source = resolve_action_path(root, &path)?;
                claim_path(&mut claimed, &source, &path)?;
                let before = read_existing_file(&source, &path)?;
                let before_text = decode_text(&before, &path)?;
                let after_text = apply_chunks(&path, &before_text, &chunks)?;
                match move_to {
                    Some(destination_text) => {
                        let destination = resolve_action_path(root, &destination_text)?;
                        claim_path(&mut claimed, &destination, &destination_text)?;
                        if destination.exists() {
                            return Err(format!(
                                "cannot move '{path}' to '{destination_text}': destination exists"
                            ));
                        }
                        append_diff(
                            &mut unified_diff,
                            &path,
                            &destination_text,
                            &before_text,
                            &after_text,
                        );
                        mutations.push(PlannedMutation::Write {
                            path: destination,
                            before: None,
                            after: after_text.into_bytes(),
                        });
                        mutations.push(PlannedMutation::Delete {
                            path: source,
                            before,
                        });
                    }
                    None => {
                        if before_text == after_text {
                            return Err(format!("update for '{path}' makes no changes"));
                        }
                        append_diff(&mut unified_diff, &path, &path, &before_text, &after_text);
                        mutations.push(PlannedMutation::Write {
                            path: source,
                            before: Some(before),
                            after: after_text.into_bytes(),
                        });
                    }
                }
            }
        }
    }
    Ok(PatchPlan {
        mutations,
        unified_diff,
        changed_files,
    })
}

fn claim_path(
    claimed: &mut HashSet<PathBuf>,
    path: &Path,
    display: &str,
) -> std::result::Result<(), String> {
    if claimed.insert(path.to_path_buf()) {
        Ok(())
    } else {
        Err(format!(
            "patch contains more than one action for '{display}'"
        ))
    }
}

fn read_existing_file(path: &Path, display: &str) -> std::result::Result<Vec<u8>, String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("cannot access '{display}': {error}"))?;
    if !metadata.is_file() {
        return Err(format!("patch target is not a regular file: '{display}'"));
    }
    std::fs::read(path).map_err(|error| format!("cannot read '{display}': {error}"))
}

fn decode_text(bytes: &[u8], display: &str) -> std::result::Result<String, String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| format!("patch target is not valid UTF-8 text: '{display}'"))
}

struct TextDocument {
    lines: Vec<String>,
    trailing_newline: bool,
    line_ending: &'static str,
}

impl TextDocument {
    fn parse(text: &str) -> Self {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count();
        let line_ending = if crlf > lf.saturating_sub(crlf) {
            "\r\n"
        } else {
            "\n"
        };
        let trailing_newline = text.ends_with('\n');
        let mut lines = text
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect::<Vec<_>>();
        if trailing_newline {
            lines.pop();
        }
        Self {
            lines,
            trailing_newline,
            line_ending,
        }
    }

    fn render(self) -> String {
        let mut text = self.lines.join(self.line_ending);
        if self.trailing_newline {
            text.push_str(self.line_ending);
        }
        text
    }
}

fn apply_chunks(
    path: &str,
    original: &str,
    chunks: &[UpdateChunk],
) -> std::result::Result<String, String> {
    if chunks.is_empty() {
        return Ok(original.to_string());
    }
    let mut document = TextDocument::parse(original);
    let mut cursor = 0usize;
    for (chunk_index, chunk) in chunks.iter().enumerate() {
        if let Some(context) = chunk.context.as_deref() {
            let Some(position) = find_line(&document.lines, context, cursor) else {
                return Err(format!(
                    "update context not found in '{path}' for hunk {}: {context}",
                    chunk_index.saturating_add(1)
                ));
            };
            cursor = position.saturating_add(1);
        }
        let start = if chunk.old_lines.is_empty() {
            if chunk.end_of_file {
                document.lines.len()
            } else {
                cursor.min(document.lines.len())
            }
        } else {
            find_sequence(&document.lines, &chunk.old_lines, cursor).ok_or_else(|| {
                format!(
                    "update hunk {} did not match '{path}'; re-read the file and retry with current context",
                    chunk_index.saturating_add(1)
                )
            })?
        };
        let end = start.saturating_add(chunk.old_lines.len());
        if chunk.end_of_file && end != document.lines.len() {
            return Err(format!(
                "update hunk {} for '{path}' was marked end-of-file but matched earlier",
                chunk_index.saturating_add(1)
            ));
        }
        document
            .lines
            .splice(start..end, chunk.new_lines.iter().cloned());
        cursor = start.saturating_add(chunk.new_lines.len());
    }
    Ok(document.render())
}

fn find_line(lines: &[String], needle: &str, start: usize) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, line)| (line == needle).then_some(index))
}

fn find_sequence(lines: &[String], needle: &[String], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start.min(lines.len()));
    }
    if needle.len() > lines.len() {
        return None;
    }
    (start..=lines.len().saturating_sub(needle.len()))
        .find(|index| lines.get(*index..index.saturating_add(needle.len())) == Some(needle))
}

fn append_diff(target: &mut String, old_path: &str, new_path: &str, old: &str, new: &str) {
    if old == new && old_path == new_path {
        return;
    }
    let diff = udiff::unified_diff(
        similar::Algorithm::default(),
        old,
        new,
        3,
        Some((old_path, new_path)),
    );
    target.push_str(&diff);
    if !target.ends_with('\n') {
        target.push('\n');
    }
}

fn create_checkpoints(
    mutations: &[PlannedMutation],
) -> std::result::Result<Vec<(String, String)>, String> {
    let mut seen = HashSet::new();
    let mut checkpoints = Vec::new();
    for mutation in mutations {
        let path = mutation.path();
        if mutation.original().is_none() || !seen.insert(path.to_path_buf()) {
            continue;
        }
        if let Some(checkpoint) = crate::git_checkpoint::create_checkpoint(path)? {
            crate::git_checkpoint::cleanup_old_checkpoints(path, 10);
            checkpoints.push((path.display().to_string(), checkpoint));
        }
    }
    Ok(checkpoints)
}

struct CommitFailure {
    error: String,
    rollback_errors: Vec<String>,
}

fn commit_plan(
    mutations: &[PlannedMutation],
    root: &Path,
) -> std::result::Result<(), CommitFailure> {
    if let Err(error) = verify_originals_unchanged(mutations) {
        return Err(CommitFailure {
            error,
            rollback_errors: Vec::new(),
        });
    }
    let mut applied = Vec::new();
    let mut created_directories = Vec::new();
    for mutation in mutations {
        let result = match mutation {
            PlannedMutation::Write { path, after, .. } => {
                created_directories.extend(missing_parent_directories(path, root));
                echo_core::utils::fs::atomic_write(path, after)
            }
            PlannedMutation::Delete { path, .. } => std::fs::remove_file(path),
        };
        if let Err(error) = result {
            let rollback_errors = rollback_mutations(&applied, &created_directories);
            return Err(CommitFailure {
                error: format!("failed to update '{}': {error}", mutation.path().display()),
                rollback_errors,
            });
        }
        applied.push(mutation.clone());
    }
    Ok(())
}

fn verify_originals_unchanged(mutations: &[PlannedMutation]) -> std::result::Result<(), String> {
    let mut seen = HashSet::new();
    for mutation in mutations {
        let path = mutation.path();
        if !seen.insert(path.to_path_buf()) {
            continue;
        }
        match mutation.original() {
            Some(expected) => {
                let current = std::fs::read(path).map_err(|error| {
                    format!("cannot re-read '{}' before commit: {error}", path.display())
                })?;
                if current != expected {
                    return Err(format!(
                        "file changed after patch validation: '{}'",
                        path.display()
                    ));
                }
            }
            None if path.exists() => {
                return Err(format!(
                    "patch destination appeared after validation: '{}'",
                    path.display()
                ));
            }
            None => {}
        }
    }
    Ok(())
}

fn rollback_mutations(applied: &[PlannedMutation], created_directories: &[PathBuf]) -> Vec<String> {
    let mut errors = Vec::new();
    for mutation in applied.iter().rev() {
        let result = match mutation {
            PlannedMutation::Write {
                path,
                before: Some(before),
                ..
            }
            | PlannedMutation::Delete { path, before } => {
                echo_core::utils::fs::atomic_write(path, before)
            }
            PlannedMutation::Write {
                path, before: None, ..
            } => match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        };
        if let Err(error) = result {
            errors.push(format!("{}: {error}", mutation.path().display()));
        }
    }
    for directory in created_directories.iter().rev() {
        if let Err(error) = std::fs::remove_dir(directory)
            && error.kind() != std::io::ErrorKind::NotFound
            && error.kind() != std::io::ErrorKind::DirectoryNotEmpty
        {
            errors.push(format!("{}: {error}", directory.display()));
        }
    }
    errors
}

fn missing_parent_directories(path: &Path, root: &Path) -> Vec<PathBuf> {
    let mut missing = Vec::new();
    let mut current = path.parent();
    while let Some(directory) = current {
        if directory == root || directory.exists() {
            break;
        }
        missing.push(directory.to_path_buf());
        current = directory.parent();
    }
    missing.reverse();
    missing
}

fn patch_result(plan: &PatchPlan, dry_run: bool, checkpoints: Vec<(String, String)>) -> ToolResult {
    let label = if dry_run { "Validated" } else { "Applied" };
    let mut result = ToolResult::success_with_kind(
        ToolResultKind::Diff {
            unified_diff: plan.unified_diff.clone(),
        },
        format!(
            "{label} patch across {} file{}:\n{}",
            plan.changed_files,
            if plan.changed_files == 1 { "" } else { "s" },
            plan.unified_diff
        ),
    )
    .with_meta("changed_files", plan.changed_files.to_string())
    .with_meta("dry_run", dry_run.to_string());
    if !checkpoints.is_empty() {
        let checkpoint_map = checkpoints.into_iter().collect::<HashMap<String, String>>();
        if let Ok(encoded) = serde_json::to_string(&checkpoint_map) {
            result = result.with_meta("git_checkpoints", encoded);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(root: &Path) -> echo_core::tools::ToolContext {
        echo_core::tools::ToolContext {
            working_dir: Some(root.to_path_buf()),
            ..Default::default()
        }
    }

    async fn run_patch(root: &Path, patch: &str) -> Result<ToolResult> {
        ApplyPatchTool::new()
            .execute_with_context(
                HashMap::from([("patch".to_string(), json!(patch))]),
                &context(root),
            )
            .await
    }

    #[tokio::test]
    async fn applies_add_update_delete_move_with_unicode() -> Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("update.txt"), "标题\n旧值\n")?;
        std::fs::write(root.path().join("delete.txt"), "remove me\n")?;
        std::fs::write(root.path().join("move.txt"), "keep\n")?;
        let patch = "*** Begin Patch\n*** Add File: nested/新增.txt\n+你好\n*** Update File: update.txt\n@@ 标题\n-旧值\n+新值🙂\n*** Delete File: delete.txt\n*** Update File: move.txt\n*** Move to: nested/moved.txt\n@@\n-keep\n+kept\n*** End Patch";

        let result = run_patch(root.path(), patch).await?;

        assert!(result.success, "{:?}", result.error);
        assert_eq!(
            std::fs::read_to_string(root.path().join("nested/新增.txt"))?,
            "你好\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("update.txt"))?,
            "标题\n新值🙂\n"
        );
        assert!(!root.path().join("delete.txt").exists());
        assert!(!root.path().join("move.txt").exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join("nested/moved.txt"))?,
            "kept\n"
        );
        assert!(matches!(result.kind, ToolResultKind::Diff { .. }));
        Ok(())
    }

    #[tokio::test]
    async fn stale_context_leaves_every_file_unchanged() -> Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("first.txt"), "one\n")?;
        std::fs::write(root.path().join("second.txt"), "two\n")?;
        let patch = "*** Begin Patch\n*** Update File: first.txt\n@@\n-one\n+changed\n*** Update File: second.txt\n@@\n-missing\n+changed\n*** End Patch";

        let result = run_patch(root.path(), patch).await?;

        assert!(!result.success);
        assert_eq!(
            std::fs::read_to_string(root.path().join("first.txt"))?,
            "one\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("second.txt"))?,
            "two\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn rejects_path_traversal_and_existing_add_target() -> Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("exists.txt"), "safe\n")?;
        let traversal = "*** Begin Patch\n*** Add File: ../outside.txt\n+bad\n*** End Patch";
        let existing = "*** Begin Patch\n*** Add File: exists.txt\n+replace\n*** End Patch";

        let traversal_result = run_patch(root.path(), traversal).await?;
        let existing_result = run_patch(root.path(), existing).await?;

        assert!(!traversal_result.success);
        assert!(!existing_result.success);
        assert_eq!(
            std::fs::read_to_string(root.path().join("exists.txt"))?,
            "safe\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn preserves_crlf_and_supports_dry_run() -> Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("lines.txt"), b"first\r\nsecond\r\n")?;
        let patch =
            "*** Begin Patch\n*** Update File: lines.txt\n@@\n-second\n+changed\n*** End Patch";
        let parameters = HashMap::from([
            ("patch".to_string(), json!(patch)),
            ("dry_run".to_string(), json!(true)),
        ]);

        let preview = ApplyPatchTool::new()
            .execute_with_context(parameters, &context(root.path()))
            .await?;
        assert!(preview.success);
        assert_eq!(
            std::fs::read(root.path().join("lines.txt"))?,
            b"first\r\nsecond\r\n"
        );

        let applied = run_patch(root.path(), patch).await?;
        assert!(applied.success);
        assert_eq!(
            std::fs::read(root.path().join("lines.txt"))?,
            b"first\r\nchanged\r\n"
        );
        Ok(())
    }

    #[test]
    fn existing_paths_share_the_canonical_parser() -> std::result::Result<(), String> {
        let patch = "*** Begin Patch\n*** Add File: new.txt\n+x\n*** Update File: old.txt\n@@\n-a\n+b\n*** Delete File: stale.txt\n*** End Patch";
        assert_eq!(
            existing_file_paths(patch)?,
            vec!["old.txt".to_string(), "stale.txt".to_string()]
        );
        Ok(())
    }
}
