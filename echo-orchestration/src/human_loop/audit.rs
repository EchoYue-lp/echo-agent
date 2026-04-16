//! 权限审计追踪
//!
//! 记录 PermissionService 管线中每个决策的完整上下文，
//! 用于安全审计、合规报告、调试分析。
//!
//! ## 使用方式
//!
//! ```rust
//! use echo_orchestration::human_loop::{InMemoryPermissionAuditSink, PermissionService};
//! use std::sync::Arc;
//!
//! let sink = Arc::new(InMemoryPermissionAuditSink::new(1000));
//! let service = PermissionService::new()
//!     .with_audit_sink(sink.clone());
//!
//! // 执行后查询审计日志
//! let recent = sink.recent(10);
//! for entry in recent {
//!     println!(
//!         "{}ms {} → {} ({})",
//!         entry.elapsed_ms, entry.tool_name, entry.decision, entry.reason
//!     );
//! }
//! ```

use async_trait::async_trait;
use echo_core::tools::permission::PermissionDecision;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

// ── 审计条目 ──────────────────────────────────────────────────────────────────

/// 权限审计条目 — 记录一次权限检查的完整上下文
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionAuditEntry {
    /// 工具名称
    pub tool_name: String,
    /// 参数哈希（SHA-256 前 8 位）
    pub args_hash: String,
    /// 决策结果: "allow" | "deny" | "require_approval" | "ask"
    pub decision: String,
    /// 决策原因: "bypass" | "plan_mode" | "rule_match" | "protected_path" |
    ///           "cache_hit" | "denial_tracker" | "classifier" | "handler" |
    ///           "dont_ask" | "no_handler" | "hook"
    pub reason: String,
    /// 决策来源（管线阶段名）
    pub source: String,
    /// 决策耗时（微秒）
    pub duration_us: u64,
    /// 时间戳（从管线启动开始的偏移）
    pub elapsed_ms: u64,
}

impl PermissionAuditEntry {
    /// 创建审计条目
    pub fn new(
        tool_name: &str,
        tool_input: &serde_json::Value,
        decision: &PermissionDecision,
        reason: &str,
        source: &str,
        pipeline_start: Instant,
        decision_duration: Duration,
    ) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            args_hash: Self::hash_args(tool_input),
            decision: Self::decision_to_str(decision),
            reason: reason.to_string(),
            source: source.to_string(),
            duration_us: decision_duration.as_micros() as u64,
            elapsed_ms: pipeline_start.elapsed().as_millis() as u64,
        }
    }

    fn decision_to_str(decision: &PermissionDecision) -> String {
        match decision {
            PermissionDecision::Allow => "allow".to_string(),
            PermissionDecision::Deny { .. } => "deny".to_string(),
            PermissionDecision::RequireApproval => "require_approval".to_string(),
            PermissionDecision::Ask { .. } => "ask".to_string(),
        }
    }

    fn hash_args(input: &serde_json::Value) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        if let Ok(canonical) = serde_json::to_string(input) {
            canonical.hash(&mut hasher);
        }
        format!("{:08x}", hasher.finish())
    }
}

// ── 审计 Sink Trait ───────────────────────────────────────────────────────────

/// 权限审计 Sink — 接收审计条目的抽象接口
///
/// 内置实现：
/// - `InMemoryPermissionAuditSink`: 内存环形缓冲区
#[async_trait]
pub trait PermissionAuditSink: Send + Sync {
    /// 记录一条审计条目
    async fn record(&self, entry: PermissionAuditEntry);
}

// 为 Arc<T> 自动实现 PermissionAuditSink
#[async_trait]
impl<T: PermissionAuditSink + ?Sized> PermissionAuditSink for Arc<T> {
    async fn record(&self, entry: PermissionAuditEntry) {
        (**self).record(entry).await;
    }
}

// ── 内存审计 Sink ─────────────────────────────────────────────────────────────

/// 内存审计 Sink — 环形缓冲区，保留最近 N 条审计记录
///
/// 线程安全，使用 `std::sync::RwLock` 保护。
/// 适用于开发调试和内存受限的场景。
pub struct InMemoryPermissionAuditSink {
    entries: RwLock<VecDeque<PermissionAuditEntry>>,
    capacity: usize,
}

