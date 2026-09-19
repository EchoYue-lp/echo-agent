//! 审计日志
//!
//! 完整记录 tool 调用链、护栏阻断、权限拒绝等事件，支持合规审查。
//!
//! # 核心类型
//!
//! - [`AuditEvent`][]: 审计事件
//! - [`AuditLogger`]: 日志记录器 trait
//! - [`AuditCallback`]: 基于 `AgentCallback` 的自动审计

pub mod file;
pub mod memory;

pub use echo_core::audit::*;

use echo_core::utils::retention::ContentRetentionPolicy;
use futures::future::BoxFuture;
use serde_json::Value;
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

const DIAGNOSTIC_DELIVERY_CAPACITY: usize = 1_024;
static DIAGNOSTIC_DELIVERY_DROPPED: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static IN_DIAGNOSTIC_DISPATCH: Cell<bool> = const { Cell::new(false) };
}

/// Diagnostic record family whose persistence delivery failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiagnosticRecordKind {
    /// Structured execution trace stored through a `RunStore`.
    Trace,
    /// Audit event stored through an [`AuditLogger`].
    Audit,
}

impl DiagnosticRecordKind {
    /// Stable lowercase identifier for structured observers.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Audit => "audit",
        }
    }
}

/// Persistence operation that produced a diagnostic delivery failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiagnosticDeliveryOperation {
    /// Persist the initial diagnostic record.
    Start,
    /// Append one event to an existing diagnostic record.
    Append,
    /// Load the record required for terminal diagnostic finalization.
    Load,
    /// Persist the terminal diagnostic projection.
    Finalize,
    /// Persist one standalone audit record.
    Record,
}

impl DiagnosticDeliveryOperation {
    /// Stable lowercase identifier for structured observers.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Append => "append",
            Self::Load => "load",
            Self::Finalize => "finalize",
            Self::Record => "record",
        }
    }
}

/// Structured fact emitted when diagnostic persistence fails.
///
/// This fact is deliberately separate from an Agent execution result. An
/// observer may retain, count, or alert on it, but cannot rewrite the producer
/// terminal that was already decided by the Agent runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiagnosticDeliveryFailure {
    /// Time when the backend failure occurred, before asynchronous reporting.
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    /// Trace or audit record family.
    pub record_kind: DiagnosticRecordKind,
    /// Failed persistence operation.
    pub operation: DiagnosticDeliveryOperation,
    /// Run, trace, or session identity when one is available.
    pub record_id: Option<String>,
    /// Backend error without the diagnostic payload.
    pub error: String,
}

impl DiagnosticDeliveryFailure {
    /// Construct one diagnostic delivery failure fact.
    pub fn new(
        record_kind: DiagnosticRecordKind,
        operation: DiagnosticDeliveryOperation,
        record_id: Option<String>,
        error: impl Into<String>,
    ) -> Self {
        let mut failure = Self {
            occurred_at: chrono::Utc::now(),
            record_kind,
            operation,
            record_id,
            error: error.into(),
        };
        failure.apply_retention();
        failure
    }

    fn apply_retention(&mut self) {
        self.error = ContentRetentionPolicy::default().sanitize_text(&self.error);
    }
}

/// Consumer for diagnostic persistence failures.
///
/// The framework invokes this callback on a bounded diagnostic dispatcher, not
/// on the Agent producer. The callback is observational and has no control
/// return. Implementations should still return promptly so later diagnostic
/// notifications are not delayed. Applications that require an audit write for
/// a business commit must call [`AuditLogger::log`] directly and bind its
/// `Result` at that commit boundary instead of relying on an Agent callback.
///
/// # Example
///
/// ```rust
/// use echo_state::audit::{DiagnosticDeliveryFailure, DiagnosticDeliveryObserver};
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicUsize, Ordering};
///
/// #[derive(Default)]
/// struct FailureCounter(AtomicUsize);
///
/// impl DiagnosticDeliveryObserver for FailureCounter {
///     fn on_failure(&self, _failure: DiagnosticDeliveryFailure) {
///         self.0.fetch_add(1, Ordering::Relaxed);
///     }
/// }
///
/// let observer: Arc<dyn DiagnosticDeliveryObserver> = Arc::new(FailureCounter::default());
/// let _ = observer;
/// ```
pub trait DiagnosticDeliveryObserver: Send + Sync {
    /// Observe one failed diagnostic persistence operation.
    fn on_failure(&self, failure: DiagnosticDeliveryFailure);
}

/// Default observer that emits a stable structured `tracing` event.
#[derive(Debug, Default)]
struct TracingDiagnosticDeliveryObserver;

