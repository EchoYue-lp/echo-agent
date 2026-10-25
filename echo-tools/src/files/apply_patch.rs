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
#[derive(Clone)]
pub struct ApplyPatchTool {
    base_dir: Option<PathBuf>,
    file_system: std::sync::Arc<dyn echo_core::tools::filesystem::FileSystem>,
}

impl ApplyPatchTool {
    pub fn with_file_system(
        mut self,
        file_system: std::sync::Arc<dyn echo_core::tools::filesystem::FileSystem>,
    ) -> Self {
        self.file_system = file_system;
        self
    }

    pub fn new() -> Self {
        Self {
            base_dir: None,
            file_system: std::sync::Arc::new(super::LocalFileSystem),
        }
    }

    pub fn with_base_dir(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base.into()),
            file_system: std::sync::Arc::new(super::LocalFileSystem),
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
        "Atomic multi-file patch: start '*** Begin Patch', end '*** End Patch'. Use '*** Add File: path', '*** Update File: path', '*** Delete File: path'; optional '*** Move to: path'. @@ hunks: + add, - remove, space context."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Complete patch."
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "Validate/diff; no writes."
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
            let file_system = self.file_system.clone();
            let base_dir = self.base_dir.clone();
            let working_dir = ctx.working_dir.clone();
            let guards = ctx.resource_guards.clone();
            tokio::task::spawn_blocking(move || {
                let _guards = guards;
                execute_patch(
                    file_system.as_ref(),
                    &patch,
                    dry_run,
                    base_dir.as_deref(),
                    working_dir.as_deref(),
                )
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

#[derive(Debug)]
enum PatchPreparationError {
    Validation(String),
    Backend(echo_core::error::ReactError),
}
impl From<String> for PatchPreparationError {
    fn from(message: String) -> Self {
        Self::Validation(message)
    }
}
impl std::fmt::Display for PatchPreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(message) => message.fmt(f),
            Self::Backend(error) => error.fmt(f),
        }
    }
}
fn preparation_io_error(
    error: echo_core::error::ReactError,
    local: impl FnOnce(&std::io::Error) -> String,
) -> PatchPreparationError {
    match error {
        echo_core::error::ReactError::Io(error) => PatchPreparationError::Validation(local(&error)),
        error => PatchPreparationError::Backend(error),
    }
}

