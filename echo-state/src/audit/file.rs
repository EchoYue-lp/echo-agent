//! 文件审计日志记录器
//!
//! 将审计事件以 JSON-lines 格式写入文件，支持按过滤条件查询。

use echo_core::audit::{AuditEvent, AuditFilter, AuditLogger};
use echo_core::error::Result;
use futures::future::BoxFuture;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct FileAuditState {
    file_guard: echo_core::utils::fs::ExistingRegularFileGuard,
    committed_len: u64,
}

/// 文件审计日志记录器
///
/// 每个事件序列化为一行 JSON 追加写入文件。同一路径只有一个 live logger
/// authority；需要在多个调用方间共享时使用 `Arc<FileAuditLogger>`。
pub struct FileAuditLogger {
    path: PathBuf,
    _lease: echo_core::utils::fs::ExclusiveFileLease,
    state: Mutex<FileAuditState>,
    retention: echo_core::utils::retention::ContentRetentionPolicy,
}

impl FileAuditLogger {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        echo_core::utils::fs::create_dir_all_durable(parent)?;
        let canonical_parent = std::fs::canonicalize(parent)?;
        let file_name = path.file_name().ok_or_else(|| {
            echo_core::error::ReactError::Other(format!(
                "audit log path has no file name: {}",
                path.display()
            ))
        })?;
        let path = canonical_parent.join(file_name);
        let lease = echo_core::utils::fs::try_exclusive_file_lease(&path)?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                file.sync_all()?;
                echo_core::utils::fs::create_dir_all_durable(&canonical_parent)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let file_guard = echo_core::utils::fs::open_existing_regular_guard(&path)?;
        let content = echo_core::utils::fs::read_existing_matching(&path, &file_guard)?;
        Self::repair_torn_tail(&path, &file_guard, &content)?;
        let committed_len =
            echo_core::utils::fs::matching_existing_regular_len(&path, &file_guard)?;
        Ok(Self {
            path,
            _lease: lease,
            state: Mutex::new(FileAuditState {
                file_guard,
                committed_len,
            }),
            retention: echo_core::utils::retention::ContentRetentionPolicy::default(),
        })
    }

    /// Recover only a crash-torn final JSONL record. Complete corrupt records
    /// remain visible as errors and are never skipped.
    fn repair_torn_tail(
        path: &Path,
        file_guard: &echo_core::utils::fs::ExistingRegularFileGuard,
        content: &[u8],
    ) -> Result<()> {
        let observed_len = checked_record_end(0, content.len())?;
        let mut committed_len = 0_u64;
        for (line_index, segment) in content.split_inclusive(|byte| *byte == b'\n').enumerate() {
            let complete = segment.last().is_some_and(|byte| *byte == b'\n');
            let line = if complete {
                segment.strip_suffix(b"\n").unwrap_or(segment)
            } else {
                segment
            };

            if line.iter().all(|byte| byte.is_ascii_whitespace()) {
                if complete {
                    committed_len = checked_record_end(committed_len, segment.len())?;
                }
                continue;
            }

            match serde_json::from_slice::<AuditEvent>(line) {
                Ok(_) if complete => {
                    committed_len = checked_record_end(committed_len, segment.len())?;
                }
                Ok(_) => {
                    echo_core::utils::fs::append_existing_matching(
                        path,
                        file_guard,
                        observed_len,
                        b"\n",
                        echo_core::utils::fs::FileDurability::SyncData,
                    )?;
                }
                Err(error) if !complete => {
                    tracing::warn!(
                        path = %path.display(),
                        line = line_index.saturating_add(1),
                        error = %error,
                        tail_preview = %String::from_utf8_lossy(line).chars().take(120).collect::<String>(),
                        "audit log: truncating crash-torn final JSONL record"
                    );
                    echo_core::utils::fs::truncate_existing_matching(
                        path,
                        file_guard,
                        observed_len,
                        committed_len,
                        echo_core::utils::fs::FileDurability::SyncData,
                    )?;
                }
                Err(error) => {
                    return Err(echo_core::error::ReactError::Other(format!(
                        "audit log {} has corrupt complete record at line {}: {error}",
                        path.display(),
                        line_index.saturating_add(1)
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn with_retention_policy(
        mut self,
        retention: echo_core::utils::retention::ContentRetentionPolicy,
    ) -> Self {
        self.retention = retention;
        self
    }
}

fn checked_record_end(current: u64, segment_len: usize) -> Result<u64> {
    let segment_len = u64::try_from(segment_len).map_err(|error| {
        echo_core::error::ReactError::Other(format!("audit record length overflow: {error}"))
    })?;
    current
        .checked_add(segment_len)
        .ok_or_else(|| echo_core::error::ReactError::Other("audit log length overflow".to_string()))
}

impl AuditLogger for FileAuditLogger {
    fn log<'a>(&'a self, event: AuditEvent) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut value = serde_json::to_value(&event)
                .map_err(|e| echo_core::error::ReactError::Other(e.to_string()))?;
            self.retention.sanitize_json(&mut value);
            let line = serde_json::to_string(&value)
                .map_err(|e| echo_core::error::ReactError::Other(e.to_string()))?;
            let mut bytes = line.into_bytes();
            bytes.push(b'\n');

            // Keep path identity and length in the same lock as the append. If
            // SyncData has an unknown outcome, committed_len does not advance;
            // this live handle then fails closed until verified reopen.
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let next_len = checked_record_end(state.committed_len, bytes.len())?;
            echo_core::utils::fs::append_existing_matching(
                &self.path,
                &state.file_guard,
                state.committed_len,
                &bytes,
                echo_core::utils::fs::FileDurability::SyncData,
            )?;
            state.committed_len = next_len;
            Ok(())
        })
    }

    fn query<'a>(&'a self, filter: AuditFilter) -> BoxFuture<'a, Result<Vec<AuditEvent>>> {
        Box::pin(async move {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let bytes =
                echo_core::utils::fs::read_existing_matching(&self.path, &state.file_guard)?;
            let observed_len = checked_record_end(0, bytes.len())?;
            if observed_len != state.committed_len {
                return Err(echo_core::error::ReactError::Other(format!(
                    "audit log length changed from {} to {observed_len}; verified reopen required",
                    state.committed_len
                )));
            }
            let content = String::from_utf8(bytes).map_err(|error| {
                echo_core::error::ReactError::Other(format!(
                    "audit log is not valid UTF-8: {error}"
                ))
            })?;
            let mut events: Vec<AuditEvent> = Vec::new();

            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                {
                    let event = serde_json::from_str::<AuditEvent>(line).map_err(|error| {
                        echo_core::error::ReactError::Other(format!(
                            "invalid audit record: {error}"
                        ))
                    })?;
                    let mut keep = true;
                    if let Some(ref sid) = filter.session_id
                        && event.session_id.as_deref() != Some(sid)
                    {
                        keep = false;
                    }
                    if let Some(ref name) = filter.agent_name
                        && &event.agent_name != name
                    {
                        keep = false;
                    }
                    if let Some(ref from) = filter.from
                        && event.timestamp < *from
                    {
                        keep = false;
                    }
                    if let Some(ref to) = filter.to
                        && event.timestamp > *to
                    {
                        keep = false;
                    }
                    if keep {
                        events.push(event);
                    }
                }
            }

            if let Some(limit) = filter.limit {
                events.truncate(limit);
            }

            Ok(events)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::audit::AuditEventType;

    #[tokio::test]
    async fn durable_audit_redacts_nested_secrets_and_bounds_unicode() -> Result<()> {
        let temp = std::env::temp_dir().join(format!(
            "echo-audit-retention-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let path = temp.join("audit.jsonl");
        let logger = FileAuditLogger::new(&path)?.with_retention_policy(
            echo_core::utils::retention::ContentRetentionPolicy {
                max_string_chars: 64,
                max_array_items: 10,
            },
        );
        logger
            .log(AuditEvent::now(
                Some("session".to_string()),
                "agent".to_string(),
                AuditEventType::ToolCall {
                    tool: "shell".to_string(),
                    input: serde_json::json!({
                        "nested": {"auth": "Bearer abcdefghijklmnopqrstuvwxyz"}
                    }),
                    output: "中文字符".repeat(40),
                    success: true,
                    duration_ms: 1,
                },
            ))
            .await?;
        let bytes = std::fs::read_to_string(path)?;
        assert!(!bytes.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(bytes.contains("[REDACTED]"));
        assert!(bytes.contains("[TRUNCATED]"));
        let _ = std::fs::remove_dir_all(temp);
        Ok(())
    }

    #[tokio::test]
    async fn reopen_truncates_only_a_torn_final_record() -> Result<()> {
        let temp = std::env::temp_dir().join(format!(
            "echo-audit-torn-tail-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let path = temp.join("audit.jsonl");
        let logger = FileAuditLogger::new(&path)?;
        logger
            .log(AuditEvent::now(
                Some("session".to_string()),
                "agent".to_string(),
                AuditEventType::FinalAnswer {
                    content: "first".to_string(),
                },
            ))
            .await?;
        let durable = std::fs::read(&path)?;
        drop(logger);

        let mut torn = durable.clone();
        torn.extend_from_slice(b"{\"timestamp\":\"torn");
        std::fs::write(&path, torn)?;

        let reopened = FileAuditLogger::new(&path)?;
        assert_eq!(std::fs::read(&path)?, durable);
        assert_eq!(reopened.query(AuditFilter::default()).await?.len(), 1);
        let _ = std::fs::remove_dir_all(temp);
        Ok(())
    }

    #[tokio::test]
    async fn reopen_preserves_valid_final_record_and_adds_newline() -> Result<()> {
        let temp = std::env::temp_dir().join(format!(
            "echo-audit-valid-tail-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp)?;
        let path = temp.join("audit.jsonl");
        let event = AuditEvent::now(
            Some("session".to_string()),
            "agent".to_string(),
            AuditEventType::FinalAnswer {
                content: "complete".to_string(),
            },
        );
        let mut expected = serde_json::to_vec(&event)?;
        std::fs::write(&path, &expected)?;

        let reopened = FileAuditLogger::new(&path)?;
        expected.push(b'\n');
        assert_eq!(std::fs::read(&path)?, expected);
        assert_eq!(reopened.query(AuditFilter::default()).await?.len(), 1);
        let _ = std::fs::remove_dir_all(temp);
        Ok(())
    }

    #[tokio::test]
    async fn opening_existing_complete_file_never_truncates_it() -> Result<()> {
        let temp = std::env::temp_dir().join(format!(
            "echo-audit-existing-create-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp)?;
        let path = temp.join("audit.jsonl");
        let event = AuditEvent::now(
            Some("concurrent-creator".to_string()),
            "agent".to_string(),
            AuditEventType::FinalAnswer {
                content: "preserve me".to_string(),
            },
        );
        let mut expected = serde_json::to_vec(&event)?;
        expected.push(b'\n');
        std::fs::write(&path, &expected)?;

        let logger = FileAuditLogger::new(&path)?;

        assert_eq!(std::fs::read(&path)?, expected);
        assert_eq!(logger.query(AuditFilter::default()).await?.len(), 1);
        drop(logger);
        let _ = std::fs::remove_dir_all(temp);
        Ok(())
    }

    #[test]
    fn reopen_rejects_complete_corrupt_record_without_mutation() -> Result<()> {
        let temp = std::env::temp_dir().join(format!(
            "echo-audit-corrupt-record-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp)?;
        let path = temp.join("audit.jsonl");
        let bytes = b"{not-json}\n";
        std::fs::write(&path, bytes)?;

        let result = FileAuditLogger::new(&path);
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path)?, bytes);
        let _ = std::fs::remove_dir_all(temp);
        Ok(())
    }

    #[test]
    fn second_live_logger_cannot_race_recovery_or_append() -> Result<()> {
        let temp = std::env::temp_dir().join(format!(
            "echo-audit-exclusive-owner-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let path = temp.join("audit.jsonl");
        let logger = FileAuditLogger::new(&path)?;

        assert!(FileAuditLogger::new(&path).is_err());

        drop(logger);
        let reopened = FileAuditLogger::new(&path)?;
        drop(reopened);
        let _ = std::fs::remove_dir_all(temp);
        Ok(())
    }

    #[tokio::test]
    async fn replaced_live_path_fails_without_writing_replacement() -> Result<()> {
        let temp = std::env::temp_dir().join(format!(
            "echo-audit-path-replacement-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let path = temp.join("audit.jsonl");
        let moved = temp.join("audit-old.jsonl");
        let logger = FileAuditLogger::new(&path)?;
        std::fs::rename(&path, &moved)?;
        std::fs::write(&path, b"replacement\n")?;

        let result = logger
            .log(AuditEvent::now(
                Some("session".to_string()),
                "agent".to_string(),
                AuditEventType::FinalAnswer {
                    content: "must not escape".to_string(),
                },
            ))
            .await;

        assert!(result.is_err());
        assert_eq!(std::fs::read(&path)?, b"replacement\n");
        drop(logger);
        let _ = std::fs::remove_dir_all(temp);
        Ok(())
    }

    #[test]
    fn recovery_refuses_to_truncate_a_complete_replacement_file() -> Result<()> {
        let temp = std::env::temp_dir().join(format!(
            "echo-audit-recovery-replacement-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp)?;
        let path = temp.join("audit.jsonl");
        let old_path = temp.join("audit-old.jsonl");
        let first = AuditEvent::now(
            Some("session-a".to_string()),
            "agent".to_string(),
            AuditEventType::FinalAnswer {
                content: "old".to_string(),
            },
        );
        let mut old_bytes = serde_json::to_vec(&first)?;
        old_bytes.extend_from_slice(b"\n{\"timestamp\":\"torn");
        std::fs::write(&path, &old_bytes)?;
        let file_guard = echo_core::utils::fs::open_existing_regular_guard(&path)?;
        let scanned = echo_core::utils::fs::read_existing_matching(&path, &file_guard)?;

        std::fs::rename(&path, &old_path)?;
        let replacement = AuditEvent::now(
            Some("session-b".to_string()),
            "agent".to_string(),
            AuditEventType::FinalAnswer {
                content: "complete replacement".to_string(),
            },
        );
        let mut replacement_bytes = serde_json::to_vec(&replacement)?;
        replacement_bytes.push(b'\n');
        std::fs::write(&path, &replacement_bytes)?;

        let result = FileAuditLogger::repair_torn_tail(&path, &file_guard, &scanned);

        assert!(result.is_err());
        assert_eq!(std::fs::read(&path)?, replacement_bytes);
        drop(file_guard);
        let _ = std::fs::remove_dir_all(temp);
        Ok(())
    }
}
