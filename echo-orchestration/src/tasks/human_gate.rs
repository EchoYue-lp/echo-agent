//! Human checkpoint gate — pauses task pipelines for user input.
//!
//! Provides a generic request/response mechanism where a running task can
//! park itself and wait for human approval, revision instructions, or
//! cancellation before continuing.
//!
//! # Example
//!
//! ```rust,ignore
//! use echo_orchestration::tasks::human_gate::{HumanGate, HumanRequest, HumanResponse};
//! use tokio_util::sync::CancellationToken;
//!
//! let gate = HumanGate::new();
//! let cancel = CancellationToken::new();
//!
//! // In the task's execute_fn:
//! let response = gate.request("task-1", HumanRequest {
//!     prompt: "Review the draft and approve or request changes".into(),
//!     context: serde_json::json!({ "draft": "..." }),
//!     options: vec!["Approve".into(), "Revise".into(), "Cancel".into()],
//!     phase: "review".into(),
//! }, &cancel).await?;
//!
//! // In the CLI/web frontend:
//! gate.respond("task-1", HumanResponse {
//!     selection: "Approve".into(),
//!     instructions: None,
//! }).await;
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

// ── Types ──────────────────────────────────────────────────────────

/// A request for human input at a checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanRequest {
    /// The question or prompt to display to the human.
    pub prompt: String,
    /// Arbitrary context data (e.g., draft content, search results).
    pub context: serde_json::Value,
    /// Available response options (e.g., ["Approve", "Revise", "Cancel"]).
    pub options: Vec<String>,
    /// Name of the phase waiting for input.
    pub phase: String,
}

/// A response from the human.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanResponse {
    /// The selected option.
    pub selection: String,
    /// Optional free-text instructions.
    pub instructions: Option<String>,
}

/// Internal pending request state.
struct PendingRequest {
    /// The original request.
    request: HumanRequest,
    /// Channel to send the response back to the waiting task.
    sender: oneshot::Sender<HumanResponse>,
}

// ── HumanGate ──────────────────────────────────────────────────────

/// Gate that blocks a task pipeline until human input is received.
///
/// Thread-safe: can be shared across tasks and frontends via `Arc`.
pub struct HumanGate {
    pending: Arc<Mutex<HashMap<String, PendingRequest>>>,
}

impl HumanGate {
    /// Create a new human gate.
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Request human input for a task. Blocks until:
    /// 1. A response is received via [`respond`](Self::respond), or
    /// 2. The `cancel` token is fired.
    ///
    /// Returns the human's response, or an error if cancelled.
    pub async fn request(
        &self,
        task_id: &str,
        request: HumanRequest,
        cancel: &CancellationToken,
    ) -> echo_core::error::Result<HumanResponse> {
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            pending.insert(
                task_id.to_string(),
                PendingRequest {
                    request,
                    sender: tx,
                },
            );
        }

        // Wait for response or cancellation
        tokio::select! {
            result = rx => {
                // Clean up
                self.pending.lock().await.remove(task_id);
                result.map_err(|_| {
                    echo_core::error::ReactError::Other(
                        format!("Human gate channel closed for task '{task_id}'")
                    )
                })
            }
            _ = cancel.cancelled() => {
                self.pending.lock().await.remove(task_id);
                Err(echo_core::error::ReactError::Other(
                    format!("Human gate cancelled for task '{task_id}'")
                ))
            }
        }
    }

    /// Respond to a pending human checkpoint request.
    ///
    /// Returns `true` if a pending request was found and responded to,
    /// `false` if no request was pending for the given task ID.
    pub async fn respond(&self, task_id: &str, response: HumanResponse) -> bool {
        let mut pending = self.pending.lock().await;
        if let Some(req) = pending.remove(task_id) {
            let _ = req.sender.send(response);
            true
        } else {
            false
        }
    }

    /// List all pending human checkpoint requests.
    ///
    /// Returns `(task_id, request)` pairs for each waiting task.
    pub async fn pending(&self) -> Vec<(String, HumanRequest)> {
        let pending = self.pending.lock().await;
        pending
            .iter()
            .map(|(id, req)| (id.clone(), req.request.clone()))
            .collect()
    }

    /// Number of pending requests.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }
}

impl Default for HumanGate {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for HumanGate {
    fn clone(&self) -> Self {
        Self {
            pending: self.pending.clone(),
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> HumanRequest {
        HumanRequest {
            prompt: "Review draft".into(),
            context: serde_json::json!({ "content": "Hello world" }),
            options: vec!["Approve".into(), "Revise".into()],
            phase: "review".into(),
        }
    }

    #[tokio::test]
    async fn test_request_and_respond() {
        let gate = HumanGate::new();
        let cancel = CancellationToken::new();

        let gate_clone = gate.clone();
        let cancel_clone = cancel.clone();

        // Spawn the request in background
        let handle = tokio::spawn(async move {
            gate_clone
                .request("task-1", sample_request(), &cancel_clone)
                .await
        });

        // Give the request time to register
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Check pending
        let pending = gate.pending().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "task-1");
        assert_eq!(pending[0].1.phase, "review");

        // Respond
        let ok = gate
            .respond(
                "task-1",
                HumanResponse {
                    selection: "Approve".into(),
                    instructions: None,
                },
            )
            .await;
        assert!(ok);

        // The request should complete
        let result = handle.await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.unwrap().selection, "Approve");
    }

    #[tokio::test]
    async fn test_cancel_request() {
        let gate = HumanGate::new();
        let cancel = CancellationToken::new();

        let gate_clone = gate.clone();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            gate_clone
                .request("task-2", sample_request(), &cancel_clone)
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Cancel
        cancel.cancel();

        let result = handle.await.unwrap();
        assert!(result.is_err());

        // No pending requests
        assert_eq!(gate.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_respond_nonexistent() {
        let gate = HumanGate::new();
        let ok = gate
            .respond(
                "nonexistent",
                HumanResponse {
                    selection: "X".into(),
                    instructions: None,
                },
            )
            .await;
        assert!(!ok);
    }
}