fn execute_patch(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    patch: &str,
    dry_run: bool,
    base_dir: Option<&Path>,
    working_dir: Option<&Path>,
) -> Result<ToolResult> {
    let actions = match PatchParser::new(patch).parse() {
        Ok(actions) => actions,
        Err(error) => return Ok(ToolResult::invalid_arguments(error)),
    };
    let root = match resolve_root(file_system, base_dir, working_dir) {
        Ok(root) => root,
        Err(PatchPreparationError::Validation(message)) => {
            return Ok(ToolResult::invalid_arguments(message));
        }
        Err(PatchPreparationError::Backend(error)) => return Err(error),
    };
    let plan = match build_plan(file_system, actions, &root) {
        Ok(plan) => plan,
        Err(PatchPreparationError::Validation(message)) => {
            return Ok(ToolResult::invalid_arguments(message));
        }
        Err(PatchPreparationError::Backend(error)) => return Err(error),
    };
    if dry_run {
        return Ok(patch_result(&plan, true, Vec::new()));
    }

    let checkpoints = match create_checkpoints(file_system, &plan.mutations) {
        Ok(checkpoints) => checkpoints,
        Err(error) => {
            if !matches!(error, echo_core::error::ReactError::Io(_)) {
                return Err(error);
            }
            return Ok(ToolResult::failure(
                ToolFailureCategory::Permanent,
                error.to_string(),
            ));
        }
    };
    match commit_plan(file_system, &plan.mutations, &root) {
        Ok(()) => Ok(patch_result(&plan, false, checkpoints)),
        Err(failure) => {
            if let Some(provider_failure) = failure.provider_failure.clone()
                && provider_failure.side_effect != ToolSideEffect::None
            {
                // A lost write response is not proof of failure or cancellation.
                // Preserve any already-confirmed prefix, without inventing an
                // effect for the failed/unknown operation itself.
                let mut result = ToolResult::error(failure.error).with_failure(*provider_failure);
                for path in failure
                    .attempted_paths
                    .iter()
                    .take(failure.attempted_paths.len().saturating_sub(1))
                {
                    result = result.with_effect(echo_core::tools::ToolEffect::FileEdit {
                        path: path.display().to_string(),
                    });
                }
                if !failure.attempted_paths.is_empty() {
                    result = result.with_meta(
                        "unsettled_paths",
                        serde_json::to_string(&failure.attempted_paths).map_err(|error| {
                            ToolError::ExecutionFailed {
                                tool: "apply_patch".to_string(),
                                message: error.to_string(),
                            }
                        })?,
                    );
                }
                return Ok(result);
            }
            let changed = match confirmed_mutation_paths(
                file_system,
                &plan.mutations,
                &failure.attempted_paths,
            ) {
                Ok(changed) => changed,
                Err(error) => {
                    // Failure to observe rollback is not proof that an already
                    // attempted mutation did not happen. Never auto-retry it.
                    return Ok(ToolResult::error(format!(
                        "Patch failed ({}); rollback state could not be verified: {error}",
                        failure.error
                    ))
                    .with_failure(
                        ToolFailure::new(ToolFailureCategory::PartialSideEffect)
                            .with_side_effect(ToolSideEffect::Possible)
                            .with_postcondition(
                                "Inspect the attempted patch paths before retrying",
                            ),
                    )
                    .with_meta(
                        "unsettled_paths",
                        serde_json::to_string(&failure.attempted_paths).map_err(|error| {
                            ToolError::ExecutionFailed {
                                tool: "apply_patch".into(),
                                message: error.to_string(),
                            }
                        })?,
                    ));
                }
            };
            let mut result = if failure.rollback_errors.is_empty() && changed.is_empty() {
                ToolResult::error(format!(
                    "Patch failed and all applied changes were rolled back: {}",
                    failure.error
                ))
                .with_failure(
                    failure
                        .provider_failure
                        .map(|failure| *failure)
                        .unwrap_or_else(|| ToolFailure::new(ToolFailureCategory::Permanent)),
                )
            } else {
                let detail = if failure.rollback_errors.is_empty() {
                    format!("Patch failed and changed paths remain: {}", failure.error)
                } else {
                    format!(
                        "Patch failed: {}. Rollback also failed: {}",
                        failure.error,
                        failure.rollback_errors.join("; ")
                    )
                };
                ToolResult::error(detail).with_failure(
                    ToolFailure::new(ToolFailureCategory::PartialSideEffect)
                        .with_side_effect(ToolSideEffect::Possible)
                        .with_postcondition(
                            "Inspect every path in the returned patch before retrying",
                        ),
                )
            };
            for path in changed {
                result = result.with_effect(echo_core::tools::ToolEffect::FileEdit { path });
            }
            Ok(result)
        }
    }
}

fn confirmed_mutation_paths(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    mutations: &[PlannedMutation],
    attempted: &[PathBuf],
) -> Result<Vec<String>> {
    let attempted = attempted.iter().cloned().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut changed = Vec::new();
    for mutation in mutations {
        if !attempted.contains(mutation.path()) || !seen.insert(mutation.path().to_path_buf()) {
            continue;
        }
        let differs = match mutation.original() {
            None => file_system.exists(mutation.path())?,
            Some(before) => match file_system.read(mutation.path()) {
                Ok(current) => current != before,
                Err(echo_core::error::ReactError::Io(error)) => {
                    error.kind() == std::io::ErrorKind::NotFound
                }
                Err(error) => return Err(error),
            },
        };
        if differs {
            changed.push(mutation.path().display().to_string());
        }
    }
    Ok(changed)
}

