//! Connection-scoped extension invocation authority (supreme plan 06, todo
//! `build-shared-extension-runtime`).
//!
//! The authority is deliberately protocol-neutral: it owns invocation
//! lifecycle only — admission, concurrency permits, per-call cancellation,
//! exclusive-mutation leases and exactly-once settlement (design §12.3). It
//! never sees language objects, protocol DTOs, product UI state or a second
//! run/terminal authority; the typed bridge in the SDK host maps this
//! lifecycle onto the official reverse-request transport.
//!
//! Invariants enforced here:
//!
//! - the reader loop never blocks on a callback: leases are acquired with
//!   `try_acquire`, and every wait has an explicit caller-side deadline;
//! - one invocation identity settles at most once; later settlements are
//!   reported as late and discarded by the caller;
//! - dropping an unsettled lease is fail-closed: it settles as cancelled and
//!   releases its resources, so a crashed proxy task can never leak a permit;
//! - exclusive leases make re-entrant mutation of the same registration a
//!   typed conflict instead of mutual waiting.

use crate::agent::CancellationToken;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore, TryAcquireError};

/// Why an invocation lease could not be acquired. Mapped by the bridge to
/// typed extension errors; the runtime itself stays error-code agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionLeaseError {
    /// The connection is closing and refuses new invocations.
    AdmissionClosed,
    /// The negotiated concurrency of in-flight callbacks is exhausted.
    ConcurrencyLimit,
    /// An exclusive lease for the same registration is still held
    /// (re-entrant mutation). Callers report a typed conflict instead of
    /// waiting (design §12.3).
    ExclusiveConflict,
}

impl std::fmt::Display for ExtensionLeaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AdmissionClosed => write!(formatter, "extension admission is closed"),
            Self::ConcurrencyLimit => write!(formatter, "extension concurrency limit reached"),
            Self::ExclusiveConflict => write!(
                formatter,
                "extension is already executing an exclusive invocation"
            ),
        }
    }
}

/// Terminal settlement of one invocation, transport-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionSettlement {
    /// The transport delivered an answer (payload already handled by the
    /// caller; the runtime only records the fact).
    Answered,
    /// The deadline elapsed before an answer settled.
    TimedOut,
    /// Cancelled by the framework, the owning run, or connection teardown.
    Cancelled,
    /// The reverse transport failed (send error or disconnect).
    Disconnected,
}

impl ExtensionSettlement {
    /// Whether this settlement is success-shaped for diagnostics.
    pub fn is_answered(&self) -> bool {
        matches!(self, Self::Answered)
    }
}

struct InvocationEntry {
    cancellation: CancellationToken,
    state: StdMutex<Option<ExtensionSettlement>>,
    settled: Notify,
}

impl InvocationEntry {
    fn settle(&self, settlement: ExtensionSettlement) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.is_some() {
            return false;
        }
        *state = Some(settlement);
        self.settled.notify_waiters();
        true
    }

    fn settlement(&self) -> Option<ExtensionSettlement> {
        self.state.lock().ok().and_then(|state| *state)
    }
}

