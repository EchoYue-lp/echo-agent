//! Headless mode — run a single prompt, print output, exit.
//!
//! Designed for CI/CD pipelines, scripting, and non-interactive automation.
//! The agent runs a single prompt, collects the output, and returns a
//! structured result that the caller can print and exit on.
//!
//! # Example
//!
//! ```rust,no_run
//! use echo_agent::headless::{HeadlessConfig, run_headless};
//!
//! # #[tokio::main]
//! # async fn main() {
//! let config = HeadlessConfig {
//!     prompt: "List all Rust files in the project".into(),
//!     exit_on_error: true,
//!     output_format: "text".into(),
//!     max_iterations: Some(10),
//!     cancel_token: None,
//! };
//!
//! let result = run_headless(config, |builder| builder).await;
//! println!("{}", result.output);
//! std::process::exit(result.exit_code());
//! # }
//! ```

use crate::agent::Agent;
use crate::agent::react::builder::ReactAgentBuilder;
use crate::runtime::{
    AgentTurnDriver, EventSink, SinkControl, TurnDeliveryOutcome, TurnMode, TurnOutcome,
    TurnReceipt, TurnRequest,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct HeadlessEventSink;

#[async_trait::async_trait]
impl EventSink for HeadlessEventSink {
    async fn on_event(
        &self,
        _envelope: crate::agent::EventEnvelope,
    ) -> crate::error::Result<SinkControl> {
        Ok(SinkControl::Continue)
    }
}

/// Configuration for headless (non-interactive) agent execution.
pub struct HeadlessConfig {
    /// The prompt to execute.
    pub prompt: String,

    /// Exit with error if the agent reports failure.
    pub exit_on_error: bool,

    /// Output format: `"text"` (default) or `"json"`.
    pub output_format: String,

    /// Max iterations before forcing stop (safety limit).
    pub max_iterations: Option<usize>,

    /// Optional caller-owned cancellation token.
    ///
    /// Headless derives a child token for this run, so cancelling the run does
    /// not cancel the caller's parent scope or sibling work.
    pub cancel_token: Option<CancellationToken>,
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            exit_on_error: true,
            output_format: "text".into(),
            max_iterations: None,
            cancel_token: None,
        }
    }
}

/// Result of a headless execution.
#[derive(Debug, Clone)]
pub struct HeadlessResult {
    /// The agent's final output text.
    pub output: String,

    /// Whether the execution succeeded.
    pub success: bool,

    /// Model name used.
    pub model: String,

    /// Output format requested.
    pub format: String,

    /// Whether a failed run should produce a non-zero process exit code.
    pub exit_on_error: bool,
}

struct HeadlessRunState {
    result: std::sync::Mutex<Option<HeadlessResult>>,
    result_ready: tokio::sync::Notify,
    cancel: CancellationToken,
    close_gate: tokio::sync::Mutex<()>,
    close_owner: std::sync::Mutex<Option<Arc<dyn Agent>>>,
}

impl HeadlessRunState {
    fn new(cancel: CancellationToken, agent: Option<Arc<dyn Agent>>) -> Self {
        Self {
            result: std::sync::Mutex::new(None),
            result_ready: tokio::sync::Notify::new(),
            cancel,
            close_gate: tokio::sync::Mutex::new(()),
            close_owner: std::sync::Mutex::new(agent),
        }
    }

    fn publish(&self, result: HeadlessResult) {
        let mut slot = self
            .result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_none() {
            *slot = Some(result);
            drop(slot);
            self.result_ready.notify_waiters();
        }
    }

    fn result(&self) -> Option<HeadlessResult> {
        self.result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    async fn close_once(&self) -> crate::error::Result<()> {
        let _gate = self.close_gate.lock().await;
        let agent = self
            .close_owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(agent) = agent else {
            return Ok(());
        };
        agent.close().await?;
        let mut owner = self
            .close_owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if owner
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &agent))
        {
            owner.take();
        }
        Ok(())
    }
}

