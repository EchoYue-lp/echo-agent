//! 内存审计日志记录器
//!
//! 将审计事件存储在内存中，适用于测试和实时查询。

use echo_core::audit::{AuditEvent, AuditFilter, AuditLogger};
use echo_core::error::Result;
use futures::future::BoxFuture;
use std::sync::RwLock;

/// 内存审计日志记录器
pub struct InMemoryAuditLogger {
    events: RwLock<Vec<AuditEvent>>,
}

impl Default for InMemoryAuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryAuditLogger {
    pub fn new() -> Self {
        Self {
            events: RwLock::new(Vec::new()),
        }
    }

    /// 获取所有事件的快照
    pub fn snapshot(&self) -> Vec<AuditEvent> {
        self.events
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 事件数量
    pub fn len(&self) -> usize {
        self.events.read().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 清空所有事件
    pub fn clear(&self) {
        if let Ok(mut events) = self.events.write() {
            events.clear();
        }
    }
}

impl AuditLogger for InMemoryAuditLogger {
    fn log<'a>(&'a self, event: AuditEvent) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if let Ok(mut events) = self.events.write() {
                events.push(event);
            }
            Ok(())
        })
    }

    fn query<'a>(&'a self, filter: AuditFilter) -> BoxFuture<'a, Result<Vec<AuditEvent>>> {
        Box::pin(async move {
            let all = self.events.read().unwrap_or_else(|e| e.into_inner());
            let mut result: Vec<AuditEvent> = all
                .iter()
                .filter(|e| {
                    if let Some(ref sid) = filter.session_id
                        && e.session_id.as_deref() != Some(sid)
                    {
                        return false;
                    }
                    if let Some(ref name) = filter.agent_name
                        && &e.agent_name != name
                    {
                        return false;
                    }
                    if let Some(ref from) = filter.from
                        && e.timestamp < *from
                    {
                        return false;
                    }
                    if let Some(ref to) = filter.to
                        && e.timestamp > *to
                    {
                        return false;
                    }
                    true
                })
                .cloned()
                .collect();

            if let Some(limit) = filter.limit {
                result.truncate(limit);
            }
            Ok(result)
        })
    }
}