impl Clone for InMemoryPermissionAuditSink {
    fn clone(&self) -> Self {
        Self {
            entries: RwLock::new(self.entries.read().unwrap().clone()),
            capacity: self.capacity,
        }
    }
}

impl InMemoryPermissionAuditSink {
    /// 创建指定容量的内存审计 Sink
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: RwLock::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    /// 获取最近 N 条审计记录
    pub fn recent(&self, n: usize) -> Vec<PermissionAuditEntry> {
        let entries = self.entries.read().unwrap();
        entries.iter().rev().take(n).cloned().collect()
    }

    /// 获取所有审计记录（按时间正序）
    pub fn all(&self) -> Vec<PermissionAuditEntry> {
        let entries = self.entries.read().unwrap();
        entries.iter().cloned().collect()
    }

    /// 按条件查询审计记录
    pub fn query<F>(&self, predicate: F) -> Vec<PermissionAuditEntry>
    where
        F: Fn(&PermissionAuditEntry) -> bool,
    {
        let entries = self.entries.read().unwrap();
        entries.iter().filter(|e| predicate(e)).cloned().collect()
    }

    /// 获取审计记录数量
    pub fn count(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    /// 清空所有审计记录
    pub fn clear(&self) {
        self.entries.write().unwrap().clear();
    }
}

impl Default for InMemoryPermissionAuditSink {
    fn default() -> Self {
        Self::new(1000)
    }
}

#[async_trait]
impl PermissionAuditSink for InMemoryPermissionAuditSink {
    async fn record(&self, entry: PermissionAuditEntry) {
        let mut entries = self.entries.write().unwrap();
        if entries.len() >= self.capacity {
            entries.pop_front();
        }
        entries.push_back(entry);
    }
}

// ── 日志审计 Sink ─────────────────────────────────────────────────────────────

/// 日志审计 Sink — 使用 tracing 记录每条审计
pub struct LoggingPermissionAuditSink {
    level: LogLevel,
}

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Debug,
    Trace,
}

impl LoggingPermissionAuditSink {
    pub fn new(level: LogLevel) -> Self {
        Self { level }
    }

    pub fn info() -> Self {
        Self::new(LogLevel::Info)
    }

    pub fn debug() -> Self {
        Self::new(LogLevel::Debug)
    }
}

#[async_trait]
impl PermissionAuditSink for LoggingPermissionAuditSink {
    async fn record(&self, entry: PermissionAuditEntry) {
        match self.level {
            LogLevel::Info => {
                tracing::info!(
                    tool = %entry.tool_name,
                    decision = %entry.decision,
                    reason = %entry.reason,
                    source = %entry.source,
                    duration_us = entry.duration_us,
                    "权限审计"
                );
            }
            LogLevel::Warn => {
                tracing::warn!(
                    tool = %entry.tool_name,
                    decision = %entry.decision,
                    reason = %entry.reason,
                    "权限审计"
                );
            }
            LogLevel::Debug => {
                tracing::debug!(
                    tool = %entry.tool_name,
                    args_hash = %entry.args_hash,
                    decision = %entry.decision,
                    reason = %entry.reason,
                    source = %entry.source,
                    duration_us = entry.duration_us,
                    elapsed_ms = entry.elapsed_ms,
                    "权限审计"
                );
            }
            LogLevel::Trace => {
                tracing::trace!(?entry, "权限审计");
            }
        }
    }
}

// ── 组合审计 Sink ─────────────────────────────────────────────────────────────

/// 组合审计 Sink — 同时写入多个 Sink
pub struct CompositePermissionAuditSink {
    sinks: Vec<Box<dyn PermissionAuditSink>>,
}

impl CompositePermissionAuditSink {
    pub fn new(sinks: Vec<Box<dyn PermissionAuditSink>>) -> Self {
        Self { sinks }
    }

    pub fn with_sink(mut self, sink: Box<dyn PermissionAuditSink>) -> Self {
        self.sinks.push(sink);
        self
    }
}