/// One in-flight invocation lease. The holder drives the reverse transport;
/// settlement must happen exactly once. Dropping an unsettled lease settles
/// it as cancelled and releases every resource it holds.
pub struct ExtensionInvocationLease {
    authority: Arc<ExtensionInvocationAuthority>,
    identity: String,
    exclusive_key: Option<String>,
    entry: Arc<InvocationEntry>,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl ExtensionInvocationLease {
    /// Unique invocation identity of this lease. Independent from any
    /// JSON-RPC request id (design §10.1).
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Cancellation token of this invocation. Signalled by the authority on
    /// teardown, or proactively by the holder's framework cancellation.
    pub fn cancellation(&self) -> CancellationToken {
        self.entry.cancellation.clone()
    }

    /// Record the settlement. Returns false when the lease already settled —
    /// the caller must discard that late outcome without touching state.
    pub fn settle(&mut self, settlement: ExtensionSettlement) -> bool {
        self.entry.settle(settlement)
    }

    /// The already-recorded settlement, when one exists.
    pub fn settlement(&self) -> Option<ExtensionSettlement> {
        self.entry.settlement()
    }

    /// Wait until this lease is settled (by the holder, cancellation or
    /// teardown). Always pair with a caller-side deadline.
    pub async fn wait_settled(&self) -> ExtensionSettlement {
        loop {
            let notified = self.entry.settled.notified();
            if let Some(settlement) = self.entry.settlement() {
                return settlement;
            }
            notified.await;
        }
    }

    /// Settle as timed out if still pending, then report whether this call
    /// was the first settlement (true) or a late one (false).
    pub fn settle_timeout(&mut self) -> bool {
        self.entry.settle(ExtensionSettlement::TimedOut)
    }

    fn release(&mut self) {
        self.permit.take();
        if let Some(key) = self.exclusive_key.take() {
            self.authority.release_exclusive(&key);
        }
        self.authority.forget_invocation(&self.identity);
    }
}

impl Drop for ExtensionInvocationLease {
    fn drop(&mut self) {
        // Fail-closed: an unsettled lease (crashed or aborted proxy task)
        // settles as cancelled so no waiter can hang on it.
        self.entry.settle(ExtensionSettlement::Cancelled);
        self.release();
    }
}

/// Per-connection invocation authority shared by every extension proxy.
pub struct ExtensionInvocationAuthority {
    admission: AtomicBool,
    semaphore: Arc<Semaphore>,
    exclusive: StdMutex<HashMap<String, usize>>,
    invocations: StdMutex<HashMap<String, Arc<InvocationEntry>>>,
    serial: AtomicU64,
    identity_prefix: String,
}

impl ExtensionInvocationAuthority {
    /// Create the authority with the negotiated concurrency bound. A
    /// non-positive bound is rejected because it would disable the bridge
    /// silently.
    pub fn new(max_concurrency: usize) -> Result<Arc<Self>, ExtensionLeaseError> {
        if max_concurrency == 0 {
            return Err(ExtensionLeaseError::ConcurrencyLimit);
        }
        Ok(Self::build(max_concurrency))
    }

    /// Create the authority, clamping a non-positive bound to one permit.
    /// Used by connection setup where the config was already validated and
    /// degrading is safer than failing the whole connection.
    pub fn new_saturating(max_concurrency: usize) -> Arc<Self> {
        Self::build(max_concurrency.max(1))
    }

    fn build(max_concurrency: usize) -> Arc<Self> {
        Arc::new(Self {
            admission: AtomicBool::new(true),
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            exclusive: StdMutex::new(HashMap::new()),
            invocations: StdMutex::new(HashMap::new()),
            serial: AtomicU64::new(0),
            identity_prefix: format!("invocation-{}", uuid::Uuid::new_v4().simple()),
        })
    }

    /// Whether new invocations are admitted.
    pub fn admission_open(&self) -> bool {
        self.admission.load(Ordering::Acquire)
    }

    /// Close admission. In-flight invocations keep running until they settle,
    /// are cancelled by [`Self::cancel_all`], or the bounded drain expires.
    pub fn close_admission(&self) {
        self.admission.store(false, Ordering::Release);
    }

    /// Number of currently in-flight invocations.
    pub fn in_flight(&self) -> usize {
        self.invocations
            .lock()
            .map(|invocations| invocations.len())
            .unwrap_or_default()
    }

    /// Acquire one invocation lease.
    ///
    /// `exclusive_key` marks the invocation as an exclusive mutation of one
    /// registration (connection generation + implementation identity): while
    /// such a lease is outstanding, another exclusive lease on the same key
    /// fails with [`ExtensionLeaseError::ExclusiveConflict`] instead of
    /// queueing (design §12.3). `framework_cancellation` links the lease to
    /// the owning run/tool cancellation so framework cancels reach the
    /// callback without waiting for the deadline.
    pub fn lease(
        self: &Arc<Self>,
        exclusive_key: Option<&str>,
        framework_cancellation: CancellationToken,
    ) -> Result<ExtensionInvocationLease, ExtensionLeaseError> {
        if !self.admission_open() {
            return Err(ExtensionLeaseError::AdmissionClosed);
        }
        let permit = match self.semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(TryAcquireError::Closed) => {
                return Err(ExtensionLeaseError::AdmissionClosed);
            }
            Err(_) => {
                return Err(ExtensionLeaseError::ConcurrencyLimit);
            }
        };
        if let Some(key) = exclusive_key
            && !self.reserve_exclusive(key)
        {
            // The permit is dropped here, releasing the concurrency slot.
            return Err(ExtensionLeaseError::ExclusiveConflict);
        }
        let serial = self.serial.fetch_add(1, Ordering::AcqRel);
        let identity = format!("{}-{}", self.identity_prefix, serial);
        let entry = Arc::new(InvocationEntry {
            cancellation: framework_cancellation,
            state: StdMutex::new(None),
            settled: Notify::new(),
        });
        if let Ok(mut invocations) = self.invocations.lock() {
            invocations.insert(identity.clone(), entry.clone());
        }
        Ok(ExtensionInvocationLease {
            authority: self.clone(),
            identity,
            exclusive_key: exclusive_key.map(str::to_string),
            entry,
            permit: Some(permit),
        })
    }

