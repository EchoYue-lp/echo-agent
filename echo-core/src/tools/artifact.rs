//! Durable, bounded tool-output artifacts.
//!
//! Streaming tools can keep a small in-memory projection while this writer
//! spills the complete text payload to disk once the configured threshold is
//! crossed. Applications choose the root directory and retention policy.

use super::{ToolContext, ToolOutputChannel};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime};

pub const DEFAULT_TOOL_OUTPUT_ARTIFACT_THRESHOLD_BYTES: usize = 1024 * 1024;
pub const DEFAULT_TEMP_ARTIFACT_MAX_AGE_SECS: u64 = 60 * 60;

#[derive(Default)]
struct ArtifactScopeRegistry {
    active_scopes: HashMap<PathBuf, usize>,
    active_scope_roots: HashMap<PathBuf, ArtifactRootIdentity>,
    active_alias_roots: HashMap<PathBuf, (ArtifactRootIdentity, usize)>,
    pending_cleanup_roots: HashMap<CleanupRequestKey, CleanupDebt>,
}

#[derive(Clone, Debug)]
struct ArtifactRootIdentity {
    path: PathBuf,
    guard: Arc<crate::utils::fs::ExistingDirectoryGuard>,
}

impl ArtifactRootIdentity {
    fn capture(path: &Path) -> io::Result<Self> {
        let path = fs::canonicalize(path)?;
        Ok(Self {
            guard: Arc::new(crate::utils::fs::open_existing_directory_guard(&path)?),
            path,
        })
    }

    fn same_owner(&self, other: &Self) -> bool {
        self.path == other.path
            && self.verify().is_ok()
            && crate::utils::fs::verify_existing_directory(&self.path, &other.guard).is_ok()
    }

    fn verify(&self) -> io::Result<()> {
        crate::utils::fs::verify_existing_directory(&self.path, &self.guard).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("tool artifact root physical identity changed: {error}"),
            )
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CleanupRequestKey {
    alias_root: PathBuf,
    owner: String,
    run: Option<String>,
}

impl CleanupRequestKey {
    fn new(root: &Path, conversation_id: &str, run_id: Option<&str>) -> io::Result<Self> {
        Ok(Self {
            alias_root: artifact_root_alias(root)?,
            owner: artifact_scope_component(conversation_id),
            run: run_id.map(artifact_scope_component),
        })
    }

    fn path_under(&self, root: &Path) -> PathBuf {
        let path = root.join(&self.owner);
        match &self.run {
            Some(run) => path.join(run),
            None => path,
        }
    }
}

#[derive(Clone)]
struct CleanupDebt {
    path: PathBuf,
    root: ArtifactRootIdentity,
}

static ARTIFACT_SCOPE_REGISTRY: LazyLock<Mutex<ArtifactScopeRegistry>> =
    LazyLock::new(|| Mutex::new(ArtifactScopeRegistry::default()));

/// Settlement of an explicitly requested tool-output scope cleanup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolOutputScopeCleanupState {
    Settled,
    Pending,
    Retryable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutputArtifactConfig {
    pub root_dir: PathBuf,
    pub retention: String,
    pub threshold_bytes: usize,
    pub max_age_secs: Option<u64>,
}

impl ToolOutputArtifactConfig {
    pub fn new(root_dir: impl Into<PathBuf>, retention: impl Into<String>) -> Self {
        Self {
            root_dir: root_dir.into(),
            retention: retention.into(),
            threshold_bytes: DEFAULT_TOOL_OUTPUT_ARTIFACT_THRESHOLD_BYTES,
            max_age_secs: None,
        }
    }

    pub fn threshold_bytes(mut self, threshold_bytes: usize) -> Self {
        self.threshold_bytes = threshold_bytes.max(1);
        self
    }

