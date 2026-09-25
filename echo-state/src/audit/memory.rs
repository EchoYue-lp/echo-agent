//! 内存审计日志记录器
//!
//! 将审计事件存储在内存中，适用于测试和实时查询。
//!
//! `InMemoryAuditLogger` sanitizes at its `log` boundary even when the
//! producer already supplied a retained copy. This makes the built-in sink a
//! regression oracle for custom logger implementations without rewriting
//! typed IDs used for diagnostic lookup.

use echo_core::audit::{AuditEvent, AuditFilter, AuditLogger};
use echo_core::error::Result;
use futures::future::BoxFuture;
use std::sync::RwLock;

/// 内存审计日志记录器
pub struct InMemoryAuditLogger {
    events: RwLock<Vec<AuditEvent>>,
    retention: echo_core::utils::retention::ContentRetentionPolicy,
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
            retention: echo_core::utils::retention::ContentRetentionPolicy::default(),
        }
    }

    pub fn with_retention_policy(
        mut self,
        retention: echo_core::utils::retention::ContentRetentionPolicy,
    ) -> Self {
        for event in self.events.get_mut().unwrap_or_else(|e| e.into_inner()) {
            event.apply_retention(&retention);
        }
        self.retention = retention;
        self
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
        let mut events = self.events.write().unwrap_or_else(|e| e.into_inner());
        events.clear();
    }
}

impl AuditLogger for InMemoryAuditLogger {
    fn log<'a>(&'a self, mut event: AuditEvent) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            event.apply_retention(&self.retention);
            let mut events = self.events.write().unwrap_or_else(|e| e.into_inner());
            events.push(event);
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

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::audit::AuditEventType;

    #[tokio::test]
    async fn poisoned_write_lock_recovers_log_query_snapshot_and_clear() -> Result<()> {
        let logger = InMemoryAuditLogger::new();
        let event = AuditEvent::now(
            Some("session-poison".to_string()),
            "agent".to_string(),
            AuditEventType::UserInput {
                content: "hello".to_string(),
            },
        );
        logger.log(event.clone()).await?;
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = logger.events.write().unwrap_or_else(|e| e.into_inner());
            std::panic::resume_unwind(Box::new("injected lock-holder unwind"));
        }));
        assert!(unwind.is_err());
        assert!(logger.events.is_poisoned());
        assert_eq!(logger.len(), 1);

        logger.log(event.clone()).await?;
        assert_eq!(logger.len(), 2);
        assert_eq!(logger.snapshot().len(), 2);
        assert_eq!(logger.query(AuditFilter::default()).await?.len(), 2);
        logger.clear();
        assert!(logger.is_empty());
        assert!(logger.snapshot().is_empty());
        assert!(logger.query(AuditFilter::default()).await?.is_empty());
        logger.log(event).await?;
        assert_eq!(logger.len(), 1);
        Ok(())
    }
}
