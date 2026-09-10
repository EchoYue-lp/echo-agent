//! Pull-based facade streams over the unified [`HandleRegistry`].
//!
//! Framework stream producers run against their existing Rust authority and
//! publish into a capacity-one queue. An SDK `stream.next` request consumes
//! one item, which provides transport backpressure without buffering an
//! unbounded response. Handle identity, owner, generation, sequence,
//! cancellation and close state remain exclusively in `HandleRegistry`;
//! this module stores only the live receiver and its producer task.

use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::handle::WireHandle;
use echo_sdk_protocol::methods::FeatureOperationRequest;
use echo_sdk_protocol::scalar::{WireField, WireU64, WireValue};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::super::handles::HandleRegistry;
use super::super::wire;

const STREAM_EVENT_TYPE_ID: &str = "echo_sdk::FacadeStreamEvent";

pub(crate) type FacadeStreamItem = Result<WireValue, EchoSdkError>;

pub(crate) struct FacadeStreamRecord {
    owner_session: String,
    family: String,
    resource_id: String,
    close_resource_on_finish: bool,
    receiver: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<FacadeStreamItem>>,
    cancel: tokio_util::sync::CancellationToken,
    background: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

pub(crate) struct FacadeStreamProducer {
    pub handle: WireHandle,
    pub sender: tokio::sync::mpsc::Sender<FacadeStreamItem>,
    pub cancel: tokio_util::sync::CancellationToken,
    record: Arc<FacadeStreamRecord>,
}

pub(crate) struct FacadeStreamRuntime {
    streams: Mutex<HashMap<String, Arc<FacadeStreamRecord>>>,
    max_open_streams: usize,
    shutdown_timeout: std::time::Duration,
}

impl FacadeStreamRuntime {
    pub fn new(max_open_streams: usize, shutdown_timeout_secs: u64) -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
            max_open_streams,
            shutdown_timeout: std::time::Duration::from_secs(shutdown_timeout_secs),
        }
    }

    pub fn open(
        &self,
        handles: &HandleRegistry,
        resource: &WireHandle,
        owner_session: &str,
        operation: &str,
    ) -> Result<FacadeStreamProducer, EchoSdkError> {
        self.open_with_ownership(handles, resource, owner_session, operation, false)
    }

    /// Open a stream whose facade resource exists only to anchor this stream.
    /// Natural terminal or explicit close tombstones both handles together.
    #[cfg(feature = "sdk-extension-bridge")]
    pub fn open_ephemeral(
        &self,
        handles: &HandleRegistry,
        resource: &WireHandle,
        owner_session: &str,
        operation: &str,
    ) -> Result<FacadeStreamProducer, EchoSdkError> {
        self.open_with_ownership(handles, resource, owner_session, operation, true)
    }

    fn open_with_ownership(
        &self,
        handles: &HandleRegistry,
        resource: &WireHandle,
        owner_session: &str,
        operation: &str,
        close_resource_on_finish: bool,
    ) -> Result<FacadeStreamProducer, EchoSdkError> {
        let resource_record = handles.facade_resource(resource, operation)?;
        let handle = handles.register_facade_stream(
            self.max_open_streams,
            resource,
            Some(owner_session),
            operation,
        )?;
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let cancel = tokio_util::sync::CancellationToken::new();
        let record = Arc::new(FacadeStreamRecord {
            owner_session: owner_session.to_string(),
            family: resource_record.family.clone(),
            resource_id: resource.id.clone(),
            close_resource_on_finish,
            receiver: tokio::sync::Mutex::new(receiver),
            cancel: cancel.clone(),
            background: Mutex::new(None),
        });
        self.streams
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(handle.id.clone(), record.clone());
        Ok(FacadeStreamProducer {
            handle,
            sender,
            cancel,
            record,
        })
    }

    pub fn attach_background(
        &self,
        producer: &FacadeStreamProducer,
        background: tokio::task::JoinHandle<()>,
    ) {
        *producer
            .record
            .background
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(background);
    }

    pub async fn next(
        &self,
        handles: &HandleRegistry,
        stream: &WireHandle,
        owner_session: &str,
        family: &str,
        operation: &str,
    ) -> Result<WireValue, EchoSdkError> {
        let record = self.record(handles, stream, owner_session, family, operation)?;
        // Serialise all pulls for this stream. The registry sequence read,
        // dequeue and sequence/terminal transition stay inside this one
        // consumer critical section, so concurrent `next` calls cannot remove
        // two items using the same pre-dequeue watermark.
        let mut receiver = record.receiver.lock().await;
        let (last_sequence, _) =
            handles.facade_stream_state(stream, Some(owner_session), operation)?;
        let item = receiver.recv().await;
        let (_, cancelled) = handles.facade_stream_state(stream, Some(owner_session), operation)?;
        if cancelled {
            let sequence =
                handles.settle_facade_stream(stream, Some(owner_session), true, operation)?;
            let terminal = stream_event(stream, sequence, "cancelled", None);
            drop(receiver);
            let _ = self
                .close(handles, stream, owner_session, family, operation)
                .await;
            return Ok(terminal);
        }
        match item {
            Some(Ok(value)) => {
                let sequence = last_sequence.checked_add(1).ok_or_else(|| {
                    wire::sdk_error(
                        ExtensionErrorCode::FrameworkError,
                        "facade stream sequence exhausted",
                        Retryability::Never,
                        operation,
                    )
                })?;
                handles.advance_facade_stream(stream, Some(owner_session), sequence, operation)?;
                Ok(stream_event(stream, sequence, "item", Some(value)))
            }
            Some(Err(error)) => {
                let sequence =
                    handles.settle_facade_stream(stream, Some(owner_session), false, operation)?;
                let error = WireValue::from_json(serde_json::to_value(error).map_err(|error| {
                    wire::sdk_error(
                        ExtensionErrorCode::FrameworkError,
                        format!("facade stream error projection failed: {error}"),
                        Retryability::Never,
                        operation,
                    )
                })?)
                .map_err(|error| {
                    wire::sdk_error(
                        ExtensionErrorCode::FrameworkError,
                        format!("facade stream error projection failed: {error}"),
                        Retryability::Never,
                        operation,
                    )
                })?;
                let terminal = stream_event(stream, sequence, "failed", Some(error));
                drop(receiver);
                let _ = self
                    .close(handles, stream, owner_session, family, operation)
                    .await;
                Ok(terminal)
            }
            None => {
                let sequence =
                    handles.settle_facade_stream(stream, Some(owner_session), false, operation)?;
                let completed = stream_event(stream, sequence, "complete", None);
                drop(receiver);
                let _ = self
                    .close(handles, stream, owner_session, family, operation)
                    .await;
                Ok(completed)
            }
        }
    }

    pub fn cancel(
        &self,
        handles: &HandleRegistry,
        stream: &WireHandle,
        owner_session: &str,
        family: &str,
        operation: &str,
    ) -> Result<bool, EchoSdkError> {
        let _ = self.record(handles, stream, owner_session, family, operation)?;
        let changed = handles.cancel_facade_stream(stream, Some(owner_session), operation)?;
        if changed
            && let Some(record) = self
                .streams
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&stream.id)
                .cloned()
        {
            record.cancel.cancel();
        }
        Ok(changed)
    }

    pub async fn close(
        &self,
        handles: &HandleRegistry,
        stream: &WireHandle,
        owner_session: &str,
        family: &str,
        operation: &str,
    ) -> Result<bool, EchoSdkError> {
        if !handles.is_closed(stream) {
            let _ = self.record(handles, stream, owner_session, family, operation)?;
        }
        let closed = handles.close_facade_stream(stream, Some(owner_session), operation)?;
        let record = {
            self.streams
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&stream.id)
        };
        if let Some(record) = record {
            record.cancel.cancel();
            let background = {
                record
                    .background
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take()
            };
            if let Some(background) = background {
                settle_background(background, self.shutdown_timeout).await;
            }
            if record.close_resource_on_finish {
                let resource = WireHandle {
                    id: record.resource_id.clone(),
                    kind: echo_sdk_protocol::handle::HandleKind::FacadeResource,
                    generation: stream.generation.clone(),
                };
                let _ = handles.close_facade_resource(&resource, operation);
            }
        }
        Ok(closed)
    }

    pub async fn close_owner(&self, owner_session: &str) {
        let records = take_matching(&self.streams, |record| {
            record.owner_session == owner_session
        });
        cancel_records(records, self.shutdown_timeout).await;
    }

    pub async fn close_resource(&self, resource_id: &str) {
        let records = take_matching(&self.streams, |record| record.resource_id == resource_id);
        cancel_records(records, self.shutdown_timeout).await;
    }

    pub async fn close_all(&self) {
        let records = std::mem::take(
            &mut *self
                .streams
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        cancel_records(records.into_values().collect(), self.shutdown_timeout).await;
    }

    fn record(
        &self,
        handles: &HandleRegistry,
        stream: &WireHandle,
        owner_session: &str,
        family: &str,
        operation: &str,
    ) -> Result<Arc<FacadeStreamRecord>, EchoSdkError> {
        handles.facade_stream(stream, Some(owner_session), operation)?;
        let record = self
            .streams
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&stream.id)
            .cloned()
            .ok_or_else(|| {
                wire::sdk_error(
                    ExtensionErrorCode::ClosedHandle,
                    "facade stream receiver is no longer available",
                    Retryability::Never,
                    operation,
                )
            })?;
        if record.family != family {
            return Err(wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                format!(
                    "facade stream belongs to family {}, not {family}",
                    record.family
                ),
                Retryability::Never,
                operation,
            ));
        }
        Ok(record)
    }
}

