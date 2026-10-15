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
#[derive(Clone)]
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

    // Outgoing views may own the original close authority. The state's stored
    // observation is always owner-free, avoiding state -> result -> state.
    retained_owner: Option<HeadlessRunHandle>,
}

impl std::fmt::Debug for HeadlessResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HeadlessResult")
            .field("output", &self.output)
            .field("success", &self.success)
            .field("model", &self.model)
            .field("format", &self.format)
            .field("exit_on_error", &self.exit_on_error)
            .finish()
    }
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

    fn publish(&self, mut result: HeadlessResult) {
        // Strip ownership before taking the result lock. Only an outward wait
        // may attach a handle; storing one here would create a strong cycle.
        result.retained_owner = None;
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
            if let Some(mut result) = self.state.result() {
                if self
                    .state
                    .close_owner
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .is_some()
                {
                    result.retained_owner = Some(self.clone());
                }
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
    /// Construct a value-only result with the original five public values.
    ///
    /// Runtime observations attach their original close owner only when an
    /// existing [`HeadlessRunHandle`] hands the result to its caller.
    pub fn new(
        output: String,
        success: bool,
        model: String,
        format: String,
        exit_on_error: bool,
    ) -> Self {
        Self {
            output,
            success,
            model,
            format,
            exit_on_error,
            retained_owner: None,
        }
    }

    /// Borrow the same original owner after an incomplete Agent close.
    ///
    /// Retain this result or clone its handle while uncertainty is relevant.
    /// This is not a physical receipt or permission to replay an execution.
    /// If waiting itself may be cancelled, use [`start_headless`] and retain
    /// its handle before waiting; no outgoing result exists before return.
    pub fn retained_owner(&self) -> Option<&HeadlessRunHandle> {
        self.retained_owner.as_ref()
    }

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
            retained_owner: None,
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
                retained_owner: None,
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
                retained_owner: None,
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
            retained_owner: None,
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
            match receipt {
                Ok(receipt) => headless_result_from_receipt(receipt),
                // This output reports an observation; it does not manufacture
                // a terminal receipt or settle the framework's retained owner.
                Err(error) => (format!("Error: {error}"), false),
            }
        }
        Err(error) => (format!("Error: {error}"), false),
    };
    let (output, success) = execution;
    HeadlessResult {
        retained_owner: None,
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
    use std::future::Future;
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
        release: Arc<tokio::sync::Notify>,
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
            let release = Arc::clone(&self.release);
            Box::pin(async move {
                Ok(futures::stream::once(async move {
                    started.notify_one();
                    release.notified().await;
                    None::<crate::error::Result<AgentEvent>>
                })
                .filter_map(futures::future::ready)
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
            retained_owner: None,
            output: "done".into(),
            success: true,
            model: "test".into(),
            format: "text".into(),
            exit_on_error: true,
        };
        assert_eq!(ok.exit_code(), 0);

        let fail = HeadlessResult {
            retained_owner: None,
            output: "error".into(),
            success: false,
            model: "test".into(),
            format: "text".into(),
            exit_on_error: true,
        };
        assert_eq!(fail.exit_code(), 1);

        let tolerated_failure = HeadlessResult {
            retained_owner: None,
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
            retained_owner: None,
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
            delivery: TurnDeliveryOutcome::Failed(AgentFailure::try_from(&ReactError::Other(
                "stdout unavailable".to_string(),
            ))?),
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
            retained_owner: None,
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
        assert!(result.retained_owner().is_none());
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
        let release = Arc::new(tokio::sync::Notify::new());
        let waiter_pending = Arc::new(tokio::sync::Notify::new());
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
                release: Arc::clone(&release),
                closes: Arc::clone(&closes),
                closed: Arc::clone(&closed),
            }),
        );
        let waiter_handle = handle.clone();
        let first_pending = Arc::clone(&waiter_pending);
        let waiter = tokio::spawn(async move {
            let wait = wait_headless_handle(waiter_handle);
            tokio::pin!(wait);
            let mut first_pending = Some(first_pending);
            futures::future::poll_fn(|cx| {
                let result = wait.as_mut().poll(cx);
                if result.is_pending()
                    && let Some(signal) = first_pending.take()
                {
                    signal.notify_one();
                }
                result
            })
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), waiter_pending.notified())
            .await
            .map_err(|_| ReactError::Other("Headless waiter was not polled".to_string()))?;
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
        assert!(waiter.await.is_err_and(|error| error.is_cancelled()));
        assert!(handle.state.cancel.is_cancelled());
        assert!(handle.state.result().is_none());
        assert_eq!(closes.load(Ordering::Acquire), 0);
        assert!(!parent_cancel.is_cancelled());
        assert!(!sibling_cancel.is_cancelled());
        // Cancellation stops observation, not this source's physical cleanup.
        // The retained owner may close the Agent only after the original EOF.
        release.notify_one();
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

    type HeadlessProducerJoin =
        futures::future::Shared<BoxFuture<'static, std::result::Result<(), String>>>;

    struct HeadlessResourceProof {
        _permit: tokio::sync::OwnedSemaphorePermit,
        drops: Arc<AtomicUsize>,
    }

    impl Drop for HeadlessResourceProof {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::AcqRel);
        }
    }

    struct HeadlessUnknownTool {
        effect: std::path::PathBuf,
        release: Arc<tokio::sync::Notify>,
        producer: Arc<std::sync::Mutex<Option<HeadlessProducerJoin>>>,
        _resource: echo_core::tools::InvocationResourceGuard,
    }

    impl crate::tools::Tool for HeadlessUnknownTool {
        fn name(&self) -> &str {
            "headless_unknown"
        }

        fn description(&self) -> &str {
            "reports an admitted physical producer whose settlement is not yet known"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }

        fn execute<'a>(
            &'a self,
            _parameters: crate::tools::ToolParameters,
        ) -> BoxFuture<'a, crate::error::Result<crate::tools::ToolResult>> {
            use futures::FutureExt;
            Box::pin(async move {
                let effect = self.effect.clone();
                let release = self.release.clone();
                let (ready, entered) = tokio::sync::oneshot::channel();
                let task = tokio::spawn(async move {
                    tokio::fs::write(&effect, b"producer-entered")
                        .await
                        .map_err(|error| error.to_string())?;
                    let _ = ready.send(());
                    release.notified().await;
                    tokio::fs::write(&effect, b"producer-physically-closed")
                        .await
                        .map_err(|error| error.to_string())?;
                    Ok::<(), String>(())
                });
                let joined = async move { task.await.map_err(|error| error.to_string())? }
                    .boxed()
                    .shared();
                *self
                    .producer
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some(joined);
                entered
                    .await
                    .map_err(|error| ReactError::Other(error.to_string()))?;
                Err(ReactError::ExecutionNotSettled {
                    run_id: "headless-original-run".to_string(),
                    execution_id: Some("headless-original-execution".to_string()),
                })
            })
        }
    }

    #[tokio::test]
    async fn convenience_headless_retains_the_only_owner_of_an_unsettled_tool()
    -> crate::error::Result<()> {
        let directory =
            tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let effect = directory.path().join("physical-effect.txt");
        let release = Arc::new(tokio::sync::Notify::new());
        let producer = Arc::new(std::sync::Mutex::new(None::<HeadlessProducerJoin>));
        let drops = Arc::new(AtomicUsize::new(0));
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = capacity
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let tool = HeadlessUnknownTool {
            effect: effect.clone(),
            release: release.clone(),
            producer: producer.clone(),
            _resource: echo_core::tools::InvocationResourceGuard::new(HeadlessResourceProof {
                _permit: permit,
                drops: drops.clone(),
            }),
        };
        let llm = Arc::new(
            MockLlmClient::new()
                .then_tool_call("headless-original-call", "headless_unknown", "{}")
                .with_response("must not continue"),
        );
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            run_headless(
                HeadlessConfig {
                    prompt: "run the admitted tool".to_string(),
                    ..HeadlessConfig::default()
                },
                |builder| {
                    builder
                        .llm_client(llm.clone())
                        .system_prompt("test")
                        .tool(Box::new(tool))
                },
            ),
        )
        .await;
        let held_drops = drops.load(Ordering::Acquire);
        let held_capacity = capacity.available_permits();
        let held_effect = tokio::fs::read(&effect).await;
        let original = producer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        // Always release and join the same real producer before asserting the
        // observed regression, including on the unfixed early-return path.
        release.notify_one();
        let original = original
            .ok_or_else(|| ReactError::Other("original producer was not admitted".to_string()))?;
        tokio::time::timeout(Duration::from_secs(2), original)
            .await
            .map_err(|_| {
                ReactError::Other("original producer did not physically close".to_string())
            })?
            .map_err(ReactError::Other)?;
        let result = result
            .map_err(|_| ReactError::Other("Headless observation did not return".to_string()))?;
        assert!(!result.success);
        assert!(result.output.contains("headless-original-run"));
        assert!(result.output.contains("Error closing agent"));
        assert_eq!(llm.call_count(), 1);
        assert_eq!(
            held_effect.map_err(|error| ReactError::Other(error.to_string()))?,
            b"producer-entered"
        );
        assert_eq!(
            tokio::fs::read(&effect)
                .await
                .map_err(|error| ReactError::Other(error.to_string()))?,
            b"producer-physically-closed"
        );
        assert_eq!(
            held_drops, 0,
            "observation return dropped the only resource owner while the original producer was still alive"
        );
        assert_eq!(
            held_capacity, 0,
            "observation return released the original live-producer capacity"
        );
        let clone = result.clone();
        let original = result.retained_owner().cloned().ok_or_else(|| {
            ReactError::Other("observation lost its original owner handle".to_string())
        })?;
        let cloned = clone.retained_owner().ok_or_else(|| {
            ReactError::Other("cloning the view lost its original owner".to_string())
        })?;
        assert!(Arc::ptr_eq(&original.state, &cloned.state));
        assert!(
            original
                .state
                .result()
                .is_some_and(|stored| stored.retained_owner().is_none())
        );
        let weak = Arc::downgrade(&original.state);
        drop(result);
        drop(clone);
        assert_eq!(drops.load(Ordering::Acquire), 0);
        assert_eq!(capacity.available_permits(), 0);
        // The physical producer was already released and joined above. Drop
        // the last explicit parent only now, without inventing a close proof.
        drop(original);
        // A reporter on a multi-thread runtime may still be returning from
        // publish. Observe its actual release without interpreting that tail
        // as an ownership cycle or changing the physical assertions above.
        tokio::time::timeout(Duration::from_secs(2), async {
            while weak.strong_count() != 0
                || drops.load(Ordering::Acquire) != 1
                || capacity.available_permits() != 1
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("stored observation retained an owner cycle".to_string()))?;
        assert!(weak.upgrade().is_none());
        assert_eq!(drops.load(Ordering::Acquire), 1);
        assert_eq!(capacity.available_permits(), 1);
        assert_eq!(llm.call_count(), 1);
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
