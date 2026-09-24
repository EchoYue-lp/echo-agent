pub mod http;
pub mod sse;
pub mod stdio;

use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{Notify, oneshot};
use tokio::time::Instant;

use super::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::redaction::{json_with_secrets, text_with_secrets};
use echo_core::error::{McpError, ReactError, Result};

/// MCP 传输层抽象
///
/// 负责在 Client 和 Server 之间传递 JSON-RPC 消息，
/// 屏蔽底层通信细节（进程 stdin/stdout 或 HTTP）。
pub trait McpTransport: Send + Sync {
    /// 发送请求并等待响应（传输层自动管理请求 ID）
    fn send(&self, request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>>;

    /// 发送通知（无需等待响应）
    fn notify(&self, notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>>;

    /// Close the connection after all transport-owned cleanup settles.
    ///
    /// Successful return is a lifecycle safe point: no request remains in the
    /// transport's pending registry and owned I/O tasks or children have
    /// reached their bounded close terminal.
    fn close(&self) -> BoxFuture<'_, Result<()>>;

    /// 获取通知接收通道（用于接收服务端推送的通知）
    /// 返回 None 表示该传输层不支持通知接收
    fn notification_rx(&self) -> Option<Arc<dyn super::types::JsonRpcNotificationReceiver>>;
}

type PendingSender = oneshot::Sender<Result<JsonRpcResponse>>;

/// Cancellation-safe registry shared by request/response transports.
///
/// The synchronous mutex is intentional: registrations are never held across
/// an await, which lets the registration guard remove an abandoned request in
/// `Drop` without spawning another cleanup task.
pub(super) struct PendingRequests {
    closed: AtomicBool,
    entries: Mutex<HashMap<u64, PendingSender>>,
}

pub(super) struct PendingRegistration {
    id: u64,
    pending: Arc<PendingRequests>,
}

#[derive(Clone)]
enum CloseStatus {
    Open,
    Closing,
    Settled,
    Failed(String),
}

/// Single-flight close receipt whose cleanup producer survives caller drop.
pub(super) struct CloseCoordinator {
    current: Mutex<Arc<CloseReceipt>>,
}

pub(super) struct CloseReceipt {
    status: Mutex<CloseStatus>,
    producer: Mutex<Option<tokio::task::JoinHandle<()>>>,
    changed: Notify,
}

impl CloseCoordinator {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            current: Mutex::new(Arc::new(CloseReceipt {
                status: Mutex::new(CloseStatus::Open),
                producer: Mutex::new(None),
                changed: Notify::new(),
            })),
        })
    }

    pub(super) fn begin(&self) -> (bool, Arc<CloseReceipt>) {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let status = current.status().clone();
        let start = match status {
            CloseStatus::Open => {
                *current.status() = CloseStatus::Closing;
                true
            }
            CloseStatus::Failed(_) => {
                *current = Arc::new(CloseReceipt {
                    status: Mutex::new(CloseStatus::Closing),
                    producer: Mutex::new(None),
                    changed: Notify::new(),
                });
                true
            }
            CloseStatus::Closing if current.producer_finished_without_receipt() => {
                *current = Arc::new(CloseReceipt {
                    status: Mutex::new(CloseStatus::Closing),
                    producer: Mutex::new(None),
                    changed: Notify::new(),
                });
                true
            }
            CloseStatus::Closing | CloseStatus::Settled => false,
        };
        (start, Arc::clone(&current))
    }
}

impl CloseReceipt {
    fn producer_finished_without_receipt(&self) -> bool {
        self.producer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
    }

    pub(super) fn retain_producer(&self, producer: tokio::task::JoinHandle<()>) {
        *self
            .producer
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(producer);
    }

    fn status(&self) -> MutexGuard<'_, CloseStatus> {
        self.status
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub(super) fn complete(&self, result: &Result<()>) {
        *self.status() = match result {
            Ok(()) => CloseStatus::Settled,
            Err(error) => CloseStatus::Failed(error.to_string()),
        };
        self.changed.notify_waiters();
    }

    pub(super) async fn wait(&self, deadline: Instant, label: &str) -> Result<()> {
        loop {
            let changed = self.changed.notified();
            let status = self.status().clone();
            match status {
                CloseStatus::Open => {
                    return Err(close_error(format!(
                        "{label} close coordinator was not started"
                    )));
                }
                CloseStatus::Settled => return Ok(()),
                CloseStatus::Failed(error) => return Err(close_error(error)),
                CloseStatus::Closing => {
                    tokio::time::timeout_at(deadline, changed)
                        .await
                        .map_err(|_| {
                            close_error(format!(
                                "{label} close receipt did not settle before deadline"
                            ))
                        })?;
                }
            }
        }
    }
}

