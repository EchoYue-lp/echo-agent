//! Bounded facade stream bookkeeping (plan 07, todo 2).
//!
//! Facade streams are family-owned event/data streams (workflow progress,
//! eval reports, subagent envelopes, …) opened over a facade resource.
//! This registry owns only the addressing lifecycle — open/close bounds,
//! sequence watermarks, cancellation and page limits — mirroring the
//! generation-fenced ladder of [`super::handles::HandleRegistry`]. Payload
//! flow stays with the family adapters (todos 3–5); the Host never becomes
//! a data authority (design §10.4).
//!
//! Deterministic edge behavior (plan verify): repeated close is idempotent
//! (`false`), cancel after close is a no-op, advancing a closed/cancelled
//! stream is a typed `closed_handle`/`cancelled` outcome, sequences are
//! strictly monotonic, and every bound (open streams, page size) rejects
//! before allocation.

use std::collections::HashMap;
use std::sync::Mutex;

use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};

use crate::core_profile::wire::sdk_error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FacadeStreamLimits {
    pub max_open_streams: usize,
    pub max_page_items: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FacadeStreamRecord {
    /// Facade resource id that owns this stream.
    pub resource_id: String,
    /// Highest sequence delivered through this stream.
    pub last_sequence: u64,
    pub cancelled: bool,
}

#[derive(Default)]
struct FacadeStreamInner {
    streams: HashMap<String, FacadeStreamRecord>,
}

#[allow(dead_code)]
pub(crate) struct FacadeStreamBookkeeping {
    limits: FacadeStreamLimits,
    inner: Mutex<FacadeStreamInner>,
}

#[allow(dead_code)]
impl FacadeStreamBookkeeping {
    pub fn new(limits: FacadeStreamLimits) -> Self {
        Self {
            limits,
            inner: Mutex::new(FacadeStreamInner::default()),
        }
    }

    pub fn limits(&self) -> FacadeStreamLimits {
        self.limits
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FacadeStreamInner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Open a stream over an existing facade resource. The stream id is
    /// minted once and never reused; reopening the same id is a typed
    /// conflict.
    pub fn open(
        &self,
        stream_id: String,
        resource_id: &str,
        operation: &str,
    ) -> Result<FacadeStreamRecord, EchoSdkError> {
        let mut inner = self.lock();
        if inner.streams.contains_key(&stream_id) {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidRequest,
                "facade stream id is already open",
                Retryability::AfterDelay,
                operation,
            ));
        }
        if inner.streams.len() >= self.limits.max_open_streams {
            return Err(sdk_error(
                ExtensionErrorCode::PayloadTooLarge,
                format!(
                    "open facade stream limit {} reached",
                    self.limits.max_open_streams
                ),
                Retryability::AfterDelay,
                operation,
            ));
        }
        let record = FacadeStreamRecord {
            resource_id: resource_id.to_string(),
            last_sequence: 0,
            cancelled: false,
        };
        inner.streams.insert(stream_id, record.clone());
        Ok(record)
    }

    /// Resolve one stream record.
    pub fn get(
        &self,
        stream_id: &str,
        operation: &str,
    ) -> Result<FacadeStreamRecord, EchoSdkError> {
        self.lock().streams.get(stream_id).cloned().ok_or_else(|| {
            sdk_error(
                ExtensionErrorCode::InvalidValue,
                "facade stream was never opened by this connection",
                Retryability::Never,
                operation,
            )
        })
    }

    /// Advance the sequence watermark. Strictly monotonic: replaying or
    /// going backwards is a typed invalid request, and cancelled streams
    /// stop accepting advances.
    pub fn advance(
        &self,
        stream_id: &str,
        sequence: u64,
        operation: &str,
    ) -> Result<(), EchoSdkError> {
        let mut inner = self.lock();
        let Some(record) = inner.streams.get_mut(stream_id) else {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidValue,
                "facade stream was never opened by this connection",
                Retryability::Never,
                operation,
            ));
        };
        if record.cancelled {
            return Err(sdk_error(
                ExtensionErrorCode::Cancelled,
                "facade stream is cancelled",
                Retryability::Never,
                operation,
            ));
        }
        if sequence <= record.last_sequence {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidRequest,
                format!(
                    "facade stream sequence {sequence} does not advance past {}",
                    record.last_sequence
                ),
                Retryability::Never,
                operation,
            ));
        }
        record.last_sequence = sequence;
        Ok(())
    }

    /// Cooperative cancellation; idempotent. Returns whether this call
    /// flipped the stream to cancelled.
    pub fn cancel(&self, stream_id: &str) -> bool {
        self.lock()
            .streams
            .get_mut(stream_id)
            .is_some_and(|record| {
                if record.cancelled {
                    false
                } else {
                    record.cancelled = true;
                    true
                }
            })
    }

    /// Close one stream; idempotent (`false` when already closed).
    pub fn close(&self, stream_id: &str) -> bool {
        self.lock().streams.remove(stream_id).is_some()
    }

    /// Close every stream owned by one resource (resource close cascade).
    pub fn close_of_resource(&self, resource_id: &str) -> usize {
        let mut inner = self.lock();
        let before = inner.streams.len();
        inner
            .streams
            .retain(|_, record| record.resource_id != resource_id);
        before.saturating_sub(inner.streams.len())
    }

    /// Streams currently open for one resource.
    pub fn open_count_of_resource(&self, resource_id: &str) -> usize {
        self.lock()
            .streams
            .values()
            .filter(|record| record.resource_id == resource_id)
            .count()
    }

    /// Bounded page size shared by every paginated family query.
    pub fn clamp_page(&self, requested: usize) -> usize {
        requested.min(self.limits.max_page_items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bookkeeping() -> FacadeStreamBookkeeping {
        FacadeStreamBookkeeping::new(FacadeStreamLimits {
            max_open_streams: 2,
            max_page_items: 16,
        })
    }

    #[test]
    fn open_resolve_advance_and_close_are_deterministic() {
        let streams = bookkeeping();
        let record = streams
            .open("fs-1".to_string(), "res-1", "test")
            .expect("open");
        assert_eq!(record.resource_id, "res-1");
        assert!(streams.advance("fs-1", 1, "test").is_ok());
        // Sequences must strictly advance.
        assert!(
            streams
                .advance("fs-1", 1, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::InvalidRequest)
        );
        assert!(streams.advance("fs-1", 2, "test").is_ok());
        // Duplicate open is a typed conflict.
        assert!(
            streams
                .open("fs-1".to_string(), "res-1", "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::InvalidRequest)
        );
        // Close is idempotent; a closed stream no longer resolves.
        assert!(streams.close("fs-1"));
        assert!(!streams.close("fs-1"));
        assert!(
            streams
                .get("fs-1", "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::InvalidValue)
        );
    }

    #[test]
    fn open_stream_bound_rejects_before_allocation() {
        let streams = bookkeeping();
        assert!(streams.open("fs-1".to_string(), "res-1", "test").is_ok());
        assert!(streams.open("fs-2".to_string(), "res-1", "test").is_ok());
        assert!(
            streams
                .open("fs-3".to_string(), "res-1", "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::PayloadTooLarge)
        );
    }

    #[test]
    fn cancel_is_idempotent_and_blocks_advances() {
        let streams = bookkeeping();
        streams
            .open("fs-1".to_string(), "res-1", "test")
            .expect("open");
        assert!(streams.advance("fs-1", 5, "test").is_ok());
        assert!(streams.cancel("fs-1"));
        assert!(!streams.cancel("fs-1"));
        assert!(
            streams
                .advance("fs-1", 6, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::Cancelled)
        );
    }

    #[test]
    fn resource_close_cascades_to_its_streams_only() {
        let streams = bookkeeping();
        streams
            .open("fs-1".to_string(), "res-1", "test")
            .expect("open");
        streams
            .open("fs-2".to_string(), "res-2", "test")
            .expect("open");
        assert_eq!(streams.close_of_resource("res-1"), 1);
        assert_eq!(streams.open_count_of_resource("res-2"), 1);
        assert_eq!(streams.open_count_of_resource("res-1"), 0);
        // Unknown resource closes are a no-op.
        assert_eq!(streams.close_of_resource("res-none"), 0);
    }

    #[test]
    fn page_clamp_is_bounded() {
        let streams = bookkeeping();
        assert_eq!(streams.clamp_page(4), 4);
        assert_eq!(streams.clamp_page(1024), 16);
    }
}
