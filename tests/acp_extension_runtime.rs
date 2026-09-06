//! Shared extension invocation runtime regression (supreme plan 06, todo
//! `build-shared-extension-runtime`).
//!
//! The tests drive the protocol-neutral
//! [`echo_agent::acp::ExtensionInvocationAuthority`] with a controllable
//! fake bridge transport: the fake acquires leases exactly like the SDK-host
//! bridge does (lease → send → await with deadline → settle exactly once),
//! so concurrency permits, deadline settlement, cancellation propagation,
//! late-response discard, re-entry conflicts and the close ordering are all
//! observed against the real lifecycle implementation.

#![cfg(feature = "acp")]

use echo_agent::acp::{
    AcpAdapterConfig, AcpConnectionServices, ExtensionInvocationAuthority, ExtensionLeaseError,
    ExtensionSettlement,
};
use echo_agent::acp::{AcpSessionContext, AcpSessionFactory, SessionRegistry};
use echo_agent::agent::EventIdentity;
use echo_agent::agent::{CancellationToken, EventEnvelope};
use echo_agent::error::Result;
use echo_agent::runtime::{TurnMode, TurnRequest};
use futures::StreamExt as _;
use futures::future::BoxFuture;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// One fake reverse invocation driven like the real bridge drives it: lease,
/// await the transport with a deadline, settle exactly once, then observe
/// late outcomes. `TransportBehavior` controls what the fake SDK side does.
struct FakeBridge {
    authority: Arc<ExtensionInvocationAuthority>,
}

enum TransportBehavior {
    /// Answer after the given delay.
    AnswerAfter(Duration),
    /// Never answer; the deadline or cancellation settles the lease.
    Silent,
}

impl FakeBridge {
    fn new(authority: Arc<ExtensionInvocationAuthority>) -> Arc<Self> {
        Arc::new(Self { authority })
    }

    /// Drive one non-streaming invocation to completion and return the
    /// settlement the caller observed, mirroring the bridge's select!.
    /// Consumes the bridge handle so the future is `'static` and can be
    /// spawned like a real proxy task.
    async fn invoke(
        self: Arc<Self>,
        exclusive_key: Option<&'static str>,
        framework_cancellation: CancellationToken,
        behavior: TransportBehavior,
        deadline: Duration,
    ) -> ExtensionSettlement {
        let mut lease = match self.authority.lease(exclusive_key, framework_cancellation) {
            Ok(lease) => lease,
            Err(error) => panic!("lease acquisition failed: {error}"),
        };
        let cancellation = lease.cancellation();
        tokio::select! {
            answered = async {
                match behavior {
                    TransportBehavior::AnswerAfter(delay) => {
                        tokio::time::sleep(delay).await;
                    }
                    TransportBehavior::Silent => {
                        std::future::pending::<()>().await;
                    }
                }
            } => {
                let _ = answered;
                lease.settle(ExtensionSettlement::Answered);
                ExtensionSettlement::Answered
            }
            () = cancellation.cancelled() => {
                // The bridge sends the cancel notice, then settles locally.
                lease.settle(ExtensionSettlement::Cancelled);
                ExtensionSettlement::Cancelled
            }
            _ = tokio::time::sleep(deadline) => {
                lease.settle_timeout();
                ExtensionSettlement::TimedOut
            }
        }
    }
}

fn authority(concurrency: usize) -> Arc<ExtensionInvocationAuthority> {
    ExtensionInvocationAuthority::new(concurrency)
        .ok()
        .expect("positive concurrency")
}