    pub fn max_age_secs(mut self, max_age_secs: Option<u64>) -> Self {
        self.max_age_secs = max_age_secs;
        self
    }
}

impl Default for ToolOutputArtifactConfig {
    fn default() -> Self {
        Self::new(
            std::env::temp_dir()
                .join("echo_agent_artifacts")
                .join("tool-logs"),
            "temporary_1h",
        )
        .max_age_secs(Some(DEFAULT_TEMP_ARTIFACT_MAX_AGE_SECS))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutputArtifactIdentity {
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
    pub call_id: String,
    pub tool_name: String,
}

impl ToolOutputArtifactIdentity {
    pub fn from_context(ctx: &ToolContext, tool_name: impl Into<String>) -> Self {
        Self {
            conversation_id: ctx.conversation_id.clone(),
            run_id: ctx.run_id.clone().or_else(|| ctx.turn_id.clone()),
            call_id: ctx
                .call_id
                .clone()
                .unwrap_or_else(|| format!("call_{}", uuid::Uuid::new_v4())),
            tool_name: tool_name.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutputArtifactRef {
    pub path: PathBuf,
    pub artifact_bytes: u64,
    pub payload_bytes: u64,
    pub sha256: String,
    pub retention: String,
}

pub struct ToolOutputArtifactWriter {
    config: ToolOutputArtifactConfig,
    root_identity_valid: bool,
    root_identity: Option<ArtifactRootIdentity>,
    alias_root: Option<PathBuf>,
    registered: bool,
    scope_dir: PathBuf,
    final_path: PathBuf,
    partial_path: PathBuf,
    buffered: Vec<u8>,
    file: Option<File>,
    hasher: Sha256,
    artifact_bytes: u64,
    payload_bytes: u64,
    last_channel: Option<ToolOutputChannel>,
    completed: bool,
}

impl ToolOutputArtifactWriter {
    pub fn new(mut config: ToolOutputArtifactConfig, identity: ToolOutputArtifactIdentity) -> Self {
        let alias_root = artifact_root_alias(&config.root_dir).ok();
        let root_identity = fs::create_dir_all(&config.root_dir)
            .and_then(|_| ArtifactRootIdentity::capture(&config.root_dir))
            .ok();
        if let Some(root) = &root_identity {
            config.root_dir = root.path.clone();
        }
        let owner = artifact_scope_component(
            identity
                .conversation_id
                .as_deref()
                .or(identity.run_id.as_deref())
                .unwrap_or("unscoped-session"),
        );
        let run = artifact_scope_component(identity.run_id.as_deref().unwrap_or("session"));
        let call = artifact_scope_component(&identity.call_id);
        let tool = artifact_scope_component(&identity.tool_name);
        let nonce = uuid::Uuid::new_v4();
        let directory = config.root_dir.join(owner).join(run);
        let filename = format!("{call}-{tool}-{nonce}.log");
        let final_path = directory.join(filename);
        let partial_path = final_path.with_extension("log.partial");
        let registered = match (&alias_root, &root_identity) {
            (Some(alias), Some(root)) => register_artifact_scope(&directory, alias, root),
            _ => false,
        };
        Self {
            config,
            root_identity_valid: registered,
            root_identity,
            alias_root,
            registered,
            scope_dir: directory,
            final_path,
            partial_path,
            buffered: Vec::new(),
            file: None,
            hasher: Sha256::new(),
            artifact_bytes: 0,
            payload_bytes: 0,
            last_channel: None,
            completed: false,
        }
    }

    pub fn push_channel(&mut self, channel: ToolOutputChannel, text: &str) -> io::Result<()> {
        if self.last_channel.as_ref() != Some(&channel) {
            let label = match channel {
                ToolOutputChannel::Stdout => "\n[stdout]\n",
                ToolOutputChannel::Stderr => "\n[stderr]\n",
                ToolOutputChannel::Log => "\n[log]\n",
            };
            self.write_artifact_bytes(label.as_bytes())?;
            self.last_channel = Some(channel);
        }
        self.payload_bytes = self
            .payload_bytes
            .saturating_add(u64::try_from(text.len()).unwrap_or(u64::MAX));
        self.write_artifact_bytes(text.as_bytes())
    }

    pub fn push_raw(&mut self, text: &str) -> io::Result<()> {
        self.payload_bytes = self
            .payload_bytes
            .saturating_add(u64::try_from(text.len()).unwrap_or(u64::MAX));
        self.write_artifact_bytes(text.as_bytes())
    }

    pub fn finish(mut self) -> io::Result<Option<ToolOutputArtifactRef>> {
        if self.payload_bytes < u64::try_from(self.config.threshold_bytes).unwrap_or(u64::MAX) {
            self.completed = true;
            return Ok(None);
        }
        self.ensure_file()?;
        if let Some(file) = self.file.as_mut() {
            file.flush()?;
            file.sync_all()?;
        }
        self.file = None;
        fs::rename(&self.partial_path, &self.final_path)?;
        self.completed = true;
        Ok(Some(ToolOutputArtifactRef {
            path: self.final_path.clone(),
            artifact_bytes: self.artifact_bytes,
            payload_bytes: self.payload_bytes,
            sha256: format!("{:x}", self.hasher.clone().finalize()),
            retention: self.config.retention.clone(),
        }))
    }

    /// Finalize on an owned blocking task so cancellation cannot release the
    /// scope while a queued filesystem operation can still publish the file.
    pub async fn finish_async(self) -> io::Result<Option<ToolOutputArtifactRef>> {
        let runtime = tokio::runtime::Handle::try_current().map_err(io::Error::other)?;
        runtime
            .spawn_blocking(move || self.finish())
            .await
            .map_err(io::Error::other)?
    }

    fn write_artifact_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.artifact_bytes = self
            .artifact_bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        self.hasher.update(bytes);
        if self.file.is_none()
            && self.payload_bytes < u64::try_from(self.config.threshold_bytes).unwrap_or(u64::MAX)
        {
            self.buffered.extend_from_slice(bytes);
            return Ok(());
        }
        self.ensure_file()?;
        if let Some(file) = self.file.as_mut() {
            file.write_all(bytes)?;
        }
        Ok(())
    }

    fn ensure_file(&mut self) -> io::Result<()> {
        if self.file.is_some() {
            return Ok(());
        }
        if !self.root_identity_valid {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "tool artifact root cannot be normalized",
            ));
        }
        let Some(directory) = self.partial_path.parent() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "tool artifact path has no parent directory",
            ));
        };
        // Cleanup and file creation share the scope registry lock. Explicit
        // conversation cleanup is deferred while any writer in that scope is
        // alive, including writers that have not crossed the spill threshold.
        let create_guard = ARTIFACT_SCOPE_REGISTRY
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = self.root_identity.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "tool artifact root has no owner",
            )
        })?;
        root.verify()?;
        if let Some(max_age_secs) = self.config.max_age_secs {
            cleanup_artifacts_older_than(
                &self.config.root_dir,
                Duration::from_secs(max_age_secs),
                &create_guard.active_scopes,
                Some(root),
            );
        }
        fs::create_dir_all(directory)?;
        root.verify()?;
        if fs::canonicalize(directory)? != self.scope_dir {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "tool artifact scope identity changed before file creation",
            ));
        }
        let mut file = File::create(&self.partial_path)?;
        drop(create_guard);
        if !self.buffered.is_empty() {
            file.write_all(&self.buffered)?;
            self.buffered.clear();
        }
        self.file = Some(file);
        Ok(())
    }
}