pub(crate) async fn dispatch_control(
    runtime: &FacadeStreamRuntime,
    handles: &HandleRegistry,
    owner_session: &str,
    family: &str,
    request: &FeatureOperationRequest,
) -> Result<WireValue, EchoSdkError> {
    if request.arguments.len() != 1 {
        return Err(wire::sdk_error(
            ExtensionErrorCode::InvalidValue,
            "facade stream control accepts exactly one Stream handle argument",
            Retryability::Never,
            &request.operation,
        ));
    }
    let stream = match request.arguments.first() {
        Some(WireValue::Handle(handle)) => handle,
        _ => {
            return Err(wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                "facade stream control requires a Stream handle",
                Retryability::Never,
                &request.operation,
            ));
        }
    };
    if request.operation.ends_with(".stream.next") {
        runtime
            .next(handles, stream, owner_session, family, &request.operation)
            .await
    } else if request.operation.ends_with(".stream.cancel") {
        runtime
            .cancel(handles, stream, owner_session, family, &request.operation)
            .map(WireValue::Bool)
    } else if request.operation.ends_with(".stream.close") {
        runtime
            .close(handles, stream, owner_session, family, &request.operation)
            .await
            .map(WireValue::Bool)
    } else {
        Err(wire::sdk_error(
            ExtensionErrorCode::InvalidValue,
            "unknown facade stream control operation",
            Retryability::Never,
            &request.operation,
        ))
    }
}