/// Retained owner and result receipt for one Headless execution.
///
/// `wait` may be cancelled without cancelling the owned cleanup task. Use
/// [`Self::cancel`] to request Turn cancellation, and [`Self::retry_close`] if
/// the first resource-close attempt is reported as failed.
#[derive(Clone)]
pub struct HeadlessRunHandle {
    state: Arc<HeadlessRunState>,
}

impl HeadlessRunHandle {
    fn completed(result: HeadlessResult) -> Self {
        let state = Arc::new(HeadlessRunState::new(CancellationToken::new(), None));
        state.publish(result);
        Self { state }
    }

    /// Request cancellation of the owned Turn.
    pub fn cancel(&self) {
        self.state.cancel.cancel();
    }

    /// Wait for execution and the first Agent close attempt to settle.
    pub async fn wait(&self) -> HeadlessResult {
        loop {
            let notified = self.state.result_ready.notified();
            if let Some(result) = self.state.result() {
                return result;
            }
            notified.await;
        }
    }

    /// Retry the same Agent owner after a reported close failure.
    ///
    /// Returns an error until [`Self::wait`] has observed the run receipt.
    /// After close succeeds, repeated calls are idempotent.
    pub async fn retry_close(&self) -> crate::error::Result<()> {
        if self.state.result().is_none() {
            return Err(crate::error::ReactError::Other(
                "Agent close cannot be retried before Headless execution settles".to_string(),
            ));
        }
        self.state.close_once().await
    }
}

struct HeadlessTaskOwner {
    state: Arc<HeadlessRunState>,
    fallback: Option<HeadlessResult>,
}

impl HeadlessTaskOwner {
    fn publish(&mut self, result: HeadlessResult) {
        self.state.publish(result);
        self.fallback.take();
    }
}

impl Drop for HeadlessTaskOwner {
    fn drop(&mut self) {
        if let Some(result) = self.fallback.take() {
            self.state.publish(result);
        }
    }
}

struct HeadlessWaitCancellationGuard {
    cancel: CancellationToken,
    active: bool,
}

impl HeadlessWaitCancellationGuard {
    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for HeadlessWaitCancellationGuard {
    fn drop(&mut self) {
        if self.active {
            self.cancel.cancel();
        }
    }
}

impl HeadlessResult {
    /// Compute the process exit code: 0 on success, 1 on failure.
    pub fn exit_code(&self) -> i32 {
        if self.success || !self.exit_on_error {
            0
        } else {
            1
        }
    }