impl Drop for ToolOutputArtifactWriter {
    fn drop(&mut self) {
        if !self.completed
            && self
                .root_identity
                .as_ref()
                .is_some_and(|root| root.verify().is_ok())
            && normalize_artifact_root(&self.partial_path)
                .is_ok_and(|path| path == self.partial_path)
            && self.partial_path.exists()
        {
            let _ = fs::remove_file(&self.partial_path);
        }
        if self.registered
            && let Some(alias) = &self.alias_root
        {
            unregister_artifact_scope(&self.scope_dir, alias);
        }
    }
}

pub fn persist_tool_output(
    config: ToolOutputArtifactConfig,
    identity: ToolOutputArtifactIdentity,
    output: &str,
) -> io::Result<Option<ToolOutputArtifactRef>> {
    let mut writer = ToolOutputArtifactWriter::new(config, identity);
    writer.push_raw(output)?;
    writer.finish()
}

pub fn cleanup_tool_output_scope(
    config: &ToolOutputArtifactConfig,
    conversation_id: &str,
    run_id: Option<&str>,
) -> io::Result<()> {
    let key = CleanupRequestKey::new(&config.root_dir, conversation_id, run_id)?;
    let mut registry = ARTIFACT_SCOPE_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let debt = if let Some(pending) = registry.pending_cleanup_roots.get(&key) {
        pending.clone()
    } else if let Some((root, _)) = registry.active_alias_roots.get(&key.alias_root) {
        CleanupDebt {
            path: key.path_under(&root.path),
            root: root.clone(),
        }
    } else {
        let current = normalize_artifact_root(&config.root_dir)?;
        let root = match ArtifactRootIdentity::capture(&current) {
            Ok(root) => root,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return match fs::symlink_metadata(&config.root_dir) {
                    Err(missing) if missing.kind() == io::ErrorKind::NotFound => Ok(()),
                    _ => Err(error),
                };
            }
            Err(error) => return Err(error),
        };
        CleanupDebt {
            path: key.path_under(&root.path),
            root,
        }
    };
    if registry
        .active_scopes
        .keys()
        .any(|active_scope| active_scope.starts_with(&debt.path))
    {
        registry.pending_cleanup_roots.insert(key, debt);
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "tool output scope cleanup is pending active writers",
        ));
    }
    match remove_artifact_scope(&debt) {
        Ok(()) => {
            registry.pending_cleanup_roots.remove(&key);
            Ok(())
        }
        Err(error) => {
            registry.pending_cleanup_roots.insert(key, debt);
            Err(error)
        }
    }
}

/// Query the exact scope after requesting cleanup. `Retryable` retains failed
/// deferred deletion so the caller can retry `cleanup_tool_output_scope`.
pub fn tool_output_scope_cleanup_state(
    config: &ToolOutputArtifactConfig,
    conversation_id: &str,
    run_id: Option<&str>,
) -> ToolOutputScopeCleanupState {
    let Ok(key) = CleanupRequestKey::new(&config.root_dir, conversation_id, run_id) else {
        return ToolOutputScopeCleanupState::Retryable;
    };
    let registry = ARTIFACT_SCOPE_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(debt) = registry.pending_cleanup_roots.get(&key) else {
        return ToolOutputScopeCleanupState::Settled;
    };
    if registry
        .active_scopes
        .keys()
        .any(|active_scope| active_scope.starts_with(&debt.path))
    {
        ToolOutputScopeCleanupState::Pending
    } else {
        ToolOutputScopeCleanupState::Retryable
    }
}