fn stream_event(
    stream: &WireHandle,
    sequence: u64,
    variant: &str,
    value: Option<WireValue>,
) -> WireValue {
    let mut fields = vec![
        WireField {
            name: "stream".to_string(),
            value: WireValue::Handle(stream.clone()),
        },
        WireField {
            name: "sequence".to_string(),
            value: WireValue::U64(WireU64::from_u64(sequence)),
        },
    ];
    if let Some(value) = value {
        fields.push(WireField {
            name: "value".to_string(),
            value,
        });
    }
    WireValue::Variant {
        type_id: STREAM_EVENT_TYPE_ID.to_string(),
        variant: variant.to_string(),
        fields,
    }
}

fn take_matching(
    streams: &Mutex<HashMap<String, Arc<FacadeStreamRecord>>>,
    matches: impl Fn(&FacadeStreamRecord) -> bool,
) -> Vec<Arc<FacadeStreamRecord>> {
    let mut streams = streams.lock().unwrap_or_else(|error| error.into_inner());
    let ids = streams
        .iter()
        .filter(|(_, record)| matches(record))
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    ids.into_iter()
        .filter_map(|id| streams.remove(&id))
        .collect()
}

async fn cancel_records(records: Vec<Arc<FacadeStreamRecord>>, timeout: std::time::Duration) {
    for record in records {
        record.cancel.cancel();
        let background = {
            record
                .background
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
        };
        if let Some(background) = background {
            settle_background(background, timeout).await;
        }
    }
}

async fn settle_background(
    mut background: tokio::task::JoinHandle<()>,
    timeout: std::time::Duration,
) {
    if tokio::time::timeout(timeout, &mut background)
        .await
        .is_err()
    {
        background.abort();
        let _ = background.await;
    }
}