fn resolve_root(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    base_dir: Option<&Path>,
    working_dir: Option<&Path>,
) -> std::result::Result<PathBuf, PatchPreparationError> {
    let root = match base_dir.or(working_dir) {
        Some(root) => root.to_path_buf(),
        None => file_system.current_dir().map_err(|error| {
            preparation_io_error(error, |error| {
                format!("cannot resolve current directory: {error}")
            })
        })?,
    };
    let canonical = file_system.canonicalize(&root).map_err(|error| {
        preparation_io_error(error, |error| {
            format!("cannot resolve patch root '{}': {error}", root.display())
        })
    })?;
    if !file_system
        .is_dir(&canonical)
        .map_err(|error| preparation_io_error(error, |error| error.to_string()))?
    {
        return Err(format!("patch root is not a directory: {}", canonical.display()).into());
    }
    Ok(canonical)
}

fn resolve_action_path(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    root: &Path,
    path: &str,
) -> std::result::Result<PathBuf, PatchPreparationError> {
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
    while !file_system
        .exists(&ancestor)
        .map_err(|error| preparation_io_error(error, |error| error.to_string()))?
    {
        let Some(name) = ancestor.file_name() else {
            return Err(format!("cannot resolve patch path '{path}'").into());
        };
        suffix.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            return Err(format!("cannot resolve patch path '{path}'").into());
        };
        ancestor = parent.to_path_buf();
    }
    let canonical_ancestor = file_system.canonicalize(&ancestor).map_err(|error| {
        preparation_io_error(error, |error| {
            format!("cannot resolve patch path '{path}': {error}")
        })
    })?;
    if !canonical_ancestor.starts_with(root) {
        return Err(format!("patch path '{path}' resolves outside the patch root").into());
    }
    let mut resolved = canonical_ancestor;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn build_plan(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    actions: Vec<PatchAction>,
    root: &Path,
) -> std::result::Result<PatchPlan, PatchPreparationError> {
    let mut claimed = HashSet::new();
    let mut mutations = Vec::new();
    let mut unified_diff = String::new();
    let changed_files = actions.len();

    for action in actions {
        match action {
            PatchAction::Add { path, contents } => {
                let target = resolve_action_path(file_system, root, &path)?;
                claim_path(&mut claimed, &target, &path)?;
                if file_system
                    .exists(&target)
                    .map_err(|error| preparation_io_error(error, |error| error.to_string()))?
                {
                    return Err(format!(
                        "cannot add '{path}': target already exists; use an update action"
                    )
                    .into());
                }
                append_diff(&mut unified_diff, "/dev/null", &path, "", &contents);
                mutations.push(PlannedMutation::Write {
                    path: target,
                    before: None,
                    after: contents.into_bytes(),
                });
            }
            PatchAction::Delete { path } => {
                let target = resolve_action_path(file_system, root, &path)?;
                claim_path(&mut claimed, &target, &path)?;
                let before = read_existing_file(file_system, &target, &path)?;
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
                let source = resolve_action_path(file_system, root, &path)?;
                claim_path(&mut claimed, &source, &path)?;
                let before = read_existing_file(file_system, &source, &path)?;
                let before_text = decode_text(&before, &path)?;
                let after_text = apply_chunks(&path, &before_text, &chunks)?;
                match move_to {
                    Some(destination_text) => {
                        let destination =
                            resolve_action_path(file_system, root, &destination_text)?;
                        claim_path(&mut claimed, &destination, &destination_text)?;
                        if file_system.exists(&destination).map_err(|error| {
                            preparation_io_error(error, |error| error.to_string())
                        })? {
                            return Err(format!(
                                "cannot move '{path}' to '{destination_text}': destination exists"
                            )
                            .into());
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
                            return Err(format!("update for '{path}' makes no changes").into());
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

fn read_existing_file(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    path: &Path,
    display: &str,
) -> std::result::Result<Vec<u8>, PatchPreparationError> {
    let metadata = file_system.metadata(path).map_err(|error| {
        preparation_io_error(error, |error| format!("cannot access '{display}': {error}"))
    })?;
    if !metadata.is_file() {
        return Err(format!("patch target is not a regular file: '{display}'").into());
    }
    file_system.read(path).map_err(|error| {
        preparation_io_error(error, |error| format!("cannot read '{display}': {error}"))
    })
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
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    mutations: &[PlannedMutation],
) -> Result<Vec<(String, String)>> {
    let mut seen = HashSet::new();
    let mut checkpoints = Vec::new();
    for mutation in mutations {
        let path = mutation.path();
        if mutation.original().is_none() || !seen.insert(path.to_path_buf()) {
            continue;
        }
        if let Some(checkpoint) = file_system.checkpoint(path)? {
            checkpoints.push((path.display().to_string(), checkpoint));
        }
    }
    Ok(checkpoints)
}

struct CommitFailure {
    error: String,
    rollback_errors: Vec<String>,
    attempted_paths: Vec<PathBuf>,
    provider_failure: Option<Box<ToolFailure>>,
}

fn commit_plan(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    mutations: &[PlannedMutation],
    root: &Path,
) -> std::result::Result<(), CommitFailure> {
    if let Err(error) = verify_originals_unchanged(file_system, mutations) {
        return Err(CommitFailure {
            error: error.to_string(),
            rollback_errors: Vec::new(),
            attempted_paths: Vec::new(),
            provider_failure: match error {
                PatchPreparationError::Backend(error) => {
                    Some(Box::new(ToolFailure::from_error(&error, false)))
                }
                _ => None,
            },
        });
    }
    let mut applied = Vec::new();
    let mut created_directories = Vec::new();
    for mutation in mutations {
        let result = match mutation {
            PlannedMutation::Write { path, after, .. } => {
                missing_parent_directories(file_system, path, root).and_then(|directories| {
                    created_directories.extend(directories);
                    file_system.atomic_write(path, after)
                })
            }
            PlannedMutation::Delete { path, .. } => file_system.remove_file(path),
        };
        if let Err(error) = result {
            let failure = ToolFailure::from_error(&error, false);
            if failure.side_effect != ToolSideEffect::None {
                let mut attempted_paths = applied
                    .iter()
                    .map(|mutation: &PlannedMutation| mutation.path().to_path_buf())
                    .collect::<Vec<_>>();
                attempted_paths.push(mutation.path().to_path_buf());
                return Err(CommitFailure {
                    error: error.to_string(),
                    rollback_errors: Vec::new(),
                    attempted_paths,
                    provider_failure: Some(Box::new(failure)),
                });
            }
            let rollback_errors = rollback_mutations(file_system, &applied, &created_directories);
            let mut attempted_paths = applied
                .iter()
                .map(|applied| applied.path().to_path_buf())
                .collect::<Vec<_>>();
            attempted_paths.push(mutation.path().to_path_buf());
            return Err(CommitFailure {
                error: format!("failed to update '{}': {error}", mutation.path().display()),
                rollback_errors,
                attempted_paths,
                provider_failure: if matches!(error, echo_core::error::ReactError::Io(_)) {
                    None
                } else {
                    Some(Box::new(failure))
                },
            });
        }
        applied.push(mutation.clone());
    }
    Ok(())
}

fn verify_originals_unchanged(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    mutations: &[PlannedMutation],
) -> std::result::Result<(), PatchPreparationError> {
    let mut seen = HashSet::new();
    for mutation in mutations {
        let path = mutation.path();
        if !seen.insert(path.to_path_buf()) {
            continue;
        }
        match mutation.original() {
            Some(expected) => {
                let current = file_system.read(path).map_err(|error| {
                    preparation_io_error(error, |error| {
                        format!("cannot re-read '{}' before commit: {error}", path.display())
                    })
                })?;
                if current != expected {
                    return Err(format!(
                        "file changed after patch validation: '{}'",
                        path.display()
                    )
                    .into());
                }
            }
            None if file_system
                .exists(path)
                .map_err(|error| preparation_io_error(error, |error| error.to_string()))? =>
            {
                return Err(format!(
                    "patch destination appeared after validation: '{}'",
                    path.display()
                )
                .into());
            }
            None => {}
        }
    }
    Ok(())
}

fn rollback_mutations(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    applied: &[PlannedMutation],
    created_directories: &[PathBuf],
) -> Vec<String> {
    let mut errors = Vec::new();
    for mutation in applied.iter().rev() {
        let result = match mutation {
            PlannedMutation::Write {
                path,
                before: Some(before),
                ..
            }
            | PlannedMutation::Delete { path, before } => file_system.atomic_write(path, before),
            PlannedMutation::Write {
                path, before: None, ..
            } => match file_system.remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if super::is_io_kind(&error, std::io::ErrorKind::NotFound) => Ok(()),
                Err(error) => Err(error),
            },
        };
        if let Err(error) = result {
            let failure = ToolFailure::from_error(&error, false);
            errors.push(format!("{}: {error}", mutation.path().display()));
            if failure.side_effect != ToolSideEffect::None {
                // No further rollback/cleanup writes while an undo operation
                // itself has an unknown result in the original backend owner.
                return errors;
            }
        }
    }
    for directory in created_directories.iter().rev() {
        if let Err(error) = file_system.remove_dir(directory)
            && !super::is_io_kind(&error, std::io::ErrorKind::NotFound)
            && !super::is_io_kind(&error, std::io::ErrorKind::DirectoryNotEmpty)
        {
            errors.push(format!("{}: {error}", directory.display()));
        }
    }
    errors
}

fn missing_parent_directories(
    file_system: &dyn echo_core::tools::filesystem::FileSystem,
    path: &Path,
    root: &Path,
) -> Result<Vec<PathBuf>> {
    let mut missing = Vec::new();
    let mut current = path.parent();
    while let Some(directory) = current {
        if directory == root || file_system.exists(directory)? {
            break;
        }
        missing.push(directory.to_path_buf());
        current = directory.parent();
    }
    missing.reverse();
    Ok(missing)
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
    if !dry_run {
        for mutation in &plan.mutations {
            result = result.with_effect(echo_core::tools::ToolEffect::FileEdit {
                path: mutation.path().display().to_string(),
            });
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
        let affected = result
            .effects
            .iter()
            .filter_map(|effect| match effect {
                echo_core::tools::ToolEffect::FileEdit { path } => Some(path.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let canonical_root = std::fs::canonicalize(root.path())?;
        assert_eq!(
            affected,
            [
                canonical_root.join("nested/新增.txt").display().to_string(),
                canonical_root.join("update.txt").display().to_string(),
                canonical_root.join("delete.txt").display().to_string(),
                canonical_root
                    .join("nested/moved.txt")
                    .display()
                    .to_string(),
                canonical_root.join("move.txt").display().to_string(),
            ]
        );
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
        assert!(preview.effects.is_empty());
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
    fn partial_rollback_effects_only_name_attempted_paths_still_changed() -> Result<()> {
        let root = tempfile::tempdir()?;
        let first = root.path().join("first.txt");
        let untouched = root.path().join("untouched.txt");
        std::fs::write(&first, "changed")?;
        std::fs::write(&untouched, "external change")?;
        let mutations = vec![
            PlannedMutation::Write {
                path: first.clone(),
                before: Some(b"original".to_vec()),
                after: b"changed".to_vec(),
            },
            PlannedMutation::Write {
                path: untouched,
                before: Some(b"original".to_vec()),
                after: b"agent change".to_vec(),
            },
        ];
        assert_eq!(
            confirmed_mutation_paths(
                &super::super::LocalFileSystem,
                &mutations,
                std::slice::from_ref(&first)
            )?,
            [first.display().to_string()]
        );
        std::fs::write(&first, "original")?;
        assert!(
            confirmed_mutation_paths(&super::super::LocalFileSystem, &mutations, &[first])?
                .is_empty()
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
