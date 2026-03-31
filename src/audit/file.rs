//! 文件审计日志记录器
//!
//! 将审计事件以 JSON-lines 格式写入文件，支持按过滤条件查询。

use super::{AuditEvent, AuditFilter, AuditLogger};
use crate::error::Result;
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
        })
    }
}

impl AuditLogger for FileAuditLogger {
    fn log<'a>(&'a self, event: AuditEvent) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let line = serde_json::to_string(&event)
                .map_err(|e| crate::error::ReactError::Other(e.to_string()))?;

            if let Ok(mut guard) = self.writer.lock() {
                if let Some(writer) = guard.as_mut() {
                    writeln!(writer, "{}", line)?;
                    writer.flush()?;
                }
            }
            Ok(())
        })
    }

    fn query<'a>(&'a self, filter: AuditFilter) -> BoxFuture<'a, Result<Vec<AuditEvent>>> {
        Box::pin(async move {
            let content = std::fs::read_to_string(&self.path).unwrap_or_default();
            let mut events: Vec<AuditEvent> = Vec::new();

            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(event) = serde_json::from_str::<AuditEvent>(line) {
                    let mut keep = true;
                    if let Some(ref sid) = filter.session_id {
                        if event.session_id.as_deref() != Some(sid) {
                            keep = false;
                        }
                    }
                    if let Some(ref name) = filter.agent_name {
                        if &event.agent_name != name {
                            keep = false;
                        }
                    }
                    if let Some(ref from) = filter.from {
                        if event.timestamp < *from {
                            keep = false;
                        }
                    }
                    if let Some(ref to) = filter.to {
                        if event.timestamp > *to {
                            keep = false;
                        }
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