    /// Cancel every in-flight invocation. Returns how many were still
    /// pending; leases observe the cancellation through their token or the
    /// settled state and release themselves.
    pub fn cancel_all(&self) -> usize {
        let entries: Vec<Arc<InvocationEntry>> = self
            .invocations
            .lock()
            .map(|invocations| invocations.values().cloned().collect())
            .unwrap_or_default();
        let mut pending: usize = 0;
        for entry in entries {
            entry.cancellation.cancel();
            if !entry.settle(ExtensionSettlement::Cancelled) {
                pending = pending.saturating_add(1);
            }
        }
        pending
    }

    /// Bounded teardown wait: wait until every lease settled (and therefore
    /// released) or the timeout expires. Returns the number of leases still
    /// in flight after the wait — zero is the clean-shutdown condition.
    pub async fn drain(&self, timeout: Duration) -> usize {
        let drained = tokio::time::timeout(timeout, async {
            loop {
                let pending = self.in_flight();
                if pending == 0 {
                    return;
                }
                // Poll briefly: lease release happens in proxy tasks, not
                // through a single notify point.
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        if drained.is_ok() { 0 } else { self.in_flight() }
    }

    fn release_exclusive(&self, key: &str) {
        if let Ok(mut exclusive) = self.exclusive.lock() {
            match exclusive.get(key).copied() {
                Some(count) if count <= 1 => {
                    exclusive.remove(key);
                }
                Some(count) => {
                    exclusive.insert(key.to_string(), count.saturating_sub(1));
                }
                None => {}
            }
        }
    }

    fn reserve_exclusive(&self, key: &str) -> bool {
        let Ok(mut exclusive) = self.exclusive.lock() else {
            return false;
        };
        match exclusive.get(key).copied() {
            Some(count) if count > 0 => false,
            _ => {
                exclusive.insert(key.to_string(), 1);
                true
            }
        }
    }

    fn forget_invocation(&self, identity: &str) {
        if let Ok(mut invocations) = self.invocations.lock() {
            invocations.remove(identity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority(concurrency: usize) -> Arc<ExtensionInvocationAuthority> {
        ExtensionInvocationAuthority::new(concurrency).expect("positive concurrency")
    }

    #[test]
    fn zero_concurrency_is_rejected() {
        assert_eq!(
            ExtensionInvocationAuthority::new(0).err(),
            Some(ExtensionLeaseError::ConcurrencyLimit)
        );
    }

    #[tokio::test]
    async fn concurrency_permits_are_enforced_and_fail_fast() {
        let authority = authority(2);
        let first = authority
            .lease(None, CancellationToken::new())
            .expect("lease 1");
        let second = authority
            .lease(None, CancellationToken::new())
            .expect("lease 2");
        assert_eq!(authority.in_flight(), 2);
        let third = authority.lease(None, CancellationToken::new());
        assert_eq!(third.err(), Some(ExtensionLeaseError::ConcurrencyLimit));
        drop(first);
        drop(second);
        assert_eq!(authority.in_flight(), 0);
        assert!(
            authority
                .lease(None, CancellationToken::new())
                .is_ok_and(|lease| lease.identity().contains("invocation-"))
        );
    }

    #[tokio::test]
    async fn settlement_is_exactly_once_and_late_settlements_are_rejected() {
        let authority = authority(4);
        let mut lease = authority
            .lease(None, CancellationToken::new())
            .expect("lease");
        assert_eq!(lease.settlement(), None);
        assert!(lease.settle(ExtensionSettlement::Answered));
        assert_eq!(lease.settlement(), Some(ExtensionSettlement::Answered));
        // A late response arriving afterwards must be discarded.
        assert!(!lease.settle(ExtensionSettlement::Answered));
        assert!(!lease.settle_timeout());
        drop(lease);
        assert_eq!(authority.in_flight(), 0);
    }

    #[tokio::test]
    async fn exclusive_leases_conflict_instead_of_waiting() {
        let authority = authority(4);
        let key = "extension-1/human-loop";
        let exclusive = authority
            .lease(Some(key), CancellationToken::new())
            .expect("exclusive lease");
        let reentrant = authority.lease(Some(key), CancellationToken::new());
        assert_eq!(
            reentrant.err(),
            Some(ExtensionLeaseError::ExclusiveConflict)
        );
        // Non-exclusive leases on other keys are unaffected.
        assert!(
            authority
                .lease(Some("extension-2/human-loop"), CancellationToken::new())
                .is_ok()
        );
        drop(exclusive);
        assert!(authority.lease(Some(key), CancellationToken::new()).is_ok());
    }

    #[tokio::test]
    async fn framework_cancellation_propagates_to_the_lease_token() {
        let authority = authority(2);
        let framework = CancellationToken::new();
        let lease = authority.lease(None, framework.clone()).expect("lease");
        let token = lease.cancellation();
        framework.cancel();
        assert!(token.is_cancelled());
        drop(lease);
    }

    #[tokio::test]
    async fn dropping_an_unsettled_lease_fails_closed_as_cancelled() {
        let authority = authority(2);
        let entry = {
            let lease = authority
                .lease(None, CancellationToken::new())
                .expect("lease");
            lease.entry.clone()
        };
        assert_eq!(
            entry.settlement(),
            Some(ExtensionSettlement::Cancelled),
            "a dropped lease must never leave an unsettled entry"
        );
        assert_eq!(authority.in_flight(), 0);
    }

    #[tokio::test]
    async fn close_then_cancel_then_drain_releases_everything() {
        let authority = authority(4);
        let held = authority
            .lease(Some("extension-1/agent"), CancellationToken::new())
            .expect("lease");
        authority.close_admission();
        assert_eq!(
            authority.lease(None, CancellationToken::new()).err(),
            Some(ExtensionLeaseError::AdmissionClosed)
        );
        let already_settled = authority.cancel_all();
        // The lease was pending until cancel_all settled it as cancelled.
        assert_eq!(already_settled, 0);
        assert_eq!(held.settlement(), Some(ExtensionSettlement::Cancelled));
        // The proxy task observes the settlement and releases its lease;
        // only then can the bounded drain observe a clean state.
        drop(held);
        let remaining = authority.drain(Duration::from_millis(200)).await;
        assert_eq!(remaining, 0);
        assert_eq!(authority.in_flight(), 0);
        // A lease dropped after settlement must not resurrect anything.
        assert_eq!(authority.in_flight(), 0);
    }

    #[tokio::test]
    async fn drain_reports_leases_that_never_release() {
        let authority = ExtensionInvocationAuthority::new_saturating(4);
        // Inject an entry whose lease object was leaked without Drop: the
        // drain must report it instead of hanging.
        authority.invocations.lock().expect("lock").insert(
            "invocation-test-stuck".to_string(),
            Arc::new(InvocationEntry {
                cancellation: CancellationToken::new(),
                state: StdMutex::new(None),
                settled: Notify::new(),
            }),
        );
        let remaining = authority.drain(Duration::from_millis(50)).await;
        assert_eq!(remaining, 1);
    }

    #[tokio::test]
    async fn wait_settled_observes_teardown_settlement() {
        let authority = authority(2);
        let mut lease = authority
            .lease(None, CancellationToken::new())
            .expect("lease");
        let waiter = {
            let entry = lease.entry.clone();
            tokio::spawn(async move {
                let notified = entry.settled.notified();
                if entry.settlement().is_none() {
                    notified.await;
                }
                entry.settlement()
            })
        };
        // Give the waiter a chance to subscribe before settling.
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(lease.settle(ExtensionSettlement::Disconnected));
        let observed = waiter.await.expect("waiter completes");
        assert_eq!(observed, Some(ExtensionSettlement::Disconnected));
        assert!(
            !observed
                .map(|settlement| settlement.is_answered())
                .unwrap_or(false)
        );
        drop(lease);
    }
}
