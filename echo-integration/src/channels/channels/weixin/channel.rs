//! One transport owner, one bounded sender, and admission receipts independent of response completion.

use super::{
    api::WeixinApi,
    auth::WeixinCredentials,
    codec::{decode_target, encode_target},
    state::{ProtocolState, WeixinContextRef},
    wire::{WeixinMessage, WeixinPollStatus, WeixinSendOutcome},
};
use crate::channels::types::*;
use async_trait::async_trait;
use echo_core::{
    agent::CancellationToken,
    error::{ChannelError, ReactError, Result},
    memory::store::Store,
};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Semaphore, mpsc, oneshot},
    task::{JoinHandle, JoinSet},
};

const SEND_CAPACITY: usize = 32;
const INGRESS_CAPACITY: usize = 32;
const CONTROL_CAPACITY: usize = 8;
const MAX_PENDING_RECEIPTS: usize = 512;

#[derive(Clone)]
pub struct WeixinConfig {
    pub credentials: WeixinCredentials,
    /// Applications choose their existing FileStore/data root; no second inbox is created.
    pub store: Arc<dyn Store>,
}

impl fmt::Debug for WeixinConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeixinConfig")
            .field("credentials", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

struct SendRequest {
    message: OutboundMessage,
    context: Option<WeixinContextRef>,
    client_id: String,
    receipt: oneshot::Sender<Result<WeixinSendOutcome>>,
    // Hold the original session lease through the actual HTTP result.
    _permit: Option<ChannelDeliveryPermit>,
}

/// Concrete typed receipt capability referencing the channel's sole bounded sender.
#[derive(Clone)]
pub struct WeixinDeliveryHandle {
    tx: mpsc::Sender<SendRequest>,
    accepting: Arc<AtomicBool>,
    settled: Arc<AtomicBool>,
    cancel: CancellationToken,
    credentials: WeixinCredentials,
    state: ProtocolState,
}

impl fmt::Debug for WeixinDeliveryHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeixinDeliveryHandle")
            .field("accepting", &self.accepting.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl WeixinDeliveryHandle {
    /// Explicit local forget after the original plugin has stopped. Histories and
    /// notification obligations belong to their original owners and remain untouched.
    pub async fn clear_protocol_state(&self) -> Result<()> {
        if self.accepting.load(Ordering::Acquire)
            || !self.cancel.is_cancelled()
            || !self.settled.load(Ordering::Acquire)
        {
            return Err(error("Stop Weixin before forgetting protocol state"));
        }
        self.state.clear().await
    }
    /// Resolve once when freezing a proactive destination, never at later send time.
    pub async fn capture_context(
        &self,
        canonical_target: &str,
    ) -> Result<Option<WeixinContextRef>> {
        let target = decode_target(canonical_target)?;
        if target.account_id != self.credentials.account_id {
            return Err(error("Weixin target belongs to a different account"));
        }
        self.state.latest_context(&target.peer).await
    }

    pub async fn send(
        &self,
        message: OutboundMessage,
        context: &WeixinContextRef,
        client_id: &str,
    ) -> Result<WeixinSendOutcome> {
        self.send_with_context(message, Some(context), client_id)
            .await
    }

    /// Capture this original inbound message's association, including explicit None.
    /// This never falls back to a newer peer context.
    pub async fn capture_message_context(
        &self,
        canonical_target: &str,
        message_id: &str,
    ) -> Result<Option<WeixinContextRef>> {
        let target = decode_target(canonical_target)?;
        if target.account_id != self.credentials.account_id {
            return Err(error("Weixin message context belongs to another account"));
        }
        self.state.message_context(&target.peer, message_id).await
    }

    /// Validate the original message's exact non-secret recovery reference.
    /// `None` requires a persisted explicit absence; it is not a latest lookup
    /// or evidence that a missing association ever existed.
    pub async fn validate_message_context(
        &self,
        canonical_target: &str,
        provider_message_id: &str,
        expected: Option<&WeixinContextRef>,
    ) -> Result<()> {
        let target = decode_target(canonical_target)?;
        if target.account_id != self.credentials.account_id {
            return Err(error("Weixin message context belongs to another account"));
        }
        self.state
            .validate_message_context(&target.peer, provider_message_id, expected)
            .await
    }

    /// Send using the context frozen by the original owner. An explicit None
    /// remains None through network admission; no latest-context lookup occurs.
    pub async fn send_with_context(
        &self,
        message: OutboundMessage,
        context: Option<&WeixinContextRef>,
        client_id: &str,
    ) -> Result<WeixinSendOutcome> {
        self.enqueue(message, context.cloned(), client_id.to_owned())
            .await
    }

    async fn enqueue(
        &self,
        message: OutboundMessage,
        context: Option<WeixinContextRef>,
        client_id: String,
    ) -> Result<WeixinSendOutcome> {
        validate_outbound(&message, &self.credentials, context.as_ref())?;
        if client_id.trim().is_empty() {
            return Err(error("Weixin client identity is required"));
        }
        if !self.accepting.load(Ordering::Acquire) {
            return Err(error("Weixin channel is not accepting delivery"));
        }
        let permit = message.begin_delivery()?;
        let (receipt, received) = oneshot::channel();
        let receipt_identity = client_id.clone();
        let request = SendRequest {
            message,
            context,
            client_id,
            receipt,
            _permit: permit,
        };
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(error("Weixin channel stopped before queue admission")),
            result = self.tx.send(request) => result.map_err(|_| error("Weixin sender is closed"))?,
        }
        // Cancellation after enqueue cannot manufacture a failed/unsent receipt.
        match received.await {
            Ok(result) => result,
            Err(_) => Ok(WeixinSendOutcome::Unknown {
                client_id: receipt_identity,
                reason: "sender ended without its original delivery receipt".into(),
            }),
        }
    }
}

pub struct WeixinChannel {
    config: WeixinConfig,
    api: WeixinApi,
    state: ProtocolState,
    transport: TransportStatus,
    cancel: CancellationToken,
    accepting: Arc<AtomicBool>,
    settled: Arc<AtomicBool>,
    handle: Option<WeixinDeliveryHandle>,
    send_rx: Option<mpsc::Receiver<SendRequest>>,
    poll_task: Option<JoinHandle<Result<()>>>,
    send_task: Option<JoinHandle<()>>,
    handler: Option<Arc<dyn MessageHandler>>,
}

impl WeixinChannel {
    /// Forget restored protocol state when this process never registered a plugin.
    ///
    /// The caller must hold its original configuration/mutation authority and
    /// ensure no active plugin owns this exact account and credential generation.
    /// Registered instances must first settle through their original `stop` path.
    /// Histories and notification obligations are owned elsewhere and stay intact.
    pub async fn forget_protocol_state(config: WeixinConfig) -> Result<()> {
        config.credentials.validate()?;
        ProtocolState::new(config.store, &config.credentials)
            .clear()
            .await
    }

    pub fn new(config: WeixinConfig) -> Result<Self> {
        let credentials = &config.credentials;
        credentials.validate()?;
        if credentials.account_id.trim().is_empty()
            || credentials.bot_token.is_empty()
            || credentials.user_id.trim().is_empty()
            || credentials.generation.trim().is_empty()
        {
            return Err(error(
                "Weixin credentials require account, scanner identity, token and stable generation",
            ));
        }
        let state = ProtocolState::new(config.store.clone(), credentials);
        let cancel = CancellationToken::new();
        let accepting = Arc::new(AtomicBool::new(false));
        let settled = Arc::new(AtomicBool::new(true));
        let (tx, rx) = mpsc::channel(SEND_CAPACITY);
        let handle = WeixinDeliveryHandle {
            tx,
            accepting: accepting.clone(),
            settled: settled.clone(),
            cancel: cancel.clone(),
            credentials: credentials.clone(),
            state: state.clone(),
        };
        Ok(Self {
            config,
            api: WeixinApi::new()?,
            state,
            transport: TransportStatus::new(ChannelTransportPhase::Stopped),
            cancel,
            accepting,
            settled,
            handle: Some(handle),
            send_rx: Some(rx),
            poll_task: None,
            send_task: None,
            handler: None,
        })
    }

    /// Construct the original poll/sender channel against an explicit loopback fixture.
    ///
    /// Downstream tests must enable `echo_integration/channel-test-support`.
    /// The endpoint must be loopback; the fixture client disables proxies and
    /// redirects. Credentials still pass the production validation, so tests use
    /// synthetic credentials with a valid HTTPS origin. Only bot poll/send requests
    /// use the local endpoint; this does not emulate QR authentication.
    ///
    /// Production [`Self::new`] remains HTTPS-only even when this feature is enabled.
    #[cfg(any(test, feature = "channel-test-support"))]
    #[cfg_attr(docsrs, doc(cfg(feature = "channel-test-support")))]
    pub fn for_local_fixture(config: WeixinConfig, address: std::net::SocketAddr) -> Result<Self> {
        let mut channel = Self::new(config)?;
        channel.api = WeixinApi::for_local_poll_fixture(address)?;
        Ok(channel)
    }

    pub fn delivery_handle(&self) -> Option<WeixinDeliveryHandle> {
        self.handle.clone()
    }

    pub fn with_bot_agent(mut self, bot_agent: &str) -> Result<Self> {
        self.api = self.api.with_bot_agent(bot_agent)?;
        Ok(self)
    }
}

#[async_trait]
impl ChannelPlugin for WeixinChannel {
    fn id(&self) -> &str {
        "weixin"
    }
    fn label(&self) -> &str {
        "Weixin"
    }
    fn capabilities(&self) -> &ChannelCapabilities {
        static CAPS: ChannelCapabilities = ChannelCapabilities {
            chat_types: &[ChatType::Direct],
            supports_media: false,
            supports_threads: false,
        };
        &CAPS
    }

    async fn start(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()> {
        if self.poll_task.is_some() || self.send_task.is_some() || self.handler.is_some() {
            return Err(error("Weixin channel already owns active tasks"));
        }
        // Fail before spawning if restoration is unreadable; do not reset a durable cursor.
        let cursor = self.state.cursor().await?;
        if self.handle.is_none() {
            self.cancel = CancellationToken::new();
            self.accepting = Arc::new(AtomicBool::new(false));
            self.settled = Arc::new(AtomicBool::new(true));
            let (tx, rx) = mpsc::channel(SEND_CAPACITY);
            self.handle = Some(WeixinDeliveryHandle {
                tx,
                accepting: self.accepting.clone(),
                settled: self.settled.clone(),
                cancel: self.cancel.clone(),
                credentials: self.config.credentials.clone(),
                state: self.state.clone(),
            });
            self.send_rx = Some(rx);
        }
        // Retain even a failed bind's original close obligation. No legacy
        // invocation, sender spawn or poll may run as a capability probe.
        self.handler = Some(handler.clone());
        handler.bind_durable_ingress(self.cancel.child_token())?;
        let rx = self
            .send_rx
            .take()
            .ok_or_else(|| error("Weixin original sender receiver is unavailable"))?;
        let delivery = self
            .handle
            .clone()
            .ok_or_else(|| error("Weixin original delivery handle is unavailable"))?;
        self.accepting.store(true, Ordering::Release);
        self.settled.store(false, Ordering::Release);
        self.transport = TransportStatus::new(ChannelTransportPhase::Connecting);
        self.send_task = Some(tokio::spawn(send_loop(
            rx,
            self.api.clone(),
            self.config.credentials.clone(),
            self.state.clone(),
            self.cancel.clone(),
            self.accepting.clone(),
            self.transport.clone(),
        )));
        self.poll_task = Some(tokio::spawn(poll_loop(
            self.api.clone(),
            self.config.credentials.clone(),
            self.state.clone(),
            cursor,
            handler.clone(),
            delivery.clone(),
            self.cancel.clone(),
            self.transport.clone(),
        )));
        self.handle = Some(delivery);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.accepting.store(false, Ordering::Release);
        self.cancel.cancel();
        self.handle = None;
        self.send_rx = None;
        // Await through borrowed handles: dropping an interrupted stop future keeps ownership.
        if let Some(task) = self.poll_task.as_mut() {
            let result = task.await;
            self.poll_task = None;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(_)) | Err(_) => {
                    self.transport.publish(
                        ChannelTransportPhase::Unknown,
                        None,
                        Some("Weixin receive tasks did not settle successfully".into()),
                    );
                    return Err(error("Weixin receive shutdown requires observation"));
                }
            }
        }
        if let Some(task) = self.send_task.as_mut() {
            let result = task.await;
            self.send_task = None;
            if result.is_err() {
                return Err(error("Weixin sender shutdown requires observation"));
            }
        }
        if let Some(handler) = self.handler.as_ref() {
            handler.close().await?;
        }
        self.handler = None;
        self.settled.store(true, Ordering::Release);
        self.transport
            .publish(ChannelTransportPhase::Stopped, None, None);
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> Result<()> {
        if message.text.is_empty() && message.attachments.is_empty() {
            return Ok(());
        }
        let handle = self
            .handle
            .as_ref()
            .ok_or_else(|| error("Weixin channel is stopped"))?;
        // Ordinary explicit sends capture now; frozen cron sends use the concrete handle instead.
        let context = handle.capture_context(&message.to).await?;
        outcome_result(
            handle
                .enqueue(
                    message,
                    context,
                    format!("eko-weixin-{}", uuid::Uuid::new_v4()),
                )
                .await?,
        )
    }

    fn transport_status(&self) -> Option<tokio::sync::watch::Receiver<ChannelTransportSnapshot>> {
        Some(self.transport.subscribe())
    }
    async fn health_check(&self) -> Result<()> {
        if self.transport.subscribe().borrow().phase == ChannelTransportPhase::Ready {
            Ok(())
        } else {
            Err(error("Weixin authenticated poll is not ready"))
        }
    }
}

async fn send_loop(
    mut rx: mpsc::Receiver<SendRequest>,
    api: WeixinApi,
    credentials: WeixinCredentials,
    state: ProtocolState,
    cancel: CancellationToken,
    accepting: Arc<AtomicBool>,
    transport: TransportStatus,
) {
    let mut observation = transport.task_guard();
    loop {
        let request = tokio::select! {
            biased;
            _ = cancel.cancelled() => { rx.close(); None },
            request = rx.recv() => request,
        };
        let Some(request) = request else { break };
        let result = if !accepting.load(Ordering::Acquire) {
            Err(error("Weixin delivery stopped before network admission"))
        } else {
            send_request(&api, &credentials, &state, &request).await
        };
        let _ = request.receipt.send(result);
    }
    // Pending requests have not reached HTTP. Every caller receives its original receipt.
    while let Some(request) = rx.recv().await {
        let _ = request.receipt.send(Err(error(
            "Weixin delivery stopped before network admission",
        )));
    }
    observation.finish();
}

async fn send_request(
    api: &WeixinApi,
    credentials: &WeixinCredentials,
    state: &ProtocolState,
    request: &SendRequest,
) -> Result<WeixinSendOutcome> {
    validate_outbound(&request.message, credentials, request.context.as_ref())?;
    let target = decode_target(&request.message.to)?;
    let token = match request.context.as_ref() {
        Some(context) => Some(state.context_token(context).await?),
        None => None,
    };
    // Do not select on cancellation around an effectful request: its real outcome owns settlement.
    api.send_text(
        credentials,
        &target.peer,
        token.as_deref(),
        &request.client_id,
        &request.message.text,
    )
    .await
}

fn validate_outbound(
    message: &OutboundMessage,
    credentials: &WeixinCredentials,
    context: Option<&WeixinContextRef>,
) -> Result<()> {
    let target = decode_target(&message.to)?;
    if message.channel_id != "weixin"
        || message.chat_type != ChatType::Direct
        || !message.attachments.is_empty()
        || target.account_id != credentials.account_id
    {
        return Err(error(
            "Weixin supports text only for its exact account/direct target",
        ));
    }
    if let Some(context) = context
        && (context.account_id != credentials.account_id
            || context.generation != credentials.generation
            || context.peer != target.peer)
    {
        return Err(error(
            "Weixin context does not match the frozen account/generation/peer",
        ));
    }
    Ok(())
}

fn outcome_result(outcome: WeixinSendOutcome) -> Result<()> {
    match outcome {
        WeixinSendOutcome::Accepted { .. } => Ok(()),
        WeixinSendOutcome::Rejected { code } => Err(error(&format!(
            "Weixin send rejected with business code {code}"
        ))),
        WeixinSendOutcome::Unknown { client_id, .. } => Err(error(&format!(
            "Weixin send outcome unknown; do not retry automatically ({client_id})"
        ))),
    }
}

struct PendingIngress {
    receipt: oneshot::Receiver<ChannelIngressDisposition>,
}

type PendingReceipts = HashMap<String, PendingIngress>;

#[allow(clippy::too_many_arguments)]
async fn poll_loop(
    api: WeixinApi,
    credentials: WeixinCredentials,
    state: ProtocolState,
    mut cursor: String,
    handler: Arc<dyn MessageHandler>,
    delivery: WeixinDeliveryHandle,
    cancel: CancellationToken,
    transport: TransportStatus,
) -> Result<()> {
    let mut observation = transport.task_guard();
    let normal_slots = Arc::new(Semaphore::new(INGRESS_CAPACITY));
    let control_slots = Arc::new(Semaphore::new(CONTROL_CAPACITY));
    // Only real admission calls live here. Their original permits are held
    // until return, even after ACK; the durable host owns queued DATA and its
    // bounded claimed long responses, rather than a transport future per input.
    let mut tasks = JoinSet::new();
    let mut pending: PendingReceipts = HashMap::new();
    let mut acknowledged = HashSet::new();
    // Ephemeral receipt frontier only; durable execution/dedup remains in the handler owner.
    // Keep unknown IDs even if the next server response omits them.
    let mut frontier = HashSet::new();
    let mut failure = None;
    loop {
        reap_receipts(&mut pending, &mut acknowledged);
        while let Some(result) = tasks.try_join_next() {
            if result.is_err() {
                failure = Some(error("Weixin admission task ended abnormally"));
                break;
            }
        }
        if failure.is_some() || cancel.is_cancelled() {
            break;
        }
        let updates = tokio::select! {
            _ = cancel.cancelled() => break,
            result = api.poll(&credentials, &cursor) => result,
        };
        let updates = match updates {
            Ok(updates) => updates,
            Err(_) => {
                transport.publish(
                    ChannelTransportPhase::Disconnected,
                    None,
                    Some("Weixin poll failed; cursor unchanged".into()),
                );
                tokio::select! { _ = cancel.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
                continue;
            }
        };
        match updates.status {
            WeixinPollStatus::Accepted => transport.publish(
                ChannelTransportPhase::Ready,
                Some(ChannelTransportEvidence::WeixinAuthenticatedPoll),
                None,
            ),
            WeixinPollStatus::SessionExpired => {
                delivery.accepting.store(false, Ordering::Release);
                transport.publish(
                    ChannelTransportPhase::Failed,
                    None,
                    Some(
                        "Weixin authorization expired (-14); reconnect requires authentication"
                            .into(),
                    ),
                );
                break;
            }
            WeixinPollStatus::Rejected { code } => {
                transport.publish(
                    ChannelTransportPhase::Disconnected,
                    None,
                    Some(format!("Weixin poll rejected ({code}); cursor unchanged")),
                );
                tokio::select! { _ = cancel.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
                continue;
            }
            WeixinPollStatus::Unknown => {
                transport.publish(
                    ChannelTransportPhase::Unknown,
                    None,
                    Some("Weixin poll business acceptance is unknown".into()),
                );
                tokio::select! { _ = cancel.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
                continue;
            }
        }
        if updates.messages.len() > MAX_PENDING_RECEIPTS {
            transport.publish(
                ChannelTransportPhase::Failed,
                None,
                Some("Weixin poll batch exceeds bounded admission; cursor unchanged".into()),
            );
            break;
        }
        let mut batch_ids = Vec::new();
        let mut batch_unknown = false;
        for message in updates.messages {
            if message.message_id.is_empty() {
                batch_unknown = true;
                continue;
            }
            let identity = match encode_target(&credentials.account_id, &message.peer) {
                Ok(target) => format!("{target}:{}", message.message_id),
                Err(_) => {
                    failure = Some(error("Weixin poll contains an invalid peer identity"));
                    batch_unknown = true;
                    break;
                }
            };
            batch_ids.push(identity.clone());
            if !frontier.contains(&identity) && frontier.len() >= MAX_PENDING_RECEIPTS {
                batch_unknown = true;
                continue;
            }
            frontier.insert(identity.clone());
            if acknowledged.contains(&identity) || pending.contains_key(&identity) {
                continue;
            }
            // Provider capability validation is separate from the embedding
            // application's peer policy. EKO applies its scanner policy at
            // its original handler boundary before any session/Agent effect.
            if message.group_id.is_some() {
                transport.publish(ChannelTransportPhase::Ready, Some(ChannelTransportEvidence::WeixinAuthenticatedPoll), Some("Weixin message rejected without Agent execution: only direct text is supported".into()));
                acknowledged.insert(identity);
                continue;
            }
            let slots = if is_control(&message.text) {
                control_slots.clone()
            } else {
                normal_slots.clone()
            };
            reap_receipts(&mut pending, &mut acknowledged);
            // Reap completed task metadata before every spawn, not only once
            // per server page. An ACK is independent of physical task exit.
            while let Some(result) = tasks.try_join_next() {
                if result.is_err() {
                    failure = Some(error("Weixin admission task ended abnormally"));
                }
            }
            if failure.is_some() {
                batch_unknown = true;
                break;
            }
            let Ok(permit) = slots.try_acquire_owned() else {
                batch_unknown = true;
                continue;
            };
            let context = match state
                .remember_message_context(
                    &message.peer,
                    &message.message_id,
                    message.context_token.as_deref(),
                )
                .await
            {
                Ok(context) => context,
                Err(_) => {
                    batch_unknown = true;
                    continue;
                }
            };
            let (publisher, received) = channel_ingress_receipt();
            pending.insert(identity, PendingIngress { receipt: received });
            let request_handler = handler.clone();
            let request_delivery = delivery.clone();
            let request_cancel = cancel.child_token();
            let account_id = credentials.account_id.clone();
            tasks.spawn(async move {
                let _admission_permit = permit;
                drive_request(
                    request_handler,
                    request_delivery,
                    request_cancel,
                    account_id,
                    message,
                    context,
                    publisher,
                )
                .await;
            });
        }
        // Waiting for original durable facts never waits for a model/response.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            reap_receipts(&mut pending, &mut acknowledged);
            if batch_ids.iter().all(|id| acknowledged.contains(id))
                || tokio::time::Instant::now() >= deadline
                || cancel.is_cancelled()
            {
                break;
            }
            tokio::select! { _ = cancel.cancelled() => break, _ = tokio::time::sleep(Duration::from_millis(10)) => {} }
        }
        if !batch_unknown && frontier.iter().all(|id| acknowledged.contains(id)) {
            if let Some(next_cursor) = updates.cursor {
                if next_cursor == cursor {
                    // A repeated server cursor is not progress. Retain the receipt
                    // frontier rather than creating duplicate response producers.
                    continue;
                }
                match state.save_cursor(&next_cursor).await {
                    Ok(()) => { cursor = next_cursor; acknowledged.clear(); frontier.clear(); },
                    Err(_) => transport.publish(ChannelTransportPhase::Disconnected, None, Some("Weixin cursor persistence failed; replay remains fenced by original ingress owners".into())),
                }
            } else {
                transport.publish(
                    ChannelTransportPhase::Ready,
                    Some(ChannelTransportEvidence::WeixinAuthenticatedPoll),
                    Some(
                        "Weixin poll omitted its next cursor; original receipt frontier retained"
                            .into(),
                    ),
                );
            }
        }
    }
    // Stop the same service child before joining every real admission call.
    cancel.cancel();
    while let Some(result) = tasks.join_next().await {
        if result.is_err() {
            failure = Some(error("Weixin admission task failed during shutdown"));
        }
    }
    observation.finish();
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn reap_receipts(pending: &mut PendingReceipts, acknowledged: &mut HashSet<String>) {
    pending.retain(|id, ingress| match ingress.receipt.try_recv() {
        Ok(
            ChannelIngressDisposition::DurableInput
            | ChannelIngressDisposition::ControlHandled
            | ChannelIngressDisposition::TerminalRejected
            | ChannelIngressDisposition::ControlRecoveryRequired,
        ) => {
            acknowledged.insert(id.clone());
            false
        }
        Err(tokio::sync::oneshot::error::TryRecvError::Empty) => true,
        Err(tokio::sync::oneshot::error::TryRecvError::Closed) => false,
    });
}

async fn drive_request(
    handler: Arc<dyn MessageHandler>,
    delivery: WeixinDeliveryHandle,
    cancel: CancellationToken,
    account_id: String,
    message: WeixinMessage,
    context: Option<WeixinContextRef>,
    receipt: ChannelIngressReceipt,
) {
    let Ok(target) = encode_target(&account_id, &message.peer) else {
        return;
    };
    let Ok(sender) = encode_target(&account_id, &message.peer) else {
        return;
    };
    let mut inbound = InboundMessage::new(
        "weixin",
        sender,
        &target,
        ChatType::Direct,
        message.text,
        message.message_id,
    );
    if message.unsupported_media {
        // This is a terminal capability rejection, never partial execution of
        // the remaining text. Inform the phone through the same original
        // sender/context instead of silently discarding the unsupported input.
        receipt.publish(ChannelIngressDisposition::TerminalRejected);
        let outcome = delivery
            .enqueue(
                OutboundMessage::new(
                    "weixin",
                    &target,
                    ChatType::Direct,
                    "当前微信渠道仅支持单聊文字，暂不处理图片、文件、语音或视频；这条消息没有交给智能体执行。",
                ),
                context,
                format!("eko-weixin-unsupported-{}", uuid::Uuid::new_v4()),
            )
            .await;
        if !matches!(outcome, Ok(WeixinSendOutcome::Accepted { .. })) {
            tracing::warn!("Weixin capability rejection could not be confirmed as delivered");
        }
        return;
    }
    if let Some(timestamp) = message
        .timestamp_ms
        .and_then(|value| u64::try_from(value).ok())
    {
        inbound.timestamp = timestamp / 1000;
    }
    // The host publishes the original receipt, registers its reply route and
    // wakes its own claim owner. It retains long output through the original
    // delivery handle; transport never consumes or spawns an output waiter.
    let _ = handler.admit_with_ingress(inbound, cancel, receipt).await;
}

fn is_control(text: &str) -> bool {
    text.trim_start().starts_with('/')
}

fn error(message: &str) -> ReactError {
    ReactError::Channel(Box::new(ChannelError::Other(message.into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_ingress(receipt: oneshot::Receiver<ChannelIngressDisposition>) -> PendingIngress {
        PendingIngress { receipt }
    }

    fn credentials() -> WeixinCredentials {
        WeixinCredentials {
            account_id: "account-a".into(),
            bot_token: "secret".into(),
            base_url: "https://example.invalid/".into(),
            user_id: "peer-a".into(),
            generation: "generation-a".into(),
        }
    }

    #[test]
    fn context_cannot_cross_account_peer_or_credential_generation() -> Result<()> {
        let credentials = credentials();
        let message = OutboundMessage::new(
            "weixin",
            encode_target("account-a", "peer-a")?,
            ChatType::Direct,
            "中文😀",
        );
        let mut context = WeixinContextRef {
            account_id: "account-a".into(),
            generation: "generation-a".into(),
            peer: "peer-a".into(),
            reference: "context:test".into(),
        };
        validate_outbound(&message, &credentials, Some(&context))?;
        context.generation = "generation-b".into();
        assert!(validate_outbound(&message, &credentials, Some(&context)).is_err());
        context.generation = "generation-a".into();
        context.peer = "peer-b".into();
        assert!(validate_outbound(&message, &credentials, Some(&context)).is_err());
        assert!(
            outcome_result(WeixinSendOutcome::Unknown {
                client_id: "id".into(),
                reason: "timeout".into()
            })
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn dropped_ingress_does_not_release_the_cursor() -> Result<()> {
        let mut pending = HashMap::new();
        let mut acknowledged = HashSet::new();
        let (publisher, received) = channel_ingress_receipt();
        pending.insert("id".into(), pending_ingress(received));
        drop(publisher);
        reap_receipts(&mut pending, &mut acknowledged);
        assert!(acknowledged.is_empty());
        Ok(())
    }

    #[test]
    fn recovery_receipt_releases_only_its_exact_message() -> Result<()> {
        let mut pending = HashMap::new();
        let mut acknowledged = HashSet::new();
        let (publisher, received) = channel_ingress_receipt();
        pending.insert("id".into(), pending_ingress(received));
        publisher.publish(ChannelIngressDisposition::ControlRecoveryRequired);
        reap_receipts(&mut pending, &mut acknowledged);
        assert!(acknowledged.contains("id"));
        assert!(!acknowledged.contains("other"));
        Ok(())
    }

    #[tokio::test]
    async fn bounded_queue_is_not_a_success_receipt_and_receipt_loss_is_unknown() -> Result<()> {
        let credentials = credentials();
        let state = ProtocolState::new(super::super::state::tests::store(), &credentials);
        let (tx, mut rx) = mpsc::channel(1);
        let handle = WeixinDeliveryHandle {
            tx,
            accepting: Arc::new(AtomicBool::new(true)),
            settled: Arc::new(AtomicBool::new(false)),
            cancel: CancellationToken::new(),
            credentials,
            state,
        };
        let message = OutboundMessage::new(
            "weixin",
            encode_target("account-a", "peer-a")?,
            ChatType::Direct,
            "你好😀",
        );
        let delivered =
            tokio::spawn(async move { handle.enqueue(message, None, "stable-id".into()).await });
        let request = rx
            .recv()
            .await
            .ok_or_else(|| error("missing original queued request"))?;
        assert!(!delivered.is_finished());
        assert_eq!(request.message.text, "你好😀");
        drop(request.receipt);
        let outcome = delivered
            .await
            .map_err(|_| error("delivery fixture task failed"))??;
        assert!(
            matches!(outcome, WeixinSendOutcome::Unknown { client_id, .. } if client_id == "stable-id")
        );
        Ok(())
    }

    struct AdmissionBeforeResponse;

    #[async_trait]
    impl MessageHandler for AdmissionBeforeResponse {
        async fn handle(&self, _message: InboundMessage) -> Result<OutboundMessage> {
            Err(error("fixture requires ingress entry"))
        }
        async fn reply(&self, _message: OutboundMessage) -> Result<()> {
            Ok(())
        }
        async fn admit_with_ingress(
            &self,
            message: InboundMessage,
            cancel: CancellationToken,
            receipt: ChannelIngressReceipt,
        ) -> Result<()> {
            let control = is_control(&message.text);
            // This fixture intentionally holds admission after its early ACK,
            // to prove the transport cannot release a still-running call.
            receipt.publish(if control {
                ChannelIngressDisposition::ControlHandled
            } else {
                ChannelIngressDisposition::DurableInput
            });
            if !control {
                cancel.cancelled().await;
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn early_ack_does_not_finish_the_original_admission_call() -> Result<()> {
        let credentials = credentials();
        let state = ProtocolState::new(super::super::state::tests::store(), &credentials);
        let (tx, _rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let delivery = WeixinDeliveryHandle {
            tx,
            accepting: Arc::new(AtomicBool::new(true)),
            settled: Arc::new(AtomicBool::new(false)),
            cancel: cancel.clone(),
            credentials,
            state,
        };
        let message = |text: &str, id: &str| WeixinMessage {
            message_id: id.into(),
            peer: "peer-a".into(),
            context_token: None,
            text: text.into(),
            unsupported_media: false,
            group_id: None,
            timestamp_ms: None,
        };
        let (receipt, received) = channel_ingress_receipt();
        let normal = tokio::spawn(drive_request(
            Arc::new(AdmissionBeforeResponse),
            delivery.clone(),
            cancel.child_token(),
            "account-a".into(),
            message("needs approval", "message-1"),
            None,
            receipt,
        ));
        let disposition = tokio::time::timeout(Duration::from_secs(1), received)
            .await
            .map_err(|_| error("ingress did not release before response"))?
            .map_err(|_| error("ingress receipt dropped"))?;
        assert_eq!(disposition, ChannelIngressDisposition::DurableInput);
        assert!(!normal.is_finished());
        let (receipt, received) = channel_ingress_receipt();
        drive_request(
            Arc::new(AdmissionBeforeResponse),
            delivery,
            cancel.child_token(),
            "account-a".into(),
            message("/reply same-request yes", "message-2"),
            None,
            receipt,
        )
        .await;
        assert_eq!(
            received
                .await
                .map_err(|_| error("control receipt dropped"))?,
            ChannelIngressDisposition::ControlHandled
        );
        assert!(!normal.is_finished());
        cancel.cancel();
        normal
            .await
            .map_err(|_| error("original response producer failed to settle"))?;
        Ok(())
    }

    #[derive(Clone, Copy)]
    enum PollCase {
        FortyThenControl,
        FortyThenControlWithoutRet,
        ManyPagesThenControl,
        Unacknowledged,
        Unknown,
        Unrelated,
        NullRet,
        NullErrcode,
        Rejected,
        RejectedErrcode,
        RejectedWithNullRet,
        RejectedWithNullErrcode,
    }

    #[derive(Clone, Default)]
    struct PollObservation {
        requests: usize,
        saw_first_cursor: bool,
        saw_second_cursor: bool,
        unexpected_cursor: bool,
        send_requests: usize,
    }

    #[derive(Clone)]
    struct PollFixtureState {
        case: PollCase,
        observation: tokio::sync::watch::Sender<PollObservation>,
    }

    struct PollFixture {
        address: std::net::SocketAddr,
        observation: tokio::sync::watch::Receiver<PollObservation>,
        stop: CancellationToken,
        server: JoinHandle<Result<()>>,
    }

    impl PollFixture {
        async fn start(case: PollCase) -> Result<Self> {
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .map_err(|_| error("poll fixture listener failed"))?;
            let address = listener
                .local_addr()
                .map_err(|_| error("poll fixture address unavailable"))?;
            let (observation, received) = tokio::sync::watch::channel(PollObservation::default());
            let state = PollFixtureState { case, observation };
            let router = axum::Router::new()
                .route(
                    "/ilink/bot/getupdates",
                    axum::routing::post(poll_fixture_response),
                )
                .route(
                    "/ilink/bot/sendmessage",
                    axum::routing::post(poll_fixture_send),
                )
                .with_state(state);
            let stop = CancellationToken::new();
            let shutdown = stop.clone();
            let server = tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async move { shutdown.cancelled().await })
                    .await
                    .map_err(|_| error("poll fixture HTTP server failed"))
            });
            Ok(Self {
                address,
                observation: received,
                stop,
                server,
            })
        }

        async fn shutdown(self) -> Result<()> {
            self.stop.cancel();
            tokio::time::timeout(Duration::from_secs(5), self.server)
                .await
                .map_err(|_| error("poll fixture server did not settle"))?
                .map_err(|_| error("poll fixture server join failed"))?
        }
    }

    fn poll_message(id: &str, text: &str) -> serde_json::Value {
        serde_json::json!({"message_id": id, "message_type": 1, "from_user_id": "peer-a", "item_list": [{"type": 1, "text_item": {"text": text}}]})
    }

    async fn poll_fixture_response(
        axum::extract::State(state): axum::extract::State<PollFixtureState>,
        headers: axum::http::HeaderMap,
        axum::Json(request): axum::Json<serde_json::Value>,
    ) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
        let cursor = request
            .get("get_updates_buf")
            .and_then(serde_json::Value::as_str);
        if headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            != Some("Bearer secret")
            || cursor.is_none()
        {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"ret": 422})),
            );
        }
        let cursor = cursor.unwrap_or_default();
        let valid_cursor = match state.case {
            PollCase::FortyThenControl | PollCase::FortyThenControlWithoutRet => {
                matches!(cursor, "" | "first-page-accepted" | "control-page-accepted")
            }
            PollCase::ManyPagesThenControl => {
                cursor.is_empty()
                    || cursor == "control-page-accepted"
                    || cursor
                        .strip_prefix("page-")
                        .and_then(|value| value.parse::<usize>().ok())
                        .is_some_and(|index| index <= 128)
            }
            _ => cursor.is_empty(),
        };
        state.observation.send_modify(|observed| {
            observed.requests = observed.requests.saturating_add(1);
            observed.saw_first_cursor |= cursor == "first-page-accepted" || cursor == "page-128";
            observed.saw_second_cursor |= cursor == "control-page-accepted";
            observed.unexpected_cursor |= !valid_cursor;
        });
        // Keep empty-page heartbeats bounded while retaining a real TCP/HTTP request.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let mut response = match state.case {
            PollCase::FortyThenControl | PollCase::FortyThenControlWithoutRet
                if cursor.is_empty() =>
            {
                serde_json::json!({"ret": 0, "msgs": (0..40).map(|index| poll_message(&format!("normal-{index}"), "wait for the original HITL reply")).collect::<Vec<_>>(), "get_updates_buf": "first-page-accepted"})
            }
            PollCase::FortyThenControl | PollCase::FortyThenControlWithoutRet
                if cursor == "first-page-accepted" =>
            {
                serde_json::json!({"ret": 0, "msgs": [poll_message("original-control", "/reply original-request yes")], "get_updates_buf": "control-page-accepted"})
            }
            PollCase::FortyThenControl | PollCase::FortyThenControlWithoutRet
                if cursor == "control-page-accepted" =>
            {
                serde_json::json!({"ret": 0, "msgs": [], "get_updates_buf": "control-page-accepted"})
            }
            PollCase::FortyThenControl | PollCase::FortyThenControlWithoutRet => {
                serde_json::json!({"ret": 422})
            }
            PollCase::ManyPagesThenControl => {
                let page = if cursor.is_empty() {
                    Some(0)
                } else {
                    cursor
                        .strip_prefix("page-")
                        .and_then(|value| value.parse::<usize>().ok())
                };
                match page {
                    Some(index) if index < 128 => {
                        serde_json::json!({"ret": 0, "msgs": [poll_message(&format!("normal-{index}"), "wait for the original HITL reply")], "get_updates_buf": format!("page-{}", index.saturating_add(1))})
                    }
                    Some(128) => {
                        serde_json::json!({"ret": 0, "msgs": [poll_message("original-control", "/reply original-request yes")], "get_updates_buf": "control-page-accepted"})
                    }
                    _ => {
                        serde_json::json!({"ret": 0, "msgs": [], "get_updates_buf": "control-page-accepted"})
                    }
                }
            }
            PollCase::Unacknowledged => {
                serde_json::json!({"ret": 0, "msgs": [poll_message("no-ack", "must retain the original cursor")], "get_updates_buf": "must-not-commit"})
            }
            // 缺ret但含合法msgs/cursor已移入正控；Unknown仍表示业务接受证据不足。
            PollCase::Unknown => serde_json::json!({}),
            PollCase::Unrelated => serde_json::json!({"diagnostic_only": true}),
            PollCase::NullRet => {
                serde_json::json!({"ret": null, "msgs": [poll_message("null-ret", "must not enter the handler")], "get_updates_buf": "must-not-commit"})
            }
            PollCase::NullErrcode => {
                serde_json::json!({"errcode": null, "msgs": [poll_message("null-errcode", "must not enter the handler")], "get_updates_buf": "must-not-commit"})
            }
            PollCase::Rejected => {
                serde_json::json!({"ret": 17, "msgs": [poll_message("rejected-business", "must not enter the handler")], "get_updates_buf": "must-not-commit"})
            }
            PollCase::RejectedErrcode => {
                serde_json::json!({"ret": 0, "errcode": 17, "msgs": [poll_message("rejected-errcode", "must not enter the handler")], "get_updates_buf": "must-not-commit"})
            }
            PollCase::RejectedWithNullRet => {
                serde_json::json!({"ret": null, "errcode": 17, "msgs": [poll_message("nonzero-before-null-ret", "must not enter the handler")], "get_updates_buf": "must-not-commit"})
            }
            PollCase::RejectedWithNullErrcode => {
                serde_json::json!({"ret": 17, "errcode": null, "msgs": [poll_message("nonzero-before-null-errcode", "must not enter the handler")], "get_updates_buf": "must-not-commit"})
            }
        };
        if matches!(state.case, PollCase::FortyThenControlWithoutRet)
            && let Some(object) = response.as_object_mut()
        {
            object.remove("ret");
        }
        (axum::http::StatusCode::OK, axum::Json(response))
    }

    async fn poll_fixture_send(
        axum::extract::State(state): axum::extract::State<PollFixtureState>,
        axum::Json(_request): axum::Json<serde_json::Value>,
    ) -> axum::Json<serde_json::Value> {
        state.observation.send_modify(|observed| {
            observed.send_requests = observed.send_requests.saturating_add(1);
        });
        axum::Json(serde_json::json!({"ret": 0}))
    }

    async fn wait_observation<T: Clone, F: Fn(&T) -> bool>(
        receiver: &mut tokio::sync::watch::Receiver<T>,
        condition: F,
    ) -> Result<T> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let observed = receiver.borrow().clone();
                if condition(&observed) {
                    return Ok(observed);
                }
                receiver
                    .changed()
                    .await
                    .map_err(|_| error("fixture observation closed before its expected fact"))?;
            }
        })
        .await
        .map_err(|_| error("fixture expected fact timed out"))?
    }

    #[derive(Clone, Default)]
    struct HandlerObservation {
        normal_admitted: usize,
        controls: usize,
        hitl_resolved: usize,
        cancellation_seen: usize,
        physically_settled: usize,
        closed: usize,
        active_admissions: usize,
        peak_admissions: usize,
        chunks_sent: usize,
    }

    /// A generic file-backed durable host fixture, independent of EKO. Queued
    /// inputs are only fsynced DATA; a single original supervisor claims one
    /// long response. No admission creates a response future or another queue.
    struct FileBackedHost {
        data: Arc<tempfile::TempDir>,
        instance: crate::channels::session::ChannelSessionInstance,
        delivery: WeixinDeliveryHandle,
        observation: tokio::sync::watch::Sender<HandlerObservation>,
        original_hitl: CancellationToken,
        allow_settlement: CancellationToken,
        hold_admissions: Option<CancellationToken>,
        service: std::sync::Mutex<Option<CancellationToken>>,
        driver: tokio::sync::Mutex<Option<JoinHandle<Result<()>>>>,
        kick: Arc<tokio::sync::Notify>,
    }

    impl FileBackedHost {
        fn persist(&self, message: &InboundMessage, control: bool) -> Result<bool> {
            use sha2::{Digest, Sha256};
            use std::io::Write;
            let digest = format!("{:x}", Sha256::digest(message.message_id.as_bytes()));
            let name = format!("{}-{digest}", if control { "control" } else { "input" });
            let path = self.data.path().join(name);
            let bytes = serde_json::to_vec(&(&message.message_id, &message.text))
                .map_err(|_| error("durable host fixture encoding failed"))?;
            let mut file = match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => file,
                Err(failure) if failure.kind() == std::io::ErrorKind::AlreadyExists => {
                    let old = std::fs::read(path)
                        .map_err(|_| error("original durable DATA unreadable"))?;
                    if old != bytes {
                        return Err(error("original durable DATA identity collision"));
                    }
                    return Ok(false);
                }
                Err(_) => return Err(error("durable host fixture admission failed")),
            };
            file.write_all(&bytes)
                .map_err(|_| error("durable DATA write failed"))?;
            file.sync_all()
                .map_err(|_| error("durable DATA sync failed"))?;
            std::fs::File::open(self.data.path())
                .and_then(|directory| directory.sync_all())
                .map_err(|_| error("durable DATA directory sync failed"))?;
            Ok(true)
        }
    }

    #[async_trait]
    impl MessageHandler for FileBackedHost {
        async fn handle(&self, _message: InboundMessage) -> Result<OutboundMessage> {
            Err(error("durable host must not run the legacy entry"))
        }
        async fn reply(&self, message: OutboundMessage) -> Result<()> {
            outcome_result(
                self.delivery
                    .send_with_context(message, None, "fixture-reset-reply")
                    .await?,
            )
        }
        fn has_pending_work(&self) -> bool {
            self.observation.borrow().normal_admitted > 0
        }
        fn bind_durable_ingress(&self, service_cancel: CancellationToken) -> Result<()> {
            let mut service = self
                .service
                .lock()
                .map_err(|_| error("fixture binding lock poisoned"))?;
            if service.is_some() || service_cancel.is_cancelled() {
                return Err(error("fixture host cannot be rebound"));
            }
            let mut owner = self
                .driver
                .try_lock()
                .map_err(|_| error("fixture driver owner busy"))?;
            let data = self.data.clone();
            let observation = self.observation.clone();
            let hitl = self.original_hitl.clone();
            let allow_settlement = self.allow_settlement.clone();
            let instance = self.instance.clone();
            let delivery = self.delivery.clone();
            let kick = self.kick.clone();
            let cancel = service_cancel.clone();
            *owner = Some(tokio::spawn(async move {
                // One original claimed driver, independently of the DATA backlog.
                tokio::select! { biased; _ = cancel.cancelled() => return Ok(()), _ = kick.notified() => {} }
                let path = std::fs::read_dir(data.path())
                    .map_err(|_| error("durable DATA enumeration failed"))?
                    .filter_map(std::result::Result::ok)
                    .find(|entry| entry.file_name().to_string_lossy().starts_with("input-"))
                    .ok_or_else(|| error("claim had no durable DATA"))?
                    .path();
                let persisted =
                    std::fs::read(path).map_err(|_| error("claimed DATA unreadable"))?;
                let (_id, _text): (String, String) = serde_json::from_slice(&persisted)
                    .map_err(|_| error("claimed DATA invalid"))?;
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {},
                    _ = hitl.cancelled() => {
                        observation.send_modify(|state| state.hitl_resolved = state.hitl_resolved.saturating_add(1));
                        let target = instance.conversation_id().to_owned();
                        for text in ["原长流第一段🙂", "原长流第二段🙂"] {
                            let message = instance.bind_outbound(OutboundMessage::new("weixin", &target, ChatType::Direct, text));
                            outcome_result(delivery.send_with_context(message, None, &uuid::Uuid::new_v4().to_string()).await?)?;
                            observation.send_modify(|state| state.chunks_sent = state.chunks_sent.saturating_add(1));
                        }
                    }
                }
                cancel.cancelled().await;
                observation.send_modify(|state| {
                    state.cancellation_seen = state.cancellation_seen.saturating_add(1)
                });
                allow_settlement.cancelled().await;
                observation.send_modify(|state| {
                    state.physically_settled = state.physically_settled.saturating_add(1)
                });
                Ok(())
            }));
            *service = Some(service_cancel);
            Ok(())
        }
        async fn admit_with_ingress(
            &self,
            message: InboundMessage,
            cancel: CancellationToken,
            receipt: ChannelIngressReceipt,
        ) -> Result<()> {
            if cancel.is_cancelled() {
                return Err(error("fixture admission cancelled"));
            }
            let control = message.text == "/reply original-request yes";
            let fresh = self.persist(&message, control)?;
            self.observation.send_modify(|state| {
                state.active_admissions = state.active_admissions.saturating_add(1);
                state.peak_admissions = state.peak_admissions.max(state.active_admissions);
                if fresh {
                    if control {
                        state.controls = state.controls.saturating_add(1);
                    } else {
                        state.normal_admitted = state.normal_admitted.saturating_add(1);
                    }
                }
            });
            receipt.publish(if control {
                ChannelIngressDisposition::ControlHandled
            } else {
                ChannelIngressDisposition::DurableInput
            });
            if control {
                self.original_hitl.cancel();
            } else {
                self.kick.notify_one();
            }
            if let Some(hold) = self.hold_admissions.as_ref()
                && !control
            {
                tokio::select! { _ = hold.cancelled() => {}, _ = cancel.cancelled() => {} }
            }
            self.observation.send_modify(|state| {
                state.active_admissions = state.active_admissions.saturating_sub(1)
            });
            Ok(())
        }
        async fn close(&self) -> Result<()> {
            if let Some(service) = self
                .service
                .lock()
                .map_err(|_| error("fixture binding poisoned"))?
                .as_ref()
            {
                service.cancel();
            }
            // Borrow the actual owner across await: cancelling close does not detach it.
            let mut owner = self.driver.lock().await;
            if let Some(driver) = owner.as_mut() {
                let result = driver.await;
                *owner = None;
                result.map_err(|_| error("fixture driver join failed"))??;
            }
            self.observation
                .send_modify(|state| state.closed = state.closed.saturating_add(1));
            Ok(())
        }
    }

    async fn durable_poll_case(case: PollCase, count: usize, early_ack: bool) -> Result<()> {
        let mut fixture = PollFixture::start(case).await?;
        let (observation, mut observed) =
            tokio::sync::watch::channel(HandlerObservation::default());
        let allow_settlement = CancellationToken::new();
        let hold = early_ack.then(CancellationToken::new);
        let data =
            Arc::new(tempfile::tempdir().map_err(|_| error("host DATA fixture directory failed"))?);
        let mut channel = WeixinChannel::for_local_fixture(
            WeixinConfig {
                credentials: credentials(),
                store: super::super::state::tests::store(),
            },
            fixture.address,
        )?;
        let delivery = channel
            .delivery_handle()
            .ok_or_else(|| error("fixture original sender missing"))?;
        let make_data = data.clone();
        let make_allow = allow_settlement.clone();
        let make_hold = hold.clone();
        let factory = move |instance: &crate::channels::session::ChannelSessionInstance| -> Box<dyn MessageHandler> {
            Box::new(FileBackedHost {
                data: make_data.clone(), instance: instance.clone(), delivery: delivery.clone(),
                observation: observation.clone(), original_hitl: CancellationToken::new(), allow_settlement: make_allow.clone(),
                hold_admissions: make_hold.clone(), service: std::sync::Mutex::new(None), driver: tokio::sync::Mutex::new(None), kick: Arc::new(tokio::sync::Notify::new()),
            })
        };
        let handler = Arc::new(
            crate::channels::session::SessionHandler::new(
                crate::channels::session::SessionConfig::default(),
                factory,
            )
            .with_durable_ingress(),
        );
        channel.start(handler).await?;
        let mut capacity_check = Ok(());
        let mut held_snapshot = None;
        if let Some(hold) = hold.as_ref() {
            let held = wait_observation(&mut observed, |state| {
                state.active_admissions == INGRESS_CAPACITY
            })
            .await;
            // Several real server pages are read while original ACKs have already arrived.
            let repeated =
                wait_observation(&mut fixture.observation, |state| state.requests >= 3).await;
            let at_capacity = observed.borrow().clone();
            hold.cancel();
            capacity_check = held.and(repeated).map(|_| ());
            held_snapshot = Some(at_capacity);
        }
        let admission = wait_observation(&mut observed, |state| {
            state.normal_admitted == count && state.controls == 1 && state.chunks_sent == 2
        })
        .await;
        let cursor_progress = if admission.is_ok() {
            wait_observation(&mut fixture.observation, |state| {
                state.saw_first_cursor && state.saw_second_cursor && state.send_requests == 2
            })
            .await
        } else {
            Err(error(
                "durable DATA backlog prevented later original control",
            ))
        };
        let before_stop = observed.borrow().clone();
        // Drop the first stop waiter while the original claimed host is still physical.
        let first_stop = tokio::time::timeout(Duration::from_millis(100), channel.stop()).await;
        let cancellation =
            wait_observation(&mut observed, |state| state.cancellation_seen == 1).await;
        let retained = channel.handler.is_some();
        let not_stopped =
            channel.transport.subscribe().borrow().phase != ChannelTransportPhase::Stopped;
        allow_settlement.cancel();
        let stopped = tokio::time::timeout(Duration::from_secs(5), channel.stop())
            .await
            .map_err(|_| error("original channel stop did not settle"))?;
        let after_stop = observed.borrow().clone();
        let files = std::fs::read_dir(data.path())
            .map_err(|_| error("durable DATA inspection failed"))?
            .count();
        let server_stopped = fixture.shutdown().await;
        capacity_check?;
        admission?;
        cursor_progress?;
        cancellation?;
        stopped?;
        server_stopped?;
        if let Some(at_capacity) = held_snapshot {
            assert_eq!(at_capacity.normal_admitted, INGRESS_CAPACITY);
            assert_eq!(at_capacity.active_admissions, INGRESS_CAPACITY);
        }
        assert!(first_stop.is_err());
        assert!(retained && not_stopped);
        assert_eq!(before_stop.normal_admitted, count);
        assert_eq!(before_stop.controls, 1);
        assert_eq!(before_stop.hitl_resolved, 1);
        assert_eq!(before_stop.physically_settled, 0);
        assert!(before_stop.peak_admissions <= INGRESS_CAPACITY + CONTROL_CAPACITY);
        assert_eq!(after_stop.physically_settled, 1);
        assert_eq!(after_stop.closed, 1);
        assert_eq!(files, count.saturating_add(1));
        Ok(())
    }

    #[tokio::test]
    async fn actual_poll_file_backed_forty_inputs_reach_later_hitl_and_stop_joins_one_claimed_long_output()
    -> Result<()> {
        durable_poll_case(PollCase::FortyThenControl, 40, false).await
    }

    #[tokio::test]
    async fn actual_poll_128_data_pages_reach_later_control_without_response_tasks_per_input()
    -> Result<()> {
        durable_poll_case(PollCase::ManyPagesThenControl, 128, false).await
    }

    #[tokio::test]
    async fn actual_poll_early_ack_retains_all_32_real_admission_permits_until_return() -> Result<()>
    {
        durable_poll_case(PollCase::FortyThenControl, 40, true).await
    }

    /// 同一个生产poller/SessionHandler/file-backed ACK与cursor原语；只改变外部响应ret的presence。
    #[tokio::test]
    async fn actual_poll_optional_ret_keeps_original_data_hitl_and_stop_owner() -> Result<()> {
        for early_ack in [false, true] {
            durable_poll_case(PollCase::FortyThenControlWithoutRet, 40, early_ack).await?;
        }
        Ok(())
    }

    struct NoAcknowledgmentHandler(tokio::sync::watch::Sender<usize>);

    #[async_trait]
    impl MessageHandler for NoAcknowledgmentHandler {
        async fn handle(&self, _message: InboundMessage) -> Result<OutboundMessage> {
            Err(error("fixture requires ingress entry"))
        }
        async fn reply(&self, _message: OutboundMessage) -> Result<()> {
            Ok(())
        }
        fn bind_durable_ingress(&self, _cancel: CancellationToken) -> Result<()> {
            Ok(())
        }
        async fn admit_with_ingress(
            &self,
            _message: InboundMessage,
            _cancel: CancellationToken,
            receipt: ChannelIngressReceipt,
        ) -> Result<()> {
            self.0.send_modify(|calls| *calls = calls.saturating_add(1));
            drop(receipt);
            Ok(())
        }
    }

    #[tokio::test]
    async fn actual_poll_never_commits_dropped_receipt_or_unknown_business_cursor() -> Result<()> {
        for case in [
            PollCase::Unacknowledged,
            PollCase::Unknown,
            PollCase::Unrelated,
            PollCase::NullRet,
            PollCase::NullErrcode,
            PollCase::Rejected,
            PollCase::RejectedErrcode,
            PollCase::RejectedWithNullRet,
            PollCase::RejectedWithNullErrcode,
        ] {
            let mut fixture = PollFixture::start(case).await?;
            let (calls, called) = tokio::sync::watch::channel(0_usize);
            let handler = Arc::new(NoAcknowledgmentHandler(calls));
            let mut channel = WeixinChannel::for_local_fixture(
                WeixinConfig {
                    credentials: credentials(),
                    store: super::super::state::tests::store(),
                },
                fixture.address,
            )?;
            channel.start(handler).await?;
            let repeated =
                wait_observation(&mut fixture.observation, |state| state.requests >= 2).await;
            let cursor = channel.state.cursor().await;
            let transport_before_stop = channel.transport.subscribe().borrow().clone();
            let stopped = channel.stop().await;
            let calls = *called.borrow();
            let server_stopped = fixture.shutdown().await;
            let observed = repeated?;
            stopped?;
            server_stopped?;
            assert_eq!(cursor?, "");
            assert!(!observed.unexpected_cursor);
            assert_eq!(observed.send_requests, 0);
            // Snapshot comes from the original poller before Stop; HTTP200 alone is not Ready.
            match case {
                PollCase::Unacknowledged => {
                    assert!(calls >= 1);
                    assert_eq!(transport_before_stop.phase, ChannelTransportPhase::Ready);
                }
                PollCase::Unknown
                | PollCase::Unrelated
                | PollCase::NullRet
                | PollCase::NullErrcode => {
                    assert_eq!(calls, 0);
                    assert_eq!(transport_before_stop.phase, ChannelTransportPhase::Unknown);
                    assert!(transport_before_stop.evidence.is_none());
                }
                PollCase::Rejected
                | PollCase::RejectedErrcode
                | PollCase::RejectedWithNullRet
                | PollCase::RejectedWithNullErrcode => {
                    assert_eq!(calls, 0);
                    assert_eq!(
                        transport_before_stop.phase,
                        ChannelTransportPhase::Disconnected
                    );
                    assert!(transport_before_stop.evidence.is_none());
                    assert!(
                        transport_before_stop
                            .last_error
                            .as_deref()
                            .is_some_and(|value| value.contains("rejected (17)"))
                    );
                }
                PollCase::FortyThenControl
                | PollCase::FortyThenControlWithoutRet
                | PollCase::ManyPagesThenControl => {
                    return Err(error("unexpected test case"));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn local_fixture_constructor_accepts_only_loopback_endpoints() -> Result<()> {
        for address in [
            std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 12345)),
            std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, 12345)),
        ] {
            let channel = WeixinChannel::for_local_fixture(
                WeixinConfig {
                    credentials: credentials(),
                    store: super::super::state::tests::store(),
                },
                address,
            )?;
            assert!(channel.poll_task.is_none() && channel.send_task.is_none());
        }
        for address in [
            std::net::SocketAddr::from(([192, 0, 2, 1], 12345)),
            std::net::SocketAddr::from((
                std::net::Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1),
                12345,
            )),
        ] {
            assert!(
                WeixinChannel::for_local_fixture(
                    WeixinConfig {
                        credentials: credentials(),
                        store: super::super::state::tests::store(),
                    },
                    address,
                )
                .is_err_and(|error| error
                    .to_string()
                    .contains("fixture must use a loopback address"))
            );
        }
        Ok(())
    }

    #[test]
    fn local_fixture_constructor_keeps_production_credential_validation() {
        let mut invalid = credentials();
        invalid.base_url = "http://example.invalid/".into();
        assert!(
            WeixinChannel::for_local_fixture(
                WeixinConfig {
                    credentials: invalid,
                    store: super::super::state::tests::store(),
                },
                std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 12345)),
            )
            .is_err_and(|error| error.to_string().contains("not a valid HTTPS origin"))
        );
    }

    struct LegacyOnlyHost {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        closed: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl MessageHandler for LegacyOnlyHost {
        async fn handle(&self, _msg: InboundMessage) -> Result<OutboundMessage> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Err(error("legacy effect must not run"))
        }
        async fn reply(&self, _msg: OutboundMessage) -> Result<()> {
            Ok(())
        }
        async fn close(&self) -> Result<()> {
            self.closed.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    #[tokio::test]
    async fn unsupported_host_fails_start_before_accepting_spawning_or_legacy_effect() -> Result<()>
    {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let closed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut channel = WeixinChannel::new(WeixinConfig {
            credentials: credentials(),
            store: super::super::state::tests::store(),
        })?;
        let result = channel
            .start(Arc::new(LegacyOnlyHost {
                calls: calls.clone(),
                closed: closed.clone(),
            }))
            .await;
        assert!(result.is_err_and(|error| {
            error
                .to_string()
                .contains("does not support durable channel ingress ownership")
        }));
        assert!(!channel.accepting.load(Ordering::Acquire));
        assert!(channel.poll_task.is_none() && channel.send_task.is_none());
        assert_eq!(calls.load(Ordering::Acquire), 0);
        assert!(channel.handler.is_some());
        channel.stop().await?;
        assert_eq!(closed.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[tokio::test]
    async fn delivery_handle_context_validation_does_not_guess_latest_or_cross_account()
    -> Result<()> {
        let channel = WeixinChannel::new(WeixinConfig {
            credentials: credentials(),
            store: super::super::state::tests::store(),
        })?;
        let handle = channel
            .delivery_handle()
            .ok_or_else(|| error("missing original handle"))?;
        let target = encode_target("account-a", "peer-a")?;
        assert!(
            handle
                .validate_message_context(&target, "missing", None)
                .await
                .is_err()
        );
        channel
            .state
            .remember_message_context("peer-a", "absent", None)
            .await?;
        handle
            .validate_message_context(&target, "absent", None)
            .await?;
        let original = channel
            .state
            .remember_message_context("peer-a", "original", Some("original-token"))
            .await?;
        handle
            .validate_message_context(&target, "original", original.as_ref())
            .await?;
        let other = encode_target("account-b", "peer-a")?;
        assert!(
            handle
                .validate_message_context(&other, "absent", None)
                .await
                .is_err()
        );
        assert!(
            handle
                .validate_message_context(&target, "original", None)
                .await
                .is_err()
        );
        assert!(!channel.accepting.load(Ordering::Acquire));
        assert!(channel.poll_task.is_none() && channel.send_task.is_none());
        Ok(())
    }
}