fn register_artifact_scope(
    scope_dir: &Path,
    alias_root: &Path,
    root: &ArtifactRootIdentity,
) -> bool {
    let mut registry = ARTIFACT_SCOPE_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if registry
        .active_scope_roots
        .get(scope_dir)
        .is_some_and(|existing| !existing.same_owner(root))
        || registry
            .active_alias_roots
            .get(alias_root)
            .is_some_and(|(existing, _)| !existing.same_owner(root))
    {
        return false;
    }
    let count = registry
        .active_scopes
        .entry(scope_dir.to_path_buf())
        .or_insert(0);
    *count = count.saturating_add(1);
    registry
        .active_scope_roots
        .insert(scope_dir.to_path_buf(), root.clone());
    let alias = registry
        .active_alias_roots
        .entry(alias_root.to_path_buf())
        .or_insert_with(|| (root.clone(), 0));
    alias.1 = alias.1.saturating_add(1);
    true
}

fn unregister_artifact_scope(scope_dir: &Path, alias_root: &Path) {
    let mut registry = ARTIFACT_SCOPE_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match registry.active_scopes.get_mut(scope_dir) {
        Some(count) if *count > 1 => *count = count.saturating_sub(1),
        Some(_) => {
            registry.active_scopes.remove(scope_dir);
            registry.active_scope_roots.remove(scope_dir);
        }
        None => {}
    }

    match registry.active_alias_roots.get_mut(alias_root) {
        Some((_, count)) if *count > 1 => *count = count.saturating_sub(1),
        Some(_) => {
            registry.active_alias_roots.remove(alias_root);
        }
        None => {}
    }

    let ready: Vec<CleanupRequestKey> = registry
        .pending_cleanup_roots
        .iter()
        .filter(|(_, debt)| {
            !registry
                .active_scopes
                .keys()
                .any(|active_scope| active_scope.starts_with(&debt.path))
        })
        .map(|(key, _)| key.clone())
        .collect();
    for key in ready {
        let Some(debt) = registry.pending_cleanup_roots.get(&key).cloned() else {
            continue;
        };
        match remove_artifact_scope(&debt) {
            Ok(()) => {
                registry.pending_cleanup_roots.remove(&key);
            }
            Err(error) => {
                tracing::warn!(
                    path = %debt.path.display(),
                    %error,
                    "deferred tool artifact cleanup failed"
                );
            }
        }
    }
}