#[tokio::test]
async fn concurrent_invocations_share_permits_and_fail_fast_when_exhausted() {
    let bridge = FakeBridge::new(authority(2));
    let first = tokio::spawn(bridge.clone().invoke(
        None,
        CancellationToken::new(),
        TransportBehavior::Silent,
        Duration::from_secs(30),
    ));
    let second = tokio::spawn(bridge.clone().invoke(
        None,
        CancellationToken::new(),
        TransportBehavior::Silent,
        Duration::from_secs(30),
    ));
    // Wait until both invocations hold their permits.
    while bridge.authority.in_flight() < 2 {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let third = bridge.authority.lease(None, CancellationToken::new());
    assert_eq!(third.err(), Some(ExtensionLeaseError::ConcurrencyLimit));
    assert_eq!(bridge.authority.in_flight(), 2);
    // Releasing the two silent leases (via cancellation) frees the permits.
    let cancel_all = bridge.authority.cancel_all();
    assert_eq!(cancel_all, 0);
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.ok(), Some(ExtensionSettlement::Cancelled));
    assert_eq!(second.ok(), Some(ExtensionSettlement::Cancelled));
    let remaining = bridge.authority.drain(Duration::from_millis(200)).await;
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn deadline_settles_timeout_and_late_answers_are_discarded() {
    let bridge = FakeBridge::new(authority(4));
    // The fake SDK stays silent; the 25ms deadline must settle a timeout.
    let observed = bridge
        .clone()
        .invoke(
            None,
            CancellationToken::new(),
            TransportBehavior::Silent,
            Duration::from_millis(25),
        )
        .await;
    assert_eq!(observed, ExtensionSettlement::TimedOut);
    // A late answer arriving after settlement must not overwrite it.
    let mut lease = bridge
        .authority
        .lease(None, CancellationToken::new())
        .ok()
        .expect("permit freed after timeout");
    assert!(lease.settle_timeout());
    assert!(!lease.settle(ExtensionSettlement::Answered));
    assert_eq!(lease.settlement(), Some(ExtensionSettlement::TimedOut));
    drop(lease);
}

#[tokio::test]
async fn framework_cancellation_settles_cancelled_before_the_deadline() {
    let bridge = FakeBridge::new(authority(2));
    let framework = CancellationToken::new();
    let invocation = bridge.clone().invoke(
        None,
        framework.clone(),
        TransportBehavior::Silent,
        Duration::from_secs(30),
    );
    // The owning run cancels long before the deadline.
    tokio::time::sleep(Duration::from_millis(10)).await;
    framework.cancel();
    let observed = invocation.await;
    assert_eq!(observed, ExtensionSettlement::Cancelled);
    let remaining = bridge.authority.drain(Duration::from_millis(200)).await;
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn exclusive_reentry_returns_conflict_without_waiting() {
    let authority = authority(4);
    let key = "extension-1/human_loop_request";
    let exclusive = authority
        .lease(Some(key), CancellationToken::new())
        .ok()
        .expect("exclusive lease");
    let reentrant = authority.lease(Some(key), CancellationToken::new());
    assert_eq!(
        reentrant.err(),
        Some(ExtensionLeaseError::ExclusiveConflict),
        "re-entrant exclusive mutation must be a typed conflict, not a wait"
    );
    // Independent registrations keep working while one holds its lease.
    assert!(
        authority
            .lease(
                Some("extension-2/human_loop_request"),
                CancellationToken::new()
            )
            .is_ok()
    );
    drop(exclusive);
    assert!(authority.lease(Some(key), CancellationToken::new()).is_ok());
}

#[tokio::test]
async fn teardown_closes_admission_then_cancels_then_drains_in_order() {
    let bridge = FakeBridge::new(authority(4));
    let in_flight = tokio::spawn(bridge.clone().invoke(
        Some("extension-1/custom_agent"),
        CancellationToken::new(),
        TransportBehavior::Silent,
        Duration::from_secs(30),
    ));
    while bridge.authority.in_flight() < 1 {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    // Close order from the adapter close chain: admission, cancel, drain.
    bridge.authority.close_admission();
    assert_eq!(
        bridge.authority.lease(None, CancellationToken::new()).err(),
        Some(ExtensionLeaseError::AdmissionClosed)
    );
    bridge.authority.cancel_all();
    let observed = in_flight.await.ok();
    assert_eq!(observed, Some(ExtensionSettlement::Cancelled));
    let leaked = bridge.authority.drain(Duration::from_millis(200)).await;
    assert_eq!(
        leaked, 0,
        "a cancelled callback that returned must release its lease"
    );
}

// ── Connection-level wiring ─────────────────────────────────────────────────

struct NullAgent {
    turns: Arc<AtomicUsize>,
}

impl echo_agent::agent::Agent for NullAgent {
    fn name(&self) -> &str {
        "extension-runtime-fixture"
    }

    fn model_name(&self) -> &str {
        "fixture-model"
    }

    fn system_prompt(&self) -> &str {
        "fixture"
    }

    fn execute<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async { Ok("done".to_string()) })
    }

    fn execute_stream<'a>(
        &'a self,
        _task: &'a str,
    ) -> BoxFuture<'a, Result<futures::stream::BoxStream<'a, Result<echo_agent::agent::AgentEvent>>>>
    {
        Box::pin(async {
            let turns = self.turns.clone();
            Ok(futures::stream::once(async move {
                turns.fetch_add(1, Ordering::AcqRel);
                Ok(echo_agent::agent::AgentEvent::FinalAnswer(
                    "done".to_string(),
                ))
            })
            .boxed())
        })
    }
}