impl DiagnosticDeliveryObserver for TracingDiagnosticDeliveryObserver {
    fn on_failure(&self, mut failure: DiagnosticDeliveryFailure) {
        failure.apply_retention();
        tracing::error!(
            target: "echo_agent::diagnostic_delivery",
            record_kind = failure.record_kind.as_str(),
            operation = failure.operation.as_str(),
            record_id_present = failure.record_id.is_some(),
            occurred_at = %failure.occurred_at,
            error = %failure.error,
            "diagnostic persistence delivery failed"
        );
    }
}

struct DiagnosticDeliveryNotification {
    observer: Option<Arc<dyn DiagnosticDeliveryObserver>>,
    failure: DiagnosticDeliveryFailure,
}

fn increment_diagnostic_delivery_dropped() {
    let _ =
        DIAGNOSTIC_DELIVERY_DROPPED.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.saturating_add(1))
        });
}

fn diagnostic_delivery_sender() -> Option<&'static SyncSender<DiagnosticDeliveryNotification>> {
    static SENDER: OnceLock<Option<SyncSender<DiagnosticDeliveryNotification>>> = OnceLock::new();
    SENDER
        .get_or_init(|| {
            let (sender, receiver) =
                std::sync::mpsc::sync_channel::<DiagnosticDeliveryNotification>(
                    DIAGNOSTIC_DELIVERY_CAPACITY,
                );
            let spawn = std::thread::Builder::new()
                .name("echo-diagnostic-delivery".to_string())
                .spawn(move || {
                    while let Ok(notification) = receiver.recv() {
                        IN_DIAGNOSTIC_DISPATCH.with(|in_dispatch| {
                            in_dispatch.set(true);
                            let delivered =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    TracingDiagnosticDeliveryObserver
                                        .on_failure(notification.failure.clone());
                                    if let Some(observer) = notification.observer {
                                        observer.on_failure(notification.failure);
                                    }
                                }));
                            in_dispatch.set(false);
                            if delivered.is_err() {
                                increment_diagnostic_delivery_dropped();
                            }
                        });
                    }
                });
            match spawn {
                Ok(_) => Some(sender),
                Err(_) => None,
            }
        })
        .as_ref()
}

/// Number of diagnostic failure notifications that could not be delivered.
///
/// This saturating process-local counter covers dispatcher initialization
/// failure, queue saturation/disconnection, reentrant reporting, and observer
/// unwind. It is best-effort self-diagnostics, not an execution result.
pub fn diagnostic_delivery_dropped_count() -> u64 {
    DIAGNOSTIC_DELIVERY_DROPPED.load(Ordering::Relaxed)
}

/// Initialize the process-local diagnostic dispatcher before producer work.
#[doc(hidden)]
pub fn initialize_diagnostic_delivery() -> bool {
    diagnostic_delivery_sender().is_some()
}

/// Enqueue the default structured failure event and optional custom observer
/// notification without blocking the Agent producer.
#[doc(hidden)]
pub fn report_diagnostic_delivery_failure(
    observer: Option<Arc<dyn DiagnosticDeliveryObserver>>,
    mut failure: DiagnosticDeliveryFailure,
) {
    failure.apply_retention();
    if IN_DIAGNOSTIC_DISPATCH.with(Cell::get) {
        increment_diagnostic_delivery_dropped();
        return;
    }
    let Some(sender) = diagnostic_delivery_sender() else {
        increment_diagnostic_delivery_dropped();
        return;
    };
    let notification = DiagnosticDeliveryNotification { observer, failure };
    if matches!(
        sender.try_send(notification),
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_))
    ) {
        increment_diagnostic_delivery_dropped();
    }
}

/// 存储工具调用开始时的信息
struct ToolCallInfo {
    args: Value,
    started_at: std::time::Instant,
}

/// 基于 `AgentCallback` 的审计日志自动记录器
///
/// 实现 `AgentCallback`，将所有回调事件自动写入 `AuditLogger`。
///
/// # 示例
///
/// ```rust
/// use echo_state::audit::{memory::InMemoryAuditLogger, AuditCallback};
/// use std::sync::Arc;
///
/// let logger = Arc::new(InMemoryAuditLogger::new());
/// let audit_cb = Arc::new(AuditCallback::new(logger, "my-agent", None));
/// // 将 `audit_cb` 接入你自己的 agent/runtime 层，或通过 `echo_agent` façade 使用。
/// let _ = audit_cb;
/// ```
pub struct AuditCallback {
    logger: Arc<dyn AuditLogger>,
    retention: ContentRetentionPolicy,
    diagnostic_delivery_observer: Option<Arc<dyn DiagnosticDeliveryObserver>>,
    agent_name: String,
    session_id: Option<String>,
    /// tool_call_id → ToolCallInfo（args + start time）
    tool_calls: Mutex<HashMap<String, ToolCallInfo>>,
    /// Monotonic counter for generating unique tool_call_ids.
    /// Using a global counter instead of a per-tool-name sequence avoids
    /// prefix-collision in the lookup when concurrent calls share a tool name.
    next_call_id: AtomicU64,
}