fn remove_artifact_scope(debt: &CleanupDebt) -> io::Result<()> {
    debt.root.verify()?;
    if normalize_artifact_root(&debt.path)? != debt.path {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tool artifact scope identity changed before deletion",
        ));
    }
    match fs::remove_dir_all(&debt.path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn artifact_root_alias(root: &Path) -> io::Result<PathBuf> {
    if root.is_absolute() {
        Ok(root.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(root))
    }
}

fn normalize_artifact_root(root: &Path) -> io::Result<PathBuf> {
    let absolute = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()?.join(root)
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(segment) => {
                let candidate = resolved.join(segment);
                match fs::canonicalize(&candidate) {
                    Ok(path) => resolved = path,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        match fs::symlink_metadata(&candidate) {
                            Err(missing) if missing.kind() == io::ErrorKind::NotFound => {
                                resolved = candidate;
                            }
                            _ => return Err(error),
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(resolved)
}

pub fn artifact_scope_component(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars().take(64) {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            output.push(character);
        } else if !output.ends_with('_') {
            output.push('_');
        }
    }
    let trimmed = output.trim_matches('_');
    let prefix = if trimmed.is_empty() { "scope" } else { trimmed };
    let hash = format!("{:x}", Sha256::digest(value.as_bytes()))
        .chars()
        .take(12)
        .collect::<String>();
    if hash.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}-{hash}")
    }
}

fn cleanup_artifacts_older_than(
    root: &Path,
    max_age: Duration,
    active_scopes: &HashMap<PathBuf, usize>,
    expected_root: Option<&ArtifactRootIdentity>,
) {
    if expected_root.is_some_and(|expected| expected.verify().is_err()) {
        return;
    }
    if !normalize_artifact_root(root).is_ok_and(|resolved| resolved == root) {
        return;
    }
    let cutoff = SystemTime::now().checked_sub(max_age);
    let Some(cutoff) = cutoff else {
        return;
    };
    cleanup_directory(root, cutoff, active_scopes);
}

fn cleanup_directory(
    directory: &Path,
    cutoff: SystemTime,
    active_scopes: &HashMap<PathBuf, usize>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if active_scopes.keys().any(|scope| path.starts_with(scope)) {
            continue;
        }
        let contains_active_scope = active_scopes.keys().any(|scope| scope.starts_with(&path));
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            let is_expired = fs::symlink_metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .is_some_and(|modified| modified < cutoff);
            if is_expired {
                let _ = fs::remove_file(path);
            }
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if file_type.is_dir() {
            cleanup_directory(&path, cutoff, active_scopes);
            let is_expired = metadata
                .modified()
                .ok()
                .is_some_and(|modified| modified < cutoff);
            if is_expired && !contains_active_scope {
                let _ = fs::remove_dir(&path);
            }
        } else if file_type.is_file()
            && metadata
                .modified()
                .ok()
                .is_some_and(|modified| modified < cutoff)
        {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "echo-core-artifact-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn writer_keeps_small_output_inline() -> io::Result<()> {
        let root = test_root("inline");
        let config = ToolOutputArtifactConfig::new(&root, "test").threshold_bytes(32);
        let identity = ToolOutputArtifactIdentity {
            conversation_id: Some("conv-1".to_string()),
            run_id: Some("run-1".to_string()),
            call_id: "call-1".to_string(),
            tool_name: "shell".to_string(),
        };
        let mut writer = ToolOutputArtifactWriter::new(config, identity);
        writer.push_raw("short")?;
        assert!(writer.finish()?.is_none());
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn writer_spills_complete_multichannel_output() -> io::Result<()> {
        let root = test_root("multichannel");
        let config = ToolOutputArtifactConfig::new(&root, "conversation")
            .threshold_bytes(8)
            .max_age_secs(Some(DEFAULT_TEMP_ARTIFACT_MAX_AGE_SECS));
        let identity = ToolOutputArtifactIdentity {
            conversation_id: Some("conv/unsafe".to_string()),
            run_id: Some("run-1".to_string()),
            call_id: "call-1".to_string(),
            tool_name: "shell".to_string(),
        };
        let mut writer = ToolOutputArtifactWriter::new(config.clone(), identity);
        writer.push_channel(ToolOutputChannel::Stdout, "hello")?;
        writer.push_channel(ToolOutputChannel::Stderr, "world")?;
        let artifact = writer
            .finish()?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "expected spilled artifact"))?;
        let content = fs::read_to_string(&artifact.path)?;
        assert!(content.contains("hello"));
        assert!(content.contains("world"));
        assert_eq!(artifact.payload_bytes, 10);
        assert!(!artifact.sha256.is_empty());
        cleanup_tool_output_scope(&config, "conv/unsafe", None)?;
        assert!(!artifact.path.exists());
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn concurrent_spills_do_not_remove_active_scope_directories() -> io::Result<()> {
        let root = test_root("concurrent");
        let config = ToolOutputArtifactConfig::new(&root, "temporary")
            .threshold_bytes(4)
            .max_age_secs(Some(DEFAULT_TEMP_ARTIFACT_MAX_AGE_SECS));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let mut handles = Vec::new();

        for index in 0..8 {
            let config = config.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || -> io::Result<PathBuf> {
                barrier.wait();
                let identity = ToolOutputArtifactIdentity {
                    conversation_id: Some("shared-conversation".to_string()),
                    run_id: Some("shared-run".to_string()),
                    call_id: format!("call-{index}"),
                    tool_name: "shell".to_string(),
                };
                persist_tool_output(config, identity, "complete output")?
                    .map(|artifact| artifact.path)
                    .ok_or_else(|| io::Error::other("expected spilled artifact"))
            }));
        }

        for handle in handles {
            let path = handle
                .join()
                .map_err(|_| io::Error::other("artifact writer thread panicked"))??;
            assert!(path.exists(), "artifact should remain available: {path:?}");
        }

        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn explicit_cleanup_waits_for_active_writer() -> io::Result<()> {
        let root = test_root("deferred-cleanup");
        let config = ToolOutputArtifactConfig::new(&root, "conversation").threshold_bytes(4);
        let identity = ToolOutputArtifactIdentity {
            conversation_id: Some("conv-active".to_string()),
            run_id: Some("run-active".to_string()),
            call_id: "call-active".to_string(),
            tool_name: "shell".to_string(),
        };
        let mut writer = ToolOutputArtifactWriter::new(config.clone(), identity);
        writer.push_raw("complete output")?;
        let partial_path = writer.partial_path.clone();
        assert!(partial_path.exists());

        let pending = cleanup_tool_output_scope(&config, "conv-active", None)
            .err()
            .ok_or_else(|| io::Error::other("cleanup falsely reported settlement"))?;
        assert_eq!(pending.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(
            tool_output_scope_cleanup_state(&config, "conv-active", None),
            ToolOutputScopeCleanupState::Pending
        );
        assert!(
            partial_path.exists(),
            "active partial output must not be deleted"
        );

        let artifact = writer
            .finish()?
            .ok_or_else(|| io::Error::other("expected spilled artifact"))?;
        assert!(
            !artifact.path.exists(),
            "deferred conversation cleanup must run after the writer finishes"
        );
        assert_eq!(
            tool_output_scope_cleanup_state(&config, "conv-active", None),
            ToolOutputScopeCleanupState::Settled
        );
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn failed_deferred_cleanup_retains_retryable_scope() -> io::Result<()> {
        let root = test_root("retry-deferred-cleanup");
        let config = ToolOutputArtifactConfig::new(&root, "conversation");
        let scope = root.join(artifact_scope_component("conv-retry"));
        let writer = ToolOutputArtifactWriter::new(
            config.clone(),
            ToolOutputArtifactIdentity {
                conversation_id: Some("conv-retry".to_string()),
                run_id: None,
                call_id: "call-retry".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        fs::create_dir_all(&root)?;
        fs::write(&scope, "blocked directory")?;
        let error = cleanup_tool_output_scope(&config, "conv-retry", None)
            .err()
            .ok_or_else(|| io::Error::other("cleanup falsely reported settlement"))?;
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(writer);
        assert_eq!(
            tool_output_scope_cleanup_state(&config, "conv-retry", None),
            ToolOutputScopeCleanupState::Retryable
        );
        fs::remove_file(&scope)?;
        cleanup_tool_output_scope(&config, "conv-retry", None)?;
        assert_eq!(
            tool_output_scope_cleanup_state(&config, "conv-retry", None),
            ToolOutputScopeCleanupState::Settled
        );
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn age_sweep_preserves_active_writer_file() -> io::Result<()> {
        let root = test_root("active-age-sweep");
        let config = ToolOutputArtifactConfig::new(&root, "temporary")
            .threshold_bytes(4)
            .max_age_secs(Some(0));
        let mut active = ToolOutputArtifactWriter::new(
            config.clone(),
            ToolOutputArtifactIdentity {
                conversation_id: Some("active".to_string()),
                run_id: None,
                call_id: "call-active".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        active.push_raw("active output")?;
        let partial = active.partial_path.clone();
        File::open(&partial)?.set_modified(SystemTime::UNIX_EPOCH)?;
        let mut trigger = ToolOutputArtifactWriter::new(
            config,
            ToolOutputArtifactIdentity {
                conversation_id: Some("trigger".to_string()),
                run_id: None,
                call_id: "call-trigger".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        trigger.push_raw("trigger output")?;
        assert!(partial.exists(), "age sweep removed a live writer's file");
        drop(trigger);
        drop(active);
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn aliased_root_cannot_delete_active_writer_scope() -> io::Result<()> {
        let root = test_root("scope-alias");
        fs::create_dir_all(root.join("detour"))?;
        let canonical_root = root.join("artifacts");
        let aliased_root = root.join("detour").join("..").join("artifacts");
        let writer_config =
            ToolOutputArtifactConfig::new(&aliased_root, "conversation").threshold_bytes(4);
        let cleanup_config =
            ToolOutputArtifactConfig::new(&canonical_root, "conversation").threshold_bytes(4);
        let mut writer = ToolOutputArtifactWriter::new(
            writer_config,
            ToolOutputArtifactIdentity {
                conversation_id: Some("same-scope".to_string()),
                run_id: None,
                call_id: "call-1".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        writer.push_raw("active output")?;
        let partial = writer.partial_path.clone();
        let error = cleanup_tool_output_scope(&cleanup_config, "same-scope", None)
            .err()
            .ok_or_else(|| io::Error::other("aliased cleanup deleted an active writer"))?;
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(partial.exists());
        drop(writer);
        assert_eq!(
            tool_output_scope_cleanup_state(&cleanup_config, "same-scope", None),
            ToolOutputScopeCleanupState::Settled
        );
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_root_uses_same_scope_owner_as_canonical_root() -> io::Result<()> {
        let root = test_root("symlinked-scope");
        fs::create_dir_all(&root)?;
        std::os::unix::fs::symlink(&root, root.join("alias"))?;
        let alias = ToolOutputArtifactConfig::new(root.join("alias/artifacts"), "conversation")
            .threshold_bytes(4);
        let canonical = ToolOutputArtifactConfig::new(root.join("artifacts"), "conversation")
            .threshold_bytes(4);
        let mut writer = ToolOutputArtifactWriter::new(
            alias,
            ToolOutputArtifactIdentity {
                conversation_id: Some("same-scope".to_string()),
                run_id: None,
                call_id: "call-1".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        writer.push_raw("active output")?;
        let error = cleanup_tool_output_scope(&canonical, "same-scope", None)
            .err()
            .ok_or_else(|| io::Error::other("symlink alias missed active writer"))?;
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(writer);
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn missing_root_replaced_by_symlink_cannot_redirect_writer() -> io::Result<()> {
        let root = test_root("missing-root-swap");
        let outside = root.join("outside");
        fs::create_dir_all(&outside)?;
        let configured = root.join("artifacts");
        let config = ToolOutputArtifactConfig::new(&configured, "conversation").threshold_bytes(4);
        let mut writer = ToolOutputArtifactWriter::new(
            config.clone(),
            ToolOutputArtifactIdentity {
                conversation_id: Some("conv".to_string()),
                run_id: None,
                call_id: "call".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        let original = root.join("original-root");
        fs::rename(&configured, &original)?;
        std::os::unix::fs::symlink(&outside, &configured)?;
        let error = writer
            .push_raw("active output")
            .err()
            .ok_or_else(|| io::Error::other("writer followed a replaced root"))?;
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read_dir(&outside)?.count(), 0);
        let pending = cleanup_tool_output_scope(&config, "conv", None)
            .err()
            .ok_or_else(|| io::Error::other("repointed alias hid an active writer"))?;
        assert_eq!(pending.kind(), io::ErrorKind::WouldBlock);
        drop(writer);
        assert_eq!(
            tool_output_scope_cleanup_state(&config, "conv", None),
            ToolOutputScopeCleanupState::Retryable
        );
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn same_path_replacement_cannot_receive_active_writer_output() -> io::Result<()> {
        let root = test_root("same-path-writer-replacement");
        let configured = root.join("artifacts");
        let config = ToolOutputArtifactConfig::new(&configured, "conversation").threshold_bytes(4);
        let mut writer = ToolOutputArtifactWriter::new(
            config,
            ToolOutputArtifactIdentity {
                conversation_id: Some("conv".to_string()),
                run_id: None,
                call_id: "call".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        fs::rename(&configured, root.join("original"))?;
        fs::create_dir_all(&configured)?;
        let error = writer
            .push_raw("active output")
            .err()
            .ok_or_else(|| io::Error::other("replacement root received writer output"))?;
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read_dir(&configured)?.count(), 0);
        drop(writer);
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn deferred_cleanup_keeps_replacement_root_and_retryable_debt() -> io::Result<()> {
        let root = test_root("deferred-root-replacement");
        let configured = root.join("artifacts");
        let config = ToolOutputArtifactConfig::new(&configured, "conversation").threshold_bytes(4);
        let mut writer = ToolOutputArtifactWriter::new(
            config.clone(),
            ToolOutputArtifactIdentity {
                conversation_id: Some("conv".to_string()),
                run_id: None,
                call_id: "call".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        writer.push_raw("active output")?;
        assert_eq!(
            cleanup_tool_output_scope(&config, "conv", None)
                .err()
                .ok_or_else(|| io::Error::other("active writer was not protected"))?
                .kind(),
            io::ErrorKind::WouldBlock
        );
        fs::rename(&configured, root.join("original"))?;
        let replacement_scope = configured.join(artifact_scope_component("conv"));
        fs::create_dir_all(&replacement_scope)?;
        let sentinel = replacement_scope.join("keep.txt");
        fs::write(&sentinel, "keep")?;
        drop(writer);
        assert_eq!(fs::read_to_string(&sentinel)?, "keep");
        assert_eq!(
            tool_output_scope_cleanup_state(&config, "conv", None),
            ToolOutputScopeCleanupState::Retryable
        );
        assert!(cleanup_tool_output_scope(&config, "conv", None).is_err());
        assert_eq!(fs::read_to_string(&sentinel)?, "keep");
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_after_alias_repoint_keeps_new_target_while_writer_active() -> io::Result<()> {
        let root = test_root("active-alias-repoint");
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first)?;
        fs::create_dir_all(&second)?;
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&first, &alias)?;
        let config = ToolOutputArtifactConfig::new(alias.join("artifacts"), "conversation")
            .threshold_bytes(4);
        let mut writer = ToolOutputArtifactWriter::new(
            config.clone(),
            ToolOutputArtifactIdentity {
                conversation_id: Some("conv".to_string()),
                run_id: None,
                call_id: "call".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        writer.push_raw("active output")?;
        fs::remove_file(&alias)?;
        std::os::unix::fs::symlink(&second, &alias)?;
        let replacement_scope = second
            .join("artifacts")
            .join(artifact_scope_component("conv"));
        fs::create_dir_all(&replacement_scope)?;
        let sentinel = replacement_scope.join("keep.txt");
        fs::write(&sentinel, "keep")?;
        let pending = cleanup_tool_output_scope(&config, "conv", None)
            .err()
            .ok_or_else(|| io::Error::other("alias repoint deleted another owner's scope"))?;
        assert_eq!(pending.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(fs::read_to_string(&sentinel)?, "keep");
        drop(writer);
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_after_writer_release_uses_current_alias_without_pending_debt() -> io::Result<()> {
        let root = test_root("released-alias-repoint");
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first)?;
        fs::create_dir_all(&second)?;
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&first, &alias)?;
        let config = ToolOutputArtifactConfig::new(alias.join("artifacts"), "conversation")
            .threshold_bytes(4);
        let mut writer = ToolOutputArtifactWriter::new(
            config.clone(),
            ToolOutputArtifactIdentity {
                conversation_id: Some("conv".to_string()),
                run_id: None,
                call_id: "call".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        writer.push_raw("old output")?;
        let old_artifact = writer
            .finish()?
            .ok_or_else(|| io::Error::other("expected old artifact"))?;
        fs::remove_file(&alias)?;
        std::os::unix::fs::symlink(&second, &alias)?;
        let current_scope = second
            .join("artifacts")
            .join(artifact_scope_component("conv"));
        fs::create_dir_all(&current_scope)?;
        fs::write(current_scope.join("current.txt"), "current")?;

        cleanup_tool_output_scope(&config, "conv", None)?;
        assert!(!current_scope.exists());
        assert!(old_artifact.path.exists());
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn age_sweep_recognizes_active_writer_through_root_alias() -> io::Result<()> {
        let root = test_root("age-alias");
        fs::create_dir_all(root.join("detour"))?;
        let aliased = ToolOutputArtifactConfig::new(root.join("detour/../artifacts"), "temporary")
            .threshold_bytes(4)
            .max_age_secs(Some(0));
        let canonical = ToolOutputArtifactConfig::new(root.join("artifacts"), "temporary")
            .threshold_bytes(4)
            .max_age_secs(Some(0));
        let mut active = ToolOutputArtifactWriter::new(
            aliased,
            ToolOutputArtifactIdentity {
                conversation_id: Some("active".to_string()),
                run_id: None,
                call_id: "call-1".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        active.push_raw("active output")?;
        let partial = active.partial_path.clone();
        File::open(&partial)?.set_modified(SystemTime::UNIX_EPOCH)?;
        let mut trigger = ToolOutputArtifactWriter::new(
            canonical,
            ToolOutputArtifactIdentity {
                conversation_id: Some("trigger".to_string()),
                run_id: None,
                call_id: "call-2".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        trigger.push_raw("trigger output")?;
        assert!(
            partial.exists(),
            "aliased sweep removed a live partial file"
        );
        drop(trigger);
        drop(active);
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn async_finish_without_tokio_runtime_returns_error() -> io::Result<()> {
        let root = test_root("finish-no-runtime");
        let config = ToolOutputArtifactConfig::new(&root, "conversation").threshold_bytes(4);
        let mut writer = ToolOutputArtifactWriter::new(
            config,
            ToolOutputArtifactIdentity {
                conversation_id: Some("conv".to_string()),
                run_id: None,
                call_id: "call".to_string(),
                tool_name: "shell".to_string(),
            },
        );
        writer.push_raw("complete output")?;
        let result = futures::executor::block_on(writer.finish_async());
        assert!(result.is_err());
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn cancelled_async_finish_keeps_scope_until_queued_io_settles() -> io::Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let root = test_root("cancelled-async-finish");
            let config = ToolOutputArtifactConfig::new(&root, "conversation").threshold_bytes(4);
            let mut writer = ToolOutputArtifactWriter::new(
                config.clone(),
                ToolOutputArtifactIdentity {
                    conversation_id: Some("conv-cancel".to_string()),
                    run_id: None,
                    call_id: "call-cancel".to_string(),
                    tool_name: "shell".to_string(),
                },
            );
            writer.push_raw("complete output")?;
            let partial = writer.partial_path.clone();

            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = started_tx.send(());
                let _ = release_rx.blocking_recv();
            });
            started_rx.await.map_err(io::Error::other)?;

            let mut finish = Box::pin(writer.finish_async());
            assert!(matches!(
                futures::poll!(&mut finish),
                std::task::Poll::Pending
            ));
            drop(finish);
            let pending = cleanup_tool_output_scope(&config, "conv-cancel", None)
                .err()
                .ok_or_else(|| io::Error::other("queued writer was released early"))?;
            assert_eq!(pending.kind(), io::ErrorKind::WouldBlock);
            assert!(partial.exists());

            let _ = release_tx.send(());
            blocker.await.map_err(io::Error::other)?;
            for _ in 0..100 {
                if tool_output_scope_cleanup_state(&config, "conv-cancel", None)
                    == ToolOutputScopeCleanupState::Settled
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(
                tool_output_scope_cleanup_state(&config, "conv-cancel", None),
                ToolOutputScopeCleanupState::Settled
            );
            assert!(!partial.exists());
            let _ = fs::remove_dir_all(root);
            Ok(())
        })
    }

    #[cfg(unix)]
    #[test]
    fn age_cleanup_does_not_follow_symlinks_outside_artifact_root() -> io::Result<()> {
        let root = test_root("symlink-cleanup");
        let outside = test_root("symlink-outside");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(&outside)?;
        let outside_file = outside.join("keep.txt");
        fs::write(&outside_file, "must remain")?;
        std::os::unix::fs::symlink(&outside, root.join("external-link"))?;

        cleanup_artifacts_older_than(&root, Duration::ZERO, &HashMap::new(), None);

        assert!(outside_file.exists(), "cleanup must not traverse symlinks");
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
        Ok(())
    }
}