fn test_services() -> Arc<AcpConnectionServices> {
    let turns = Arc::new(AtomicUsize::new(0));
    let factory = move |_context: AcpSessionContext| {
        let turns = turns.clone();
        async move { Ok(Box::new(NullAgent { turns }) as Box<dyn echo_agent::agent::Agent>) }
    };
    let factory: Arc<dyn AcpSessionFactory> = Arc::new(factory);
    let registry = Arc::new(SessionRegistry::new(factory, 8));
    let config = Arc::new(AcpAdapterConfig::default());
    Arc::new(AcpConnectionServices::new(registry, config))
}

/// The shared connection services expose exactly one extension authority and
/// the unified close chain drains it with every other connection state.
#[tokio::test]
async fn connection_services_own_one_extension_authority_and_close_drains_it() -> Result<()> {
    let services = test_services();
    let authority = services.extensions();
    assert!(authority.admission_open());

    let held = authority
        .lease(Some("extension-1/tool_execute"), CancellationToken::new())
        .ok()
        .expect("lease");
    assert_eq!(authority.in_flight(), 1);

    // The unified close chain: admission first, then in-flight cancellation,
    // then a bounded drain before Sessions close.
    services.close_admission();
    authority.close_admission();
    authority.cancel_all();
    assert_eq!(
        held.settlement(),
        Some(ExtensionSettlement::Cancelled),
        "connection close must settle in-flight extension invocations"
    );
    drop(held);
    let leaked = authority.drain(Duration::from_millis(200)).await;
    assert_eq!(leaked, 0);
    services.close_sessions().await?;
    Ok(())
}

/// A prompt-driven run keeps using the same Session/Run authority while an
/// extension lease runs next to it; the run settles via the framework
/// receipt, the lease settles via its own lifecycle, and neither settles the
/// other (design §12.1: one Run authority, one invocation authority).
#[tokio::test]
async fn extension_leases_coexist_with_framework_runs_without_second_terminal() -> Result<()> {
    let services = test_services();
    services
        .sessions()
        .initialize(agent_client_protocol::schema::v1::ClientCapabilities::default())
        .await;
    let session = services
        .sessions()
        .create(agent_client_protocol::schema::v1::NewSessionRequest::new(
            std::path::PathBuf::from("/tmp/extension-runtime"),
        ))
        .await?;
    let acp_session = services
        .sessions()
        .get(&session)
        .await
        .ok_or_else(|| echo_agent::error::ReactError::Other("session missing".to_string()))?;
    let active = acp_session.begin_turn()?;
    let run_id = active.turn.id().to_string();
    let identity = EventIdentity::new(format!("stream-{run_id}"), run_id.clone())?;
    let turn = TurnRequest::new(identity, "run next to an extension lease")
        .mode(TurnMode::Chat)
        .cancel(active.turn.cancellation());
    let spec = echo_agent::acp::RunStartSpec {
        session: acp_session.clone(),
        active,
        run_id: run_id.clone(),
        stream_id: format!("stream-{run_id}"),
        turn,
        projector: None,
        journal: None,
        observers: Vec::new(),
    };
    let (entry, task) = services.prepare_run(spec).await?;
    tokio::spawn(task);

    let bridge = FakeBridge::new(services.extensions().clone());
    let invocation = bridge.invoke(
        None,
        CancellationToken::new(),
        TransportBehavior::AnswerAfter(Duration::from_millis(5)),
        Duration::from_secs(5),
    );
    let receipt = entry.wait_receipt().await;
    assert_eq!(
        receipt.outcome,
        echo_agent::runtime::TurnOutcome::Completed,
        "the framework receipt stays the only run terminal"
    );
    assert_eq!(invocation.await, ExtensionSettlement::Answered);
    let _ = services.remove_run(&run_id).await;
    Ok(())
}

// Silence the unused helper warning when EventEnvelope is only referenced in
// doc position on some feature combinations.
#[allow(dead_code)]
fn _envelope_type_witness() -> Option<EventEnvelope> {
    None
}