impl AuditCallback {
    pub fn new(
        logger: Arc<dyn AuditLogger>,
        agent_name: impl Into<String>,
        session_id: Option<String>,
    ) -> Self {
        let _ = initialize_diagnostic_delivery();
        Self {
            logger,
            retention: ContentRetentionPolicy::default(),
            diagnostic_delivery_observer: None,
            agent_name: agent_name.into(),
            session_id,
            tool_calls: Mutex::new(HashMap::new()),
            next_call_id: AtomicU64::new(1),
        }
    }

    /// Replace the observer used when the audit backend rejects a callback
    /// record. The callback remains observational and never changes execution.
    pub fn with_diagnostic_delivery_observer(
        mut self,
        observer: Arc<dyn DiagnosticDeliveryObserver>,
    ) -> Self {
        self.diagnostic_delivery_observer = Some(observer);
        self
    }

    pub fn with_retention_policy(mut self, retention: ContentRetentionPolicy) -> Self {
        for info in self
            .tool_calls
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
        {
            retention.sanitize_json(&mut info.args);
        }
        self.retention = retention;
        self
    }

    async fn record_event(&self, mut event: AuditEvent) {
        event.apply_retention(&self.retention);
        let record_id = event.trace_id.clone().or_else(|| event.session_id.clone());
        if let Err(error) = self.logger.log(event).await {
            report_diagnostic_delivery_failure(
                self.diagnostic_delivery_observer.clone(),
                DiagnosticDeliveryFailure::new(
                    DiagnosticRecordKind::Audit,
                    DiagnosticDeliveryOperation::Record,
                    record_id,
                    error.to_string(),
                ),
            );
        }
    }

    /// 为工具调用生成唯一的 map key。
    ///
    /// Key 格式为 `"{tool}#{n}"`：以工具名为前缀 + 全局单调递增序号。前缀让
    /// [`pop_tool_call`] 能按工具名做前缀匹配（一个工具名下可能有多个并发
    /// in-flight 调用），全局序号保证 key 唯一、且可比较出最旧的调用。
    ///
    /// 之前实现返回 `call_{n}`，而 [`pop_tool_call`] 按 `"{tool}#"` 前缀查 ——
    /// 两者格式不匹配，pop 永远命中不了，导致 on_tool_end/on_tool_error 的
    /// duration_ms 恒为 0、input 恒为 Null（P1-13）。统一为 `tool#N` 修复。
    fn make_tool_call_id(&self, tool: &str) -> String {
        let n = self.next_call_id.fetch_add(1, Ordering::Relaxed);
        format!("{}#{}", tool, n)
    }

    fn make_event(&self, event_type: AuditEventType) -> AuditEvent {
        AuditEvent::now(self.session_id.clone(), self.agent_name.clone(), event_type)
    }

    /// Look up and remove the oldest in-flight tool call by tool name.
    ///
    /// Uses an exact-prefix match on the tool-in-key pattern (not
    /// iteration-order-dependent), then removes and returns the entry
    /// with the smallest embedded sequence number.  This is deterministic
    /// even when concurrent calls share a tool name.
    fn pop_tool_call(&self, tool: &str) -> Option<ToolCallInfo> {
        let mut map = self
            .tool_calls
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // Collect all keys matching this tool's tracking prefix ("{tool}#N").
        let candidates: Vec<String> = map
            .keys()
            .filter(|k| k.starts_with(&format!("{}#", tool)))
            .cloned()
            .collect();
        if candidates.is_empty() {
            return None;
        }
        // Remove the entry with the smallest sequence number (oldest).
        let key = candidates.into_iter().min_by_key(|k| {
            k.split('#')
                .nth(1)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(u64::MAX)
        })?;
        map.remove(&key)
    }

    fn remember_tool_call(&self, call_id: String, args: &Value) {
        let mut args = args.clone();
        self.retention.sanitize_json(&mut args);
        self.tool_calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                call_id,
                ToolCallInfo {
                    args,
                    started_at: std::time::Instant::now(),
                },
            );
    }

    fn take_tool_call(&self, call_id: &str) -> Option<ToolCallInfo> {
        self.tool_calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(call_id)
    }
}

