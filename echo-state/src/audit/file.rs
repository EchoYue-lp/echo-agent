//! 文件审计日志记录器
//!
//! 将审计事件以 JSON-lines 格式写入文件，支持按过滤条件查询。

use echo_core::audit::{AuditEvent, AuditFilter, AuditLogger};
use echo_core::error::Result;
use futures::future::BoxFuture;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// 文件审计日志记录器
///
/// 每个事件序列化为一行 JSON 追加写入文件。
pub struct FileAuditLogger {
    path: PathBuf,
    writer: Mutex<Option<std::io::BufWriter<std::fs::File>>>,
    retention: echo_core::utils::retention::ContentRetentionPolicy,
}

impl FileAuditLogger {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Self {
            path,
            writer: Mutex::new(Some(std::io::BufWriter::new(file))),
            retention: echo_core::utils::retention::ContentRetentionPolicy::default(),
        })
    }

    pub fn with_retention_policy(
        mut self,
        retention: echo_core::utils::retention::ContentRetentionPolicy,
    ) -> Self {
        self.retention = retention;
        self
    }
}

impl AuditLogger for FileAuditLogger {
    fn log<'a>(&'a self, event: AuditEvent) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut value = serde_json::to_value(&event)
                .map_err(|e| echo_core::error::ReactError::Other(e.to_string()))?;
            self.retention.sanitize_json(&mut value);
            let line = serde_json::to_string(&value)
                .map_err(|e| echo_core::error::ReactError::Other(e.to_string()))?;

            // Recover from a poisoned lock (another thread panicked while holding it)
            // rather than silently dropping the audit event — the rest of the codebase
            // uses this same `into_inner()` recovery pattern. This ensures a security
            // audit record is never lost without a signal.
            let mut guard = self.writer.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(writer) = guard.as_mut() {
                writeln!(writer, "{}", line)?;
                writer.flush()?;
            }
            Ok(())
        })
    }

    fn query<'a>(&'a self, filter: AuditFilter) -> BoxFuture<'a, Result<Vec<AuditEvent>>> {
        Box::pin(async move {
            let content = std::fs::read_to_string(&self.path)?;
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
}
