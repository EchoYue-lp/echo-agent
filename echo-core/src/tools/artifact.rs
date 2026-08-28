//! Durable, bounded tool-output artifacts.
//!
//! Streaming tools can keep a small in-memory projection while this writer
//! spills the complete text payload to disk once the configured threshold is
//! crossed. Applications choose the root directory and retention policy.

use super::{ToolContext, ToolOutputChannel};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

pub const DEFAULT_TOOL_OUTPUT_ARTIFACT_THRESHOLD_BYTES: usize = 1024 * 1024;
pub const DEFAULT_TEMP_ARTIFACT_MAX_AGE_SECS: u64 = 60 * 60;

#[derive(Default)]
struct ArtifactScopeRegistry {
    active_scopes: HashMap<PathBuf, usize>,
    pending_cleanup_roots: HashSet<PathBuf>,
}

static ARTIFACT_SCOPE_REGISTRY: LazyLock<Mutex<ArtifactScopeRegistry>> =
    LazyLock::new(|| Mutex::new(ArtifactScopeRegistry::default()));

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
    pub fn new(config: ToolOutputArtifactConfig, identity: ToolOutputArtifactIdentity) -> Self {
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
        register_artifact_scope(&directory);
        Self {
            config,
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

    /// Finalize without blocking the async runtime thread on flush, sync, or rename.
    ///
    /// If the future is cancelled before publication, `Drop` removes the
    /// partial file and releases the artifact scope.
    pub async fn finish_async(mut self) -> io::Result<Option<ToolOutputArtifactRef>> {
        use tokio::io::AsyncWriteExt;

        if self.payload_bytes < u64::try_from(self.config.threshold_bytes).unwrap_or(u64::MAX) {
            self.completed = true;
            return Ok(None);
        }
        self.ensure_file()?;
        if let Some(file) = self.file.take() {
            let mut file = tokio::fs::File::from_std(file);
            file.flush().await?;
            file.sync_all().await?;
        }
        tokio::fs::rename(&self.partial_path, &self.final_path).await?;
        self.completed = true;
        Ok(Some(ToolOutputArtifactRef {
            path: self.final_path.clone(),
            artifact_bytes: self.artifact_bytes,
            payload_bytes: self.payload_bytes,
            sha256: format!("{:x}", self.hasher.clone().finalize()),
            retention: self.config.retention.clone(),
        }))
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
        if let Some(max_age_secs) = self.config.max_age_secs {
            cleanup_artifacts_older_than(&self.config.root_dir, Duration::from_secs(max_age_secs));
        }
        fs::create_dir_all(directory)?;
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
        if !self.completed && self.partial_path.exists() {
            let _ = fs::remove_file(&self.partial_path);
        }
        unregister_artifact_scope(&self.scope_dir);
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
    let mut path = config
        .root_dir
        .join(artifact_scope_component(conversation_id));
    if let Some(run_id) = run_id {
        path = path.join(artifact_scope_component(run_id));
    }
    let mut registry = ARTIFACT_SCOPE_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if registry
        .active_scopes
        .keys()
        .any(|active_scope| active_scope.starts_with(&path))
    {
        registry.pending_cleanup_roots.insert(path);
        return Ok(());
    }
    remove_artifact_scope(&path)
}

fn register_artifact_scope(scope_dir: &Path) {
    let mut registry = ARTIFACT_SCOPE_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let count = registry
        .active_scopes
        .entry(scope_dir.to_path_buf())
        .or_insert(0);
    *count = count.saturating_add(1);
}

fn unregister_artifact_scope(scope_dir: &Path) {
    let mut registry = ARTIFACT_SCOPE_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match registry.active_scopes.get_mut(scope_dir) {
        Some(count) if *count > 1 => *count = count.saturating_sub(1),
        Some(_) => {
            registry.active_scopes.remove(scope_dir);
        }
        None => {}
    }

    let ready: Vec<PathBuf> = registry
        .pending_cleanup_roots
        .iter()
        .filter(|cleanup_root| {
            !registry
                .active_scopes
                .keys()
                .any(|active_scope| active_scope.starts_with(cleanup_root.as_path()))
        })
        .cloned()
        .collect();
    for cleanup_root in ready {
        registry.pending_cleanup_roots.remove(&cleanup_root);
        if let Err(error) = remove_artifact_scope(&cleanup_root) {
            tracing::warn!(
                path = %cleanup_root.display(),
                %error,
                "deferred tool artifact cleanup failed"
            );
        }
    }
}

fn remove_artifact_scope(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
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

fn cleanup_artifacts_older_than(root: &Path, max_age: Duration) {
    let cutoff = SystemTime::now().checked_sub(max_age);
    let Some(cutoff) = cutoff else {
        return;
    };
    cleanup_directory(root, cutoff);
}

fn cleanup_directory(directory: &Path, cutoff: SystemTime) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
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
            cleanup_directory(&path, cutoff);
            let is_expired = metadata
                .modified()
                .ok()
                .is_some_and(|modified| modified < cutoff);
            if is_expired {
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

        cleanup_tool_output_scope(&config, "conv-active", None)?;
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
        let _ = fs::remove_dir_all(root);
        Ok(())
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

        cleanup_artifacts_older_than(&root, Duration::ZERO);

        assert!(outside_file.exists(), "cleanup must not traverse symlinks");
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
        Ok(())
    }
}