#[async_trait]
impl PermissionAuditSink for CompositePermissionAuditSink {
    async fn record(&self, entry: PermissionAuditEntry) {
        for sink in &self.sinks {
            sink.record(entry.clone()).await;
        }
    }
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_entry(
        tool: &str,
        decision: &PermissionDecision,
        reason: &str,
        source: &str,
    ) -> PermissionAuditEntry {
        PermissionAuditEntry::new(
            tool,
            &json!({}),
            decision,
            reason,
            source,
            Instant::now(),
            Duration::from_micros(100),
        )
    }

    #[tokio::test]
    async fn test_in_memory_sink_record() {
        let sink = InMemoryPermissionAuditSink::new(100);

        sink.record(make_entry(
            "Bash",
            &PermissionDecision::Allow,
            "bypass",
            "bypass_mode",
        ))
        .await;
        sink.record(make_entry(
            "Read",
            &PermissionDecision::Deny {
                reason: "test".into(),
            },
            "rule",
            "rules",
        ))
        .await;

        assert_eq!(sink.count(), 2);
    }

    #[tokio::test]
    async fn test_in_memory_sink_capacity_eviction() {
        let sink = InMemoryPermissionAuditSink::new(3);

        for i in 0..5 {
            sink.record(make_entry(
                &format!("tool{i}"),
                &PermissionDecision::Allow,
                "test",
                "test",
            ))
            .await;
        }

        assert_eq!(sink.count(), 3);
        // 最早的 2 条被淘汰
        let all = sink.all();
        assert_eq!(all[0].tool_name, "tool2");
    }

    #[tokio::test]
    async fn test_in_memory_sink_recent() {
        let sink = InMemoryPermissionAuditSink::new(100);

        for i in 0..5 {
            sink.record(make_entry(
                &format!("tool{i}"),
                &PermissionDecision::Allow,
                "test",
                "test",
            ))
            .await;
        }

        let recent = sink.recent(2);
        assert_eq!(recent.len(), 2);
        // recent 返回倒序（最新的在前）
        assert_eq!(recent[0].tool_name, "tool4");
    }

    #[tokio::test]
    async fn test_in_memory_sink_query() {
        let sink = InMemoryPermissionAuditSink::new(100);

        sink.record(make_entry(
            "Bash",
            &PermissionDecision::Allow,
            "cache_hit",
            "cache",
        ))
        .await;
        sink.record(make_entry(
            "Bash",
            &PermissionDecision::Deny { reason: "r".into() },
            "rule",
            "rules",
        ))
        .await;
        sink.record(make_entry(
            "Read",
            &PermissionDecision::Allow,
            "bypass",
            "bypass",
        ))
        .await;

        let denies = sink.query(|e| e.decision == "deny");
        assert_eq!(denies.len(), 1);
        assert_eq!(denies[0].tool_name, "Bash");
    }

    #[tokio::test]
    async fn test_in_memory_sink_clear() {
        let sink = InMemoryPermissionAuditSink::new(100);
        sink.record(make_entry(
            "tool",
            &PermissionDecision::Allow,
            "test",
            "test",
        ))
        .await;
        assert_eq!(sink.count(), 1);

        sink.clear();
        assert_eq!(sink.count(), 0);
    }

    #[test]
    fn test_audit_entry_decision_str() {
        assert_eq!(
            PermissionAuditEntry::new(
                "t",
                &json!({}),
                &PermissionDecision::Allow,
                "r",
                "s",
                Instant::now(),
                Duration::ZERO,
            )
            .decision,
            "allow"
        );

        assert_eq!(
            PermissionAuditEntry::new(
                "t",
                &json!({}),
                &PermissionDecision::RequireApproval,
                "r",
                "s",
                Instant::now(),
                Duration::ZERO,
            )
            .decision,
            "require_approval"
        );
    }

    #[tokio::test]
    async fn test_composite_sink() {
        let sink1 = Arc::new(InMemoryPermissionAuditSink::new(100));
        let sink2 = Arc::new(InMemoryPermissionAuditSink::new(100));

        let composite = CompositePermissionAuditSink::new(vec![
            Box::new(sink1.clone()) as _,
            Box::new(sink2.clone()) as _,
        ]);

        composite
            .record(make_entry(
                "Bash",
                &PermissionDecision::Allow,
                "test",
                "test",
            ))
            .await;

        assert_eq!(sink1.count(), 1);
        assert_eq!(sink2.count(), 1);
    }
}