impl echo_core::agent::AgentCallback for AuditCallback {
    fn on_tool_start<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        args: &'a Value,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let tool_call_id = self.make_tool_call_id(tool);
            self.remember_tool_call(tool_call_id, args);
        })
    }

    fn on_tool_start_with_id<'a>(
        &'a self,
        _agent: &'a str,
        call_id: &'a str,
        _tool: &'a str,
        args: &'a Value,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.remember_tool_call(call_id.to_string(), args);
        })
    }

    fn on_tool_end<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        result: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let (duration_ms, input) = self
                .pop_tool_call(tool)
                .map(|info| (info.started_at.elapsed().as_millis() as u64, info.args))
                .unwrap_or((0, Value::Null));

            let event = self.make_event(AuditEventType::ToolCall {
                call_id: None,
                tool: tool.to_string(),
                input,
                output: result.to_string(),
                success: true,
                duration_ms,
            });
            self.record_event(event).await;
        })
    }

    fn on_tool_end_with_id<'a>(
        &'a self,
        _agent: &'a str,
        call_id: &'a str,
        tool: &'a str,
        result: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let (duration_ms, input) = self
                .take_tool_call(call_id)
                .map(|info| (info.started_at.elapsed().as_millis() as u64, info.args))
                .unwrap_or((0, Value::Null));
            let event = self.make_event(AuditEventType::ToolCall {
                call_id: Some(call_id.to_string()),
                tool: tool.to_string(),
                input,
                output: result.to_string(),
                success: true,
                duration_ms,
            });
            self.record_event(event).await;
        })
    }

    fn on_tool_error<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        err: &'a echo_core::error::ReactError,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let (duration_ms, input) = self
                .pop_tool_call(tool)
                .map(|info| (info.started_at.elapsed().as_millis() as u64, info.args))
                .unwrap_or((0, Value::Null));

            let event = self.make_event(AuditEventType::ToolCall {
                call_id: None,
                tool: tool.to_string(),
                input,
                output: err.to_string(),
                success: false,
                duration_ms,
            });
            self.record_event(event).await;
        })
    }

    fn on_tool_error_with_id<'a>(
        &'a self,
        _agent: &'a str,
        call_id: &'a str,
        tool: &'a str,
        error: &'a echo_core::error::ReactError,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let (duration_ms, input) = self
                .take_tool_call(call_id)
                .map(|info| (info.started_at.elapsed().as_millis() as u64, info.args))
                .unwrap_or((0, Value::Null));
            let event = self.make_event(AuditEventType::ToolCall {
                call_id: Some(call_id.to_string()),
                tool: tool.to_string(),
                input,
                output: error.to_string(),
                success: false,
                duration_ms,
            });
            self.record_event(event).await;
        })
    }

    fn on_tool_interrupted_with_id<'a>(
        &'a self,
        _agent: &'a str,
        call_id: &'a str,
        tool: &'a str,
        input: &'a Value,
        error: &'a echo_core::error::ReactError,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let (duration_ms, input) = self
                .take_tool_call(call_id)
                .map(|info| (info.started_at.elapsed().as_millis() as u64, info.args))
                .unwrap_or_else(|| {
                    let mut input = input.clone();
                    self.retention.sanitize_json(&mut input);
                    (0, input)
                });
            let event = self.make_event(AuditEventType::ToolCall {
                call_id: Some(call_id.to_string()),
                tool: tool.to_string(),
                input,
                output: error.to_string(),
                success: false,
                duration_ms,
            });
            self.record_event(event).await;
        })
    }

    fn on_final_answer<'a>(&'a self, _agent: &'a str, answer: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let event = self.make_event(AuditEventType::FinalAnswer {
                content: answer.to_string(),
            });
            self.record_event(event).await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::agent::AgentCallback;
    use echo_core::error::{ReactError, Result};

    pub(super) fn capture_audit_logs<T>(action: impl FnOnce() -> T) -> (T, String) {
        struct Capture(Arc<Mutex<String>>);

        impl tracing::field::Visit for Capture {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                use std::fmt::Write;
                let _ = write!(
                    self.0.lock().unwrap_or_else(|e| e.into_inner()),
                    "{}={value:?};",
                    field.name()
                );
            }

            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write;
                let _ = write!(
                    self.0.lock().unwrap_or_else(|e| e.into_inner()),
                    "{}={value:?};",
                    field.name()
                );
            }
        }

        impl tracing::Subscriber for Capture {
            fn register_callsite(
                &self,
                _: &'static tracing::Metadata<'static>,
            ) -> tracing::subscriber::Interest {
                tracing::subscriber::Interest::always()
            }

            fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
            fn event(&self, event: &tracing::Event<'_>) {
                event.record(&mut Capture(self.0.clone()));
            }
            fn enter(&self, _: &tracing::span::Id) {}
            fn exit(&self, _: &tracing::span::Id) {}
        }

        let output = Arc::new(Mutex::new(String::new()));
        let capture = tracing::Dispatch::new(Capture(output.clone()));
        // tracing-core 0.1.36 can cache `Interest::never` when a callsite is
        // first observed on another thread while only one Dispatch exists.
        // Keep a second Dispatch registered so cache rebuilds use the
        // multi-dispatch path. Remove this after tokio-rs/tracing#3611 lands.
        let _callsite_cache_guard =
            tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        let result = tracing::dispatcher::with_default(&capture, action);
        let text = output.lock().unwrap_or_else(|e| e.into_inner()).clone();
        (result, text)
    }

    #[test]
    fn audit_default_diagnostic_logging_sanitizes_mutated_error() {
        let mut failure = sample_failure();
        failure.record_id = Some("token=raw-record-identity-secret".into());
        failure.error = "Bearer abcdefghijklmnopqrstuvwxyz".into();
        let (_, logs) = capture_audit_logs(|| {
            // Exercise the upstream race deterministically: after Capture is
            // registered, another thread without a subscriber sees this
            // static callsite before the Capture thread does.
            let uncaptured = sample_failure();
            std::thread::spawn(move || {
                TracingDiagnosticDeliveryObserver.on_failure(uncaptured);
            })
            .join()
            .unwrap_or(());
            TracingDiagnosticDeliveryObserver.on_failure(failure);
        });
        assert!(logs.contains("diagnostic persistence delivery failed"));
        assert!(logs.contains("[REDACTED]"));
        assert!(!logs.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!logs.contains("raw-record-identity-secret"));
    }

    #[tokio::test]
    async fn audit_memory_default_retention_and_policy_change_sanitize_stored_copies() -> Result<()>
    {
        let logger = memory::InMemoryAuditLogger::new();
        let event = AuditEvent::now(
            None,
            "agent".into(),
            AuditEventType::ToolCall {
                call_id: Some("retention-call".into()),
                tool: "shell".into(),
                input: serde_json::json!({"password":"tiny", "count":9}),
                output: format!("Bearer abcdefghijklmnopqrstuvwxyz {}", "中".repeat(17_000)),
                success: true,
                duration_ms: 100,
            },
        );
        logger.log(event.clone()).await?;
        let before = serde_json::to_string(&logger.snapshot())?;
        assert!(!before.contains("tiny"));
        assert!(!before.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(before.contains("[TRUNCATED]"));
        assert!(serde_json::to_string(&event)?.contains("tiny"));
        let logger = logger.with_retention_policy(ContentRetentionPolicy {
            max_string_chars: 0,
            max_array_items: 0,
        });
        let stored = logger.query(AuditFilter::default()).await?;
        assert!(
            matches!(stored.first().map(|e| &e.event_type), Some(AuditEventType::ToolCall { input, output, duration_ms: 100, .. }) if input.as_object().is_some_and(|fields| fields.len() == 1 && fields.values().any(|value| value.as_str() == Some("[TRUNCATED OBJECT]"))) && output == "...[TRUNCATED]")
        );
        Ok(())
    }

    #[derive(Default)]
    struct RecordingAuditLogger(Mutex<Vec<AuditEvent>>);

    impl AuditLogger for RecordingAuditLogger {
        fn log<'a>(&'a self, event: AuditEvent) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.0.lock().unwrap_or_else(|e| e.into_inner()).push(event);
                Ok(())
            })
        }

        fn query<'a>(&'a self, _filter: AuditFilter) -> BoxFuture<'a, Result<Vec<AuditEvent>>> {
            Box::pin(async move { Ok(self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()) })
        }
    }

    #[tokio::test]
    async fn audit_callback_custom_backend_receives_only_retained_copies() -> Result<()> {
        let logger = Arc::new(RecordingAuditLogger::default());
        let callback = AuditCallback::new(logger.clone(), "agent", Some("session".into()));
        let args = serde_json::json!({"password": "tiny", "count": 9});
        callback.on_tool_start("agent", "shell", &args).await;
        {
            let calls = callback
                .tool_calls
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            assert!(
                calls
                    .values()
                    .all(|info| !info.args.to_string().contains("tiny"))
            );
        }
        assert_eq!(args.get("password"), Some(&Value::String("tiny".into())));
        callback
            .on_tool_end("agent", "shell", "Bearer abcdefghijklmnopqrstuvwxyz")
            .await;
        callback.on_tool_start("agent", "shell", &args).await;
        callback
            .on_tool_error(
                "agent",
                "shell",
                &ReactError::Other("password=abcdefgh".into()),
            )
            .await;
        callback
            .on_final_answer("agent", r#"{"secret":"short"}"#)
            .await;
        let events = logger.query(AuditFilter::default()).await?;
        assert_eq!(events.len(), 3);
        let encoded = serde_json::to_string(&events)?;
        for secret in ["tiny", "abcdefghijklmnopqrstuvwxyz", "abcdefgh", "short"] {
            assert!(!encoded.contains(secret));
        }
        assert!(matches!(
            events.first().map(|e| &e.event_type),
            Some(AuditEventType::ToolCall { success: true, .. })
        ));
        assert!(matches!(
            events.get(1).map(|e| &e.event_type),
            Some(AuditEventType::ToolCall { success: false, .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn audit_callback_correlates_reverse_completion_by_call_id_after_poison() -> Result<()> {
        let logger = Arc::new(RecordingAuditLogger::default());
        let callback = AuditCallback::new(logger.clone(), "agent", Some("session".into()));
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = callback
                .tool_calls
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            std::panic::resume_unwind(Box::new("poison callback correlation".to_string()));
        }));
        assert!(poisoned.is_err());

        callback
            .on_tool_start_with_id(
                "agent",
                "call-a",
                "shell",
                &serde_json::json!({"label": "a"}),
            )
            .await;
        callback
            .on_tool_start_with_id(
                "agent",
                "call-b",
                "shell",
                &serde_json::json!({"label": "b"}),
            )
            .await;
        callback
            .on_tool_end_with_id("agent", "call-b", "shell", "output-b")
            .await;
        callback
            .on_tool_error_with_id(
                "agent",
                "call-a",
                "shell",
                &ReactError::Other("error-a".to_string()),
            )
            .await;

        let events = logger.query(AuditFilter::default()).await?;
        assert!(matches!(
            events.first().map(|event| &event.event_type),
            Some(AuditEventType::ToolCall { call_id: Some(call_id), input, output, success: true, .. })
            if call_id == "call-b" && input.get("label") == Some(&serde_json::json!("b")) && output == "output-b"
        ));
        assert!(matches!(
            events.get(1).map(|event| &event.event_type),
            Some(AuditEventType::ToolCall { call_id: Some(call_id), input, output, success: false, .. })
            if call_id == "call-a" && input.get("label") == Some(&serde_json::json!("a")) && output == "error-a"
        ));
        Ok(())
    }

    #[tokio::test]
    async fn audit_callback_interruption_uses_admitted_input_with_or_without_start() -> Result<()> {
        let logger = Arc::new(RecordingAuditLogger::default());
        let callback = AuditCallback::new(logger.clone(), "agent", Some("session".into()));
        let error = ReactError::Other("interrupted".to_string());

        callback
            .on_tool_interrupted_with_id(
                "agent",
                "before-start",
                "shell",
                &serde_json::json!({"label": "admitted", "password": "tiny-secret"}),
                &error,
            )
            .await;
        callback
            .on_tool_start_with_id(
                "agent",
                "after-start",
                "shell",
                &serde_json::json!({"label": "started"}),
            )
            .await;
        callback
            .on_tool_interrupted_with_id(
                "agent",
                "after-start",
                "shell",
                &serde_json::json!({"label": "must-not-replace-start"}),
                &error,
            )
            .await;

        let events = logger.query(AuditFilter::default()).await?;
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events.first().map(|event| &event.event_type),
            Some(AuditEventType::ToolCall { call_id: Some(call_id), input, success: false, duration_ms: 0, .. })
                if call_id == "before-start"
                    && input.get("label") == Some(&serde_json::json!("admitted"))
                    && input.get("password") == Some(&serde_json::json!("[REDACTED]"))
        ));
        assert!(matches!(
            events.get(1).map(|event| &event.event_type),
            Some(AuditEventType::ToolCall { call_id: Some(call_id), input, success: false, .. })
                if call_id == "after-start"
                    && input.get("label") == Some(&serde_json::json!("started"))
        ));
        assert!(callback.take_tool_call("before-start").is_none());
        assert!(callback.take_tool_call("after-start").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn audit_memory_and_file_retention_preserve_typed_contract_at_zero_limits() -> Result<()>
    {
        let policy = ContentRetentionPolicy {
            max_string_chars: 0,
            max_array_items: 0,
        };
        let memory = memory::InMemoryAuditLogger::new().with_retention_policy(policy);
        let temp = std::env::temp_dir().join(format!("echo-audit-typed-{}", uuid::Uuid::new_v4()));
        let path = temp.join("audit.jsonl");
        let file = file::FileAuditLogger::new(&path)?.with_retention_policy(policy);
        let kinds = vec![
            AuditEventType::UserInput {
                content: "Bearer abcdefghijklmnopqrstuvwxyz".into(),
            },
            AuditEventType::FinalAnswer {
                content: "中文字符".repeat(30),
            },
            AuditEventType::LlmCall {
                model: "model-long-name".into(),
                prompt_tokens: Some(123),
                completion_tokens: Some(456),
            },
            AuditEventType::ToolCall {
                call_id: Some("zero-limit-call".into()),
                tool: "shell".into(),
                input: serde_json::json!({"password":"short", "values": [1, 2], "count": 42}),
                output: "password=abcdefgh".into(),
                success: false,
                duration_ms: 987,
            },
            AuditEventType::GuardBlock {
                guard: "guard".into(),
                direction: echo_core::guard::GuardDirection::Input,
                reason: "token=abcdefgh".into(),
            },
            AuditEventType::PermissionDenied {
                tool: "shell".into(),
                required: vec![
                    echo_core::tools::permission::ToolPermission::Read,
                    echo_core::tools::permission::ToolPermission::Write,
                ],
                reason: "secret=abcdefgh".into(),
            },
            AuditEventType::ApprovalRequested {
                tool: "shell".into(),
                args_hash: "hash-long-name".into(),
                risk_level: "high".into(),
            },
            AuditEventType::ApprovalCompleted {
                tool: "shell".into(),
                decision: "denied".into(),
                scope: "session".into(),
                reason: Some("password=abcdefgh".into()),
                duration_ms: 654,
            },
        ];
        for kind in kinds {
            let mut event = AuditEvent::now(
                Some("session-long-name".into()),
                "agent-long-name".into(),
                kind,
            );
            event.trace_id = Some("trace-long-name".into());
            memory.log(event.clone()).await?;
            file.log(event).await?;
        }
        let filter = AuditFilter {
            session_id: Some("session-long-name".into()),
            agent_name: Some("agent-long-name".into()),
            ..Default::default()
        };
        let retained = memory.query(filter.clone()).await?;
        assert_eq!(retained.len(), 8);
        assert_eq!(
            serde_json::to_value(&retained)?,
            serde_json::to_value(file.query(filter).await?)?
        );
        for event in &retained {
            assert_eq!(event.trace_id.as_deref(), Some("trace-long-name"));
            let encoded = serde_json::to_string(event)?;
            let _: AuditEvent = serde_json::from_str(&encoded)?;
            for secret in ["abcdefghijklmnopqrstuvwxyz", "abcdefgh", "short"] {
                assert!(!encoded.contains(secret));
            }
        }
        assert!(matches!(
            retained.get(2).map(|e| &e.event_type),
            Some(AuditEventType::LlmCall {
                prompt_tokens: Some(123),
                completion_tokens: Some(456),
                ..
            })
        ));
        assert!(matches!(
            retained.get(3).map(|e| &e.event_type),
            Some(AuditEventType::ToolCall {
                success: false,
                duration_ms: 987,
                ..
            })
        ));
        assert!(
            matches!(retained.get(5).map(|e| &e.event_type), Some(AuditEventType::PermissionDenied { required, .. }) if required.len() == 2)
        );
        drop(file);
        let reopened = file::FileAuditLogger::new(path)?.with_retention_policy(policy);
        assert_eq!(reopened.query(AuditFilter::default()).await?.len(), 8);
        drop(reopened);
        std::fs::remove_dir_all(temp)?;
        Ok(())
    }

    #[tokio::test]
    async fn memory_and_file_audit_redact_complete_private_key_body() -> Result<()> {
        let directory =
            std::env::temp_dir().join(format!("echo-audit-private-key-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("audit.jsonl");
        let memory = memory::InMemoryAuditLogger::new();
        let file = file::FileAuditLogger::new(&path)?;
        let private_key_body = "MIIEvQIBADANBgkqhkiG9w0B";
        let event = AuditEvent::now(
            Some("session-pem".to_string()),
            "agent".to_string(),
            AuditEventType::UserInput {
                content: format!(
                    "-----BEGIN PRIVATE KEY-----\n{private_key_body}\n-----END PRIVATE KEY-----"
                ),
            },
        );
        memory.log(event.clone()).await?;
        file.log(event).await?;
        let filter = AuditFilter {
            session_id: Some("session-pem".to_string()),
            ..Default::default()
        };
        let in_memory = memory.query(filter.clone()).await?;
        let on_disk = file.query(filter).await?;
        assert_eq!(
            serde_json::to_value(&in_memory)?,
            serde_json::to_value(&on_disk)?
        );
        let serialized = std::fs::read_to_string(&path)?;
        assert!(!serialized.contains(private_key_body));
        assert!(!serialized.contains("BEGIN PRIVATE KEY"));
        assert!(serialized.contains("[REDACTED]"));
        drop(file);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn audit_diagnostic_errors_are_retained_before_custom_dispatch() -> Result<()> {
        let observer = Arc::new(RecordingObserver::default());
        let mut failure = DiagnosticDeliveryFailure::new(
            DiagnosticRecordKind::Audit,
            DiagnosticDeliveryOperation::Record,
            Some("session".into()),
            "Bearer abcdefghijklmnopqrstuvwxyz",
        );
        assert!(!format!("{failure:?}").contains("abcdefghijklmnopqrstuvwxyz"));
        failure.error = format!("password=abcdefgh {}", "中".repeat(20_000));
        report_diagnostic_delivery_failure(Some(observer.clone()), failure);
        let failures = observer.wait_for_count(1)?;
        let failure = failures
            .first()
            .ok_or_else(|| ReactError::Other("missing failure".into()))?;
        assert!(!failure.error.contains("abcdefgh"));
        assert!(failure.error.contains("[TRUNCATED]"));
        assert_eq!(failure.record_id.as_deref(), Some("session"));
        assert_eq!(failure.operation, DiagnosticDeliveryOperation::Record);
        Ok(())
    }

    struct FailingAuditLogger;

    impl AuditLogger for FailingAuditLogger {
        fn log<'a>(&'a self, _event: AuditEvent) -> BoxFuture<'a, Result<()>> {
            Box::pin(async {
                Err(ReactError::Other(
                    "injected audit persistence failure".to_string(),
                ))
            })
        }

        fn query<'a>(&'a self, _filter: AuditFilter) -> BoxFuture<'a, Result<Vec<AuditEvent>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        failures: Mutex<Vec<DiagnosticDeliveryFailure>>,
        changed: std::sync::Condvar,
    }

    impl DiagnosticDeliveryObserver for RecordingObserver {
        fn on_failure(&self, failure: DiagnosticDeliveryFailure) {
            self.failures
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(failure);
            self.changed.notify_all();
        }
    }

    impl RecordingObserver {
        fn wait_for_count(&self, count: usize) -> Result<Vec<DiagnosticDeliveryFailure>> {
            let failures = self
                .failures
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let (failures, wait) = self
                .changed
                .wait_timeout_while(failures, std::time::Duration::from_secs(5), |failures| {
                    failures.len() < count
                })
                .map_err(|error| ReactError::Other(format!("observer wait failed: {error}")))?;
            if wait.timed_out() && failures.len() < count {
                return Err(ReactError::Other(format!(
                    "timed out waiting for {count} diagnostic failures"
                )));
            }
            Ok(failures.clone())
        }
    }

    struct ReentrantObserver {
        completed: std::sync::mpsc::Sender<()>,
    }

    impl DiagnosticDeliveryObserver for ReentrantObserver {
        fn on_failure(&self, failure: DiagnosticDeliveryFailure) {
            report_diagnostic_delivery_failure(None, failure);
            let _ = self.completed.send(());
        }
    }

    struct UnwindingObserver {
        attempted: std::sync::mpsc::Sender<()>,
    }

    impl DiagnosticDeliveryObserver for UnwindingObserver {
        fn on_failure(&self, _failure: DiagnosticDeliveryFailure) {
            let _ = self.attempted.send(());
            // Intentional fault injection: the diagnostic dispatcher catches
            // this unwind, increments its drop counter, and remains alive.
            std::panic::resume_unwind(Box::new("injected observer unwind".to_string()));
        }
    }

    fn sample_failure() -> DiagnosticDeliveryFailure {
        DiagnosticDeliveryFailure::new(
            DiagnosticRecordKind::Audit,
            DiagnosticDeliveryOperation::Record,
            Some("session-46".to_string()),
            "injected audit persistence failure",
        )
    }

    #[tokio::test]
    async fn audit_callback_reports_backend_failure_without_changing_callback_result() -> Result<()>
    {
        let observer = Arc::new(RecordingObserver::default());
        let callback = AuditCallback::new(
            Arc::new(FailingAuditLogger),
            "agent",
            Some("session-46".to_string()),
        )
        .with_diagnostic_delivery_observer(observer.clone());

        callback
            .on_final_answer("agent", "producer answer remains successful")
            .await;

        let failures = observer.wait_for_count(1)?;
        let failure = failures
            .first()
            .ok_or_else(|| ReactError::Other("missing diagnostic delivery failure".to_string()))?;
        assert_eq!(failure.record_kind, DiagnosticRecordKind::Audit);
        assert_eq!(failure.operation, DiagnosticDeliveryOperation::Record);
        assert_eq!(failure.record_id.as_deref(), Some("session-46"));
        assert!(failure.error.contains("injected audit persistence failure"));
        Ok(())
    }

    #[test]
    fn reentrant_observer_is_bounded_and_counted() -> Result<()> {
        let dropped_before = diagnostic_delivery_dropped_count();
        let (completed, completion) = std::sync::mpsc::channel();
        report_diagnostic_delivery_failure(
            Some(Arc::new(ReentrantObserver { completed })),
            sample_failure(),
        );

        completion
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| {
                ReactError::Other(format!("reentrant observer did not run: {error}"))
            })?;
        assert!(diagnostic_delivery_dropped_count() > dropped_before);
        Ok(())
    }

    #[test]
    fn unwinding_observer_does_not_stop_later_diagnostic_delivery() -> Result<()> {
        let dropped_before = diagnostic_delivery_dropped_count();
        let (attempted, attempt) = std::sync::mpsc::channel();
        report_diagnostic_delivery_failure(
            Some(Arc::new(UnwindingObserver { attempted })),
            sample_failure(),
        );
        attempt
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| {
                ReactError::Other(format!("unwinding observer did not run: {error}"))
            })?;

        let observer = Arc::new(RecordingObserver::default());
        report_diagnostic_delivery_failure(Some(observer.clone()), sample_failure());
        let _ = observer.wait_for_count(1)?;
        assert!(diagnostic_delivery_dropped_count() > dropped_before);
        Ok(())
    }
}