    /// Format the result for stdout according to the requested output format.
    pub fn format_output(&self) -> String {
        match self.format.as_str() {
            "json" => {
                let json = serde_json::json!({
                    "success": self.success,
                    "model": self.model,
                    "output": self.output,
                });
                serde_json::to_string_pretty(&json).unwrap_or_else(|_| self.output.clone())
            }
            _ => self.output.clone(),
        }
    }
}

/// Run the agent in headless mode.
///
/// The `configure` closure receives a [`ReactAgentBuilder`] so the caller can
/// set model, system prompt, tools, etc. before the agent is built and run.
///
/// # Arguments
///
/// * `config` — headless execution parameters (prompt, format, limits)
/// * `configure` — closure to customize the agent builder
///
/// # Returns
///
/// A [`HeadlessResult`] with the agent output, success flag, and metadata.
pub async fn run_headless<F>(config: HeadlessConfig, configure: F) -> HeadlessResult
where
    F: FnOnce(ReactAgentBuilder) -> ReactAgentBuilder,
{
    let handle = start_headless(config, configure);
    wait_headless_handle(handle).await
}

async fn wait_headless_handle(handle: HeadlessRunHandle) -> HeadlessResult {
    let mut cancellation_guard = HeadlessWaitCancellationGuard {
        cancel: handle.state.cancel.clone(),
        active: true,
    };
    let result = handle.wait().await;
    cancellation_guard.disarm();
    result
}

/// Start Headless execution and synchronously return its retained owner.
///
/// This is the cancellation-safe entry point for callers that may stop waiting
/// and later observe the same result or retry a failed Agent close.
pub fn start_headless<F>(config: HeadlessConfig, configure: F) -> HeadlessRunHandle
where
    F: FnOnce(ReactAgentBuilder) -> ReactAgentBuilder,
{
    let exit_on_error = config.exit_on_error;
    if config.prompt.is_empty() {
        return HeadlessRunHandle::completed(HeadlessResult {
            output: "Error: empty prompt".into(),
            success: false,
            model: String::new(),
            format: config.output_format,
            exit_on_error,
        });
    }

    // Build the agent
    let builder = ReactAgentBuilder::new();
    let builder = configure(builder);

    // Apply max_iterations if set
    let builder = if let Some(max) = config.max_iterations {
        builder.max_iterations(max)
    } else {
        builder
    };

    let agent = match builder.build() {
        Ok(a) => a,
        Err(e) => {
            return HeadlessRunHandle::completed(HeadlessResult {
                output: format!("Error building agent: {}", e),
                success: false,
                model: String::new(),
                format: config.output_format,
                exit_on_error,
            });
        }
    };

    start_headless_agent(config, Arc::new(agent))
}

fn start_headless_agent(config: HeadlessConfig, agent: Arc<dyn Agent>) -> HeadlessRunHandle {
    // The caller owns the configured token. Headless cancellation must remain
    // inside this run and must not cancel sibling work in the caller's scope.
    let cancel = config
        .cancel_token
        .as_ref()
        .map(CancellationToken::child_token)
        .unwrap_or_default();
    let state = Arc::new(HeadlessRunState::new(cancel.clone(), Some(agent.clone())));
    let handle = HeadlessRunHandle {
        state: Arc::clone(&state),
    };
    let runtime = match tokio::runtime::Handle::try_current() {
        Ok(runtime) => runtime,
        Err(error) => {
            state.publish(HeadlessResult {
                output: format!("Error starting headless run: {error}"),
                success: false,
                model: agent.model_name().to_string(),
                format: config.output_format,
                exit_on_error: config.exit_on_error,
            });
            return handle;
        }
    };
    // Construct the owner before spawn so dropping an unpolled task still
    // publishes a failure receipt and retains the Agent for close retry.
    let task_owner = HeadlessTaskOwner {
        state: Arc::clone(&state),
        fallback: Some(HeadlessResult {
            output: "Error: headless execution owner ended before settlement".to_string(),
            success: false,
            model: agent.model_name().to_string(),
            format: config.output_format.clone(),
            exit_on_error: config.exit_on_error,
        }),
    };
    let task = runtime.spawn(async move {
        let mut task_owner = task_owner;
        let mut result = execute_headless_agent(config, agent.as_ref(), cancel).await;
        if let Err(error) = state.close_once().await {
            if !result.success {
                result.output.push_str("; ");
            } else {
                result.output.clear();
            }
            result
                .output
                .push_str(&format!("Error closing agent: {error}"));
            result.success = false;
        }
        task_owner.publish(result);
    });
    // The task retains the shared state and Agent until the first close
    // attempt, and publishes a result receipt even if a waiter is cancelled.
    // Dropping only the JoinHandle does not abandon that logical owner.
    std::mem::drop(task);
    handle
}

async fn execute_headless_agent(
    config: HeadlessConfig,
    agent: &dyn Agent,
    cancel: CancellationToken,
) -> HeadlessResult {
    let model = agent.model_name().to_string();
    let exit_on_error = config.exit_on_error;
    let identity_value = format!("headless-{}", uuid::Uuid::new_v4());
    let execution = match crate::agent::EventIdentity::new(&identity_value, &identity_value) {
        Ok(identity) => {
            let request = TurnRequest::new(identity, config.prompt)
                .mode(TurnMode::Execute)
                .cancel(cancel);
            let receipt = AgentTurnDriver
                .drive(agent, request, &HeadlessEventSink)
                .await;
            headless_result_from_receipt(receipt)
        }
        Err(error) => (format!("Error: {error}"), false),
    };
    let (output, success) = execution;
    HeadlessResult {
        output,
        success,
        model,
        format: config.output_format,
        exit_on_error,
    }
}

fn headless_result_from_receipt(receipt: TurnReceipt) -> (String, bool) {
    let delivery_failure = match &receipt.delivery {
        TurnDeliveryOutcome::Failed(failure) => Some(failure.message.clone()),
        _ => None,
    };
    let delivery_ok = matches!(receipt.delivery, TurnDeliveryOutcome::Delivered);
    let (output, success) = match (receipt.outcome, receipt.final_answer, delivery_failure) {
        (TurnOutcome::Completed, Some(output), None) if delivery_ok => (output, true),
        (TurnOutcome::Completed, Some(_), Some(error)) => {
            (format!("Error: turn delivery failed: {error}"), false)
        }
        (TurnOutcome::Completed, Some(_), None) => (
            "Error: completed turn was not fully delivered".to_string(),
            false,
        ),
        (TurnOutcome::Completed, None, _) => (
            "Error: completed turn did not include a final answer".to_string(),
            false,
        ),
        (TurnOutcome::Cancelled, _, _) => ("Cancelled".to_string(), false),
        (TurnOutcome::Failed(failure), _, _) => (
            format!("Error ({}): {}", failure.code, failure.message),
            false,
        ),
    };
    (output, success)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use crate::error::{AgentFailure, ReactError};
    use crate::testing::MockLlmClient;
    use echo_core::agent::TurnId;
    use futures::StreamExt;
    use futures::future::BoxFuture;
    use futures::stream::BoxStream;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    struct ClosingAgent {
        closes: Arc<AtomicUsize>,
        fail_first_close: Arc<AtomicBool>,
    }

    impl Agent for ClosingAgent {
        fn name(&self) -> &str {
            "closing-agent"
        }

        fn model_name(&self) -> &str {
            "closing-model"
        }

        fn system_prompt(&self) -> &str {
            "test"
        }

        fn execute<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, crate::error::Result<String>> {
            Box::pin(async { Ok("done".to_string()) })
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<'a, crate::error::Result<BoxStream<'a, crate::error::Result<AgentEvent>>>>
        {
            Box::pin(async {
                Ok(
                    futures::stream::iter(vec![Ok(AgentEvent::FinalAnswer("done".to_string()))])
                        .boxed(),
                )
            })
        }

        fn close(&self) -> BoxFuture<'_, crate::error::Result<()>> {
            Box::pin(async move {
                self.closes.fetch_add(1, Ordering::AcqRel);
                if self.fail_first_close.swap(false, Ordering::AcqRel) {
                    Err(ReactError::Other("close failed".to_string()))
                } else {
                    Ok(())
                }
            })
        }
    }

    struct CancellableHeadlessAgent {
        started: Arc<tokio::sync::Notify>,
        closes: Arc<AtomicUsize>,
        closed: Arc<tokio::sync::Notify>,
    }

    impl Agent for CancellableHeadlessAgent {
        fn name(&self) -> &str {
            "cancellable-headless-agent"
        }

        fn model_name(&self) -> &str {
            "cancellable-model"
        }

        fn system_prompt(&self) -> &str {
            "test"
        }

        fn execute<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, crate::error::Result<String>> {
            Box::pin(async { Ok("done".to_string()) })
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<'a, crate::error::Result<BoxStream<'a, crate::error::Result<AgentEvent>>>>
        {
            let started = Arc::clone(&self.started);
            Box::pin(async move {
                Ok(futures::stream::once(async move {
                    started.notify_one();
                    futures::future::pending::<crate::error::Result<AgentEvent>>().await
                })
                .boxed())
            })
        }

        fn close(&self) -> BoxFuture<'_, crate::error::Result<()>> {
            Box::pin(async move {
                self.closes.fetch_add(1, Ordering::AcqRel);
                self.closed.notify_waiters();
                Ok(())
            })
        }
    }

    #[test]
    fn test_headless_config_default() {
        let config = HeadlessConfig::default();
        assert!(config.prompt.is_empty());
        assert!(config.exit_on_error);
        assert_eq!(config.output_format, "text");
        assert!(config.max_iterations.is_none());
        assert!(config.cancel_token.is_none());
    }

    #[test]
    fn test_headless_result_exit_code() {
        let ok = HeadlessResult {
            output: "done".into(),
            success: true,
            model: "test".into(),
            format: "text".into(),
            exit_on_error: true,
        };
        assert_eq!(ok.exit_code(), 0);

        let fail = HeadlessResult {
            output: "error".into(),
            success: false,
            model: "test".into(),
            format: "text".into(),
            exit_on_error: true,
        };
        assert_eq!(fail.exit_code(), 1);

        let tolerated_failure = HeadlessResult {
            output: "error".into(),
            success: false,
            model: "test".into(),
            format: "text".into(),
            exit_on_error: false,
        };
        assert_eq!(tolerated_failure.exit_code(), 0);
    }

    #[test]
    fn test_headless_result_format_json() {
        let result = HeadlessResult {
            output: "hello world".into(),
            success: true,
            model: "test-model".into(),
            format: "json".into(),
            exit_on_error: true,
        };
        let formatted = result.format_output();
        assert!(formatted.contains("\"success\": true"));
        assert!(formatted.contains("\"model\": \"test-model\""));
        assert!(formatted.contains("hello world"));
    }

    #[test]
    fn completed_execution_with_failed_delivery_is_not_headless_success() -> crate::error::Result<()>
    {
        let receipt = TurnReceipt {
            turn_id: TurnId::new("headless-delivery-failed")?,
            outcome: TurnOutcome::Completed,
            delivery: TurnDeliveryOutcome::Failed(AgentFailure::from(&ReactError::Other(
                "stdout unavailable".to_string(),
            ))),
            final_answer: Some("done".to_string()),
            final_message_id: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            llm_calls: 0,
            compaction_count: 0,
            last_event_sequence: 1,
            elapsed: Duration::ZERO,
        };
        let (output, success) = headless_result_from_receipt(receipt);
        assert!(!success);
        assert!(output.contains("turn delivery failed"));
        Ok(())
    }

    #[test]
    fn test_headless_result_format_text() {
        let result = HeadlessResult {
            output: "hello world".into(),
            success: true,
            model: "test-model".into(),
            format: "text".into(),
            exit_on_error: true,
        };
        assert_eq!(result.format_output(), "hello world");
    }

    #[tokio::test]
    async fn run_headless_uses_shared_turn_driver_terminal_contract() {
        let llm = Arc::new(MockLlmClient::new().with_response("driver result"));
        let result = run_headless(
            HeadlessConfig {
                prompt: "run through the shared driver".to_string(),
                ..HeadlessConfig::default()
            },
            |builder| builder.llm_client(llm).system_prompt("test"),
        )
        .await;

        assert!(result.success);
        assert_eq!(result.output, "driver result");
        assert_eq!(result.exit_code(), 0);
    }

    #[tokio::test]
    async fn headless_awaits_agent_close_and_surfaces_cleanup_failure() -> crate::error::Result<()>
    {
        let closes = Arc::new(AtomicUsize::new(0));
        let fail_first_close = Arc::new(AtomicBool::new(true));
        let handle = start_headless_agent(
            HeadlessConfig {
                prompt: "close after settlement".to_string(),
                ..HeadlessConfig::default()
            },
            Arc::new(ClosingAgent {
                closes: Arc::clone(&closes),
                fail_first_close,
            }),
        );
        let result = handle.wait().await;

        assert_eq!(closes.load(Ordering::Acquire), 1);
        assert!(!result.success);
        assert!(result.output.contains("close failed"));
        handle.retry_close().await?;
        assert_eq!(closes.load(Ordering::Acquire), 2);
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_headless_waiter_keeps_owned_cleanup_running() -> crate::error::Result<()> {
        let started = Arc::new(tokio::sync::Notify::new());
        let closes = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(tokio::sync::Notify::new());
        let parent_cancel = CancellationToken::new();
        let sibling_cancel = parent_cancel.child_token();
        let handle = start_headless_agent(
            HeadlessConfig {
                prompt: "wait until cancelled".to_string(),
                cancel_token: Some(parent_cancel.clone()),
                ..HeadlessConfig::default()
            },
            Arc::new(CancellableHeadlessAgent {
                started: Arc::clone(&started),
                closes: Arc::clone(&closes),
                closed: Arc::clone(&closed),
            }),
        );
        let waiter_handle = handle.clone();
        let waiter = tokio::spawn(async move { wait_headless_handle(waiter_handle).await });
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .map_err(|_| ReactError::Other("Headless turn did not start".to_string()))?;
        let retry_error = handle.retry_close().await.err().ok_or_else(|| {
            ReactError::Other("close retry was accepted before execution settled".to_string())
        })?;
        assert!(
            retry_error
                .to_string()
                .contains("before Headless execution settles")
        );
        assert_eq!(closes.load(Ordering::Acquire), 0);
        let closed_wait = closed.notified();
        tokio::pin!(closed_wait);
        closed_wait.as_mut().enable();
        waiter.abort();
        let _ = waiter.await;
        tokio::time::timeout(Duration::from_secs(2), closed_wait)
            .await
            .map_err(|_| ReactError::Other("cancelled waiter abandoned Agent close".to_string()))?;
        assert_eq!(closes.load(Ordering::Acquire), 1);
        assert!(!parent_cancel.is_cancelled());
        assert!(!sibling_cancel.is_cancelled());
        let result = handle.wait().await;
        assert!(!result.success);
        assert!(result.output.contains("Cancelled"));
        Ok(())
    }

    #[test]
    fn missing_runtime_returns_failure_without_dropping_close_owner() -> crate::error::Result<()> {
        let closes = Arc::new(AtomicUsize::new(0));
        let handle = start_headless_agent(
            HeadlessConfig {
                prompt: "requires a runtime".to_string(),
                ..HeadlessConfig::default()
            },
            Arc::new(ClosingAgent {
                closes: Arc::clone(&closes),
                fail_first_close: Arc::new(AtomicBool::new(false)),
            }),
        );
        let result = handle.state.result().ok_or_else(|| {
            ReactError::Other("missing-runtime failure was not published".to_string())
        })?;
        assert!(!result.success);
        assert!(result.output.contains("Error starting headless run"));
        assert_eq!(closes.load(Ordering::Acquire), 0);

        let runtime = tokio::runtime::Runtime::new()
            .map_err(|error| ReactError::Other(format!("test runtime failed: {error}")))?;
        runtime.block_on(handle.retry_close())?;
        assert_eq!(closes.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[test]
    fn runtime_drop_before_first_poll_publishes_failure_and_retains_owner()
    -> crate::error::Result<()> {
        let closes = Arc::new(AtomicUsize::new(0));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| ReactError::Other(format!("test runtime failed: {error}")))?;
        let enter_guard = runtime.enter();
        let handle = start_headless_agent(
            HeadlessConfig {
                prompt: "runtime drops before polling".to_string(),
                ..HeadlessConfig::default()
            },
            Arc::new(ClosingAgent {
                closes: Arc::clone(&closes),
                fail_first_close: Arc::new(AtomicBool::new(false)),
            }),
        );
        drop(enter_guard);
        drop(runtime);

        let result = handle.state.result().ok_or_else(|| {
            ReactError::Other("unpolled task did not publish a failure receipt".to_string())
        })?;
        assert!(!result.success);
        assert!(result.output.contains("owner ended before settlement"));
        assert_eq!(closes.load(Ordering::Acquire), 0);

        let retry_runtime = tokio::runtime::Runtime::new()
            .map_err(|error| ReactError::Other(format!("retry runtime failed: {error}")))?;
        retry_runtime.block_on(handle.retry_close())?;
        assert_eq!(closes.load(Ordering::Acquire), 1);
        Ok(())
    }
}