impl PendingRequests {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            closed: AtomicBool::new(false),
            entries: Mutex::new(HashMap::new()),
        })
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<u64, PendingSender>> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub(super) fn register(
        self: &Arc<Self>,
        id: u64,
    ) -> Result<(
        oneshot::Receiver<Result<JsonRpcResponse>>,
        PendingRegistration,
    )> {
        let (sender, receiver) = oneshot::channel();
        let mut entries = self.entries();
        if self.closed.load(Ordering::Acquire) {
            return Err(transport_closed_error());
        }
        entries.insert(id, sender);
        drop(entries);
        Ok((
            receiver,
            PendingRegistration {
                id,
                pending: Arc::clone(self),
            },
        ))
    }

    pub(super) fn resolve(&self, id: u64, response: JsonRpcResponse) {
        if let Some(sender) = self.entries().remove(&id) {
            let _ = sender.send(Ok(response));
        }
    }

    /// Fence new requests and fail every request already accepted.
    pub(super) fn close(&self) -> usize {
        self.closed.store(true, Ordering::Release);
        let entries = std::mem::take(&mut *self.entries());
        let count = entries.len();
        for sender in entries.into_values() {
            let _ = sender.send(Err(transport_closed_error()));
        }
        count
    }

    pub(super) fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            Err(transport_closed_error())
        } else {
            Ok(())
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries().len()
    }
}

impl Drop for PendingRegistration {
    fn drop(&mut self) {
        self.pending.entries().remove(&self.id);
    }
}

pub(super) fn transport_closed_error() -> ReactError {
    ReactError::Mcp(Box::new(McpError::TransportClosed))
}

pub(super) fn close_error(message: impl Into<String>) -> ReactError {
    ReactError::Mcp(Box::new(McpError::ConnectionFailed(message.into())))
}

pub(super) fn redact_response_error(response: &mut JsonRpcResponse, secrets: &[String]) {
    let Some(error) = response.error.as_mut() else {
        return;
    };
    error.message = text_with_secrets(&error.message, secrets.iter());
    if let Some(data) = error.data.as_mut() {
        *data = json_with_secrets(data, secrets);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::types::JsonRpcError;

    #[tokio::test]
    async fn retry_cannot_overwrite_original_waiters_failure() -> Result<()> {
        let coordinator = CloseCoordinator::new();
        let (start, first) = coordinator.begin();
        assert!(start);
        let (start, waiter) = coordinator.begin();
        assert!(!start);
        let deadline = Instant::now() + std::time::Duration::from_secs(1);
        let waiting = waiter.wait(deadline, "original");
        tokio::pin!(waiting);
        assert!(futures::poll!(&mut waiting).is_pending());
        first.complete(&Err(close_error("original failure")));
        let (start, retry) = coordinator.begin();
        assert!(start);
        retry.complete(&Ok(()));
        let error = waiting
            .await
            .err()
            .ok_or_else(|| close_error("lost original failure"))?;
        assert!(error.to_string().contains("original failure"));
        retry.wait(deadline, "retry").await?;
        assert!(first.wait(deadline, "first").await.is_err());
        Ok(())
    }

    #[test]
    fn transport_error_redaction_removes_configured_values() {
        let mut response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: "server echoed opaque-secret".to_string(),
                data: Some(serde_json::json!({"detail": "opaque-secret"})),
            }),
        };
        redact_response_error(&mut response, &["opaque-secret".to_string()]);
        let debug = format!("{response:?}");
        assert!(!debug.contains("opaque-secret"));
    }

    #[tokio::test]
    async fn abandoned_registration_is_removed_without_async_cleanup() -> Result<()> {
        let pending = PendingRequests::new();
        let (_receiver, registration) = pending.register(7)?;
        assert_eq!(pending.len(), 1);

        drop(registration);

        assert_eq!(pending.len(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn close_fails_every_pending_request_and_fences_new_registration() -> Result<()> {
        let pending = PendingRequests::new();
        let (first, _first_registration) = pending.register(1)?;
        let (second, _second_registration) = pending.register(2)?;

        assert_eq!(pending.close(), 2);
        assert!(matches!(first.await, Ok(Err(ReactError::Mcp(_)))));
        assert!(matches!(second.await, Ok(Err(ReactError::Mcp(_)))));
        assert!(pending.register(3).is_err());
        Ok(())
    }
}
