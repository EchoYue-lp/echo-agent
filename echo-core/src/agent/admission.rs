//! Product-neutral keyed execution admission.
//!
//! This module owns only the lifecycle of opaque execution keys. Applications
//! decide what a key means and which Agent or workspace it selects. The
//! admission owner tracks active leases, one process permit per active key,
//! retirement fences, and shutdown waiting without depending on application
//! state or persistence.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

/// Errors returned while admitting or retiring a keyed execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyedExecutionAdmissionError {
    /// New leases are rejected after [`KeyedExecutionAdmission::close`].
    Closed,
    /// The key is fenced by an in-progress retirement.
    Retiring { key: String },
    /// A second retirement receipt for the same key was requested.
    RetirementAlreadyActive { key: String },
    /// A lease counter would overflow.
    CapacityOverflow { key: String },
    /// The supplied process semaphore has no available permit.
    ProcessCapacity,
}

impl std::fmt::Display for KeyedExecutionAdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => formatter.write_str("keyed execution admission is closed"),
            Self::Retiring { key } => write!(formatter, "key '{key}' is retiring"),
            Self::RetirementAlreadyActive { key } => {
                write!(formatter, "key '{key}' already has an active retirement")
            }
            Self::CapacityOverflow { key } => {
                write!(
                    formatter,
                    "execution lease capacity overflow for key '{key}'"
                )
            }
            Self::ProcessCapacity => formatter.write_str("process execution capacity exhausted"),
        }
    }
}

impl std::error::Error for KeyedExecutionAdmissionError {}

#[derive(Default)]
struct AdmissionState {
    accepting: bool,
    total: usize,
    by_key: HashMap<String, usize>,
    process_permits: HashMap<String, OwnedSemaphorePermit>,
    retiring: HashSet<String>,
}

impl AdmissionState {
    fn accepting() -> Self {
        Self {
            accepting: true,
            ..Self::default()
        }
    }
}

/// Shared authority for opaque keyed execution admission.
///
/// A consumer may use one key for all leases that share an Agent or another
/// execution resource. Reusing a key does not consume another process permit;
/// the permit is held until the last lease for that key is dropped.
///
/// ```
/// use std::sync::Arc;
/// use tokio::sync::Semaphore;
/// use echo_core::agent::admission::KeyedExecutionAdmission;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let admission = Arc::new(KeyedExecutionAdmission::default());
/// let process_limit = Arc::new(Semaphore::new(1));
/// let lease = admission.issue_process_scoped("conversation", &process_limit)?;
/// assert_eq!(admission.active_count(), 1);
/// drop(lease);
/// admission.wait_until_idle().await;
/// assert_eq!(admission.active_count(), 0);
/// # Ok(())
/// # }
/// ```
pub struct KeyedExecutionAdmission {
    state: Mutex<AdmissionState>,
    idle: Notify,
}

impl Default for KeyedExecutionAdmission {
    fn default() -> Self {
        Self {
            state: Mutex::new(AdmissionState::accepting()),
            idle: Notify::new(),
        }
    }
}

impl KeyedExecutionAdmission {
    fn lock_state(&self) -> MutexGuard<'_, AdmissionState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Admit one lease without a process-wide semaphore.
    pub fn issue(
        self: &Arc<Self>,
        key: impl Into<String>,
    ) -> Result<KeyedExecutionLease, KeyedExecutionAdmissionError> {
        self.issue_inner(key.into(), None)
    }

    /// Admit one lease while reserving one process permit for a new key.
    ///
    /// Additional leases for an already active key reuse that key's existing
    /// permit. The final lease drop releases it.
    pub fn issue_process_scoped(
        self: &Arc<Self>,
        key: impl Into<String>,
        process_semaphore: &Arc<Semaphore>,
    ) -> Result<KeyedExecutionLease, KeyedExecutionAdmissionError> {
        self.issue_inner(key.into(), Some(Arc::clone(process_semaphore)))
    }

    fn issue_inner(
        self: &Arc<Self>,
        key: String,
        process_semaphore: Option<Arc<Semaphore>>,
    ) -> Result<KeyedExecutionLease, KeyedExecutionAdmissionError> {
        {
            let mut state = self.lock_state();
            self.ensure_accepting(&state, &key)?;
            if self.increment_existing(&mut state, &key)? {
                return Ok(KeyedExecutionLease {
                    admission: Arc::clone(self),
                    key,
                    active: true,
                });
            }
        }

        let process_permit = match process_semaphore {
            Some(semaphore) => match semaphore.try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(tokio::sync::TryAcquireError::Closed) => {
                    return Err(KeyedExecutionAdmissionError::Closed);
                }
                Err(tokio::sync::TryAcquireError::NoPermits) => {
                    let mut state = self.lock_state();
                    self.ensure_accepting(&state, &key)?;
                    if self.increment_existing(&mut state, &key)? {
                        return Ok(KeyedExecutionLease {
                            admission: Arc::clone(self),
                            key,
                            active: true,
                        });
                    }
                    return Err(KeyedExecutionAdmissionError::ProcessCapacity);
                }
            },
            None => None,
        };

        let mut state = self.lock_state();
        self.ensure_accepting(&state, &key)?;
        if self.increment_existing(&mut state, &key)? {
            drop(process_permit);
        } else {
            let total = state.total.checked_add(1).ok_or_else(|| {
                KeyedExecutionAdmissionError::CapacityOverflow { key: key.clone() }
            })?;
            if let Some(permit) = process_permit {
                state.process_permits.insert(key.clone(), permit);
            }
            state.total = total;
            state.by_key.insert(key.clone(), 1);
        }

        Ok(KeyedExecutionLease {
            admission: Arc::clone(self),
            key,
            active: true,
        })
    }

    fn ensure_accepting(
        &self,
        state: &AdmissionState,
        key: &str,
    ) -> Result<(), KeyedExecutionAdmissionError> {
        if !state.accepting {
            return Err(KeyedExecutionAdmissionError::Closed);
        }
        if state.retiring.contains(key) {
            return Err(KeyedExecutionAdmissionError::Retiring {
                key: key.to_string(),
            });
        }
        Ok(())
    }

    fn increment_existing(
        &self,
        state: &mut AdmissionState,
        key: &str,
    ) -> Result<bool, KeyedExecutionAdmissionError> {
        let Some(existing) = state.by_key.get(key).copied() else {
            return Ok(false);
        };
        let next_key_count = existing.checked_add(1).ok_or_else(|| {
            KeyedExecutionAdmissionError::CapacityOverflow {
                key: key.to_string(),
            }
        })?;
        state.total = state.total.checked_add(1).ok_or_else(|| {
            KeyedExecutionAdmissionError::CapacityOverflow {
                key: key.to_string(),
            }
        })?;
        state.by_key.insert(key.to_string(), next_key_count);
        Ok(true)
    }

    /// Whether at least one lease currently retains `key`.
    pub fn is_active(&self, key: &str) -> bool {
        self.lock_state()
            .by_key
            .get(key)
            .copied()
            .unwrap_or_default()
            != 0
    }

    /// Whether new leases for `key` are fenced by a retirement receipt.
    pub fn is_retiring(&self, key: &str) -> bool {
        self.lock_state().retiring.contains(key)
    }

    /// Number of issued leases retained by all keys.
    pub fn active_count(&self) -> usize {
        self.lock_state().total
    }

    /// Permanently reject new leases while allowing existing leases to settle.
    pub fn close(&self) {
        self.lock_state().accepting = false;
        self.idle.notify_waiters();
    }

    /// Fence one key until the returned receipt is dropped.
    pub fn begin_retirement(
        self: &Arc<Self>,
        key: impl Into<String>,
    ) -> Result<KeyedExecutionRetirement, KeyedExecutionAdmissionError> {
        let key = key.into();
        let mut state = self.lock_state();
        if !state.accepting {
            return Err(KeyedExecutionAdmissionError::Closed);
        }
        if !state.retiring.insert(key.clone()) {
            return Err(KeyedExecutionAdmissionError::RetirementAlreadyActive { key });
        }
        drop(state);
        Ok(KeyedExecutionRetirement {
            admission: Arc::clone(self),
            key,
            active: true,
        })
    }

    /// Wait until all leases for one key have been dropped.
    pub async fn wait_key_idle(&self, key: &str) {
        loop {
            let notified = self.idle.notified();
            if !self.is_active(key) {
                return;
            }
            notified.await;
        }
    }

    /// Wait until all issued leases have been dropped.
    pub async fn wait_until_idle(&self) {
        loop {
            let notified = self.idle.notified();
            if self.active_count() == 0 {
                return;
            }
            notified.await;
        }
    }
}

/// Non-cloneable lease that retains one keyed execution admission.
#[must_use]
pub struct KeyedExecutionLease {
    admission: Arc<KeyedExecutionAdmission>,
    key: String,
    active: bool,
}

impl KeyedExecutionLease {
    /// Whether this lease belongs to the exact admission owner and key.
    pub fn owns(&self, admission: &Arc<KeyedExecutionAdmission>, key: &str) -> bool {
        self.active && Arc::ptr_eq(&self.admission, admission) && self.key == key
    }
}

impl Drop for KeyedExecutionLease {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let mut state = self.admission.lock_state();
        state.total = state.total.saturating_sub(1);
        let mut release_process_permit = false;
        if let Some(count) = state.by_key.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.by_key.remove(&self.key);
                release_process_permit = true;
            }
        }
        if release_process_permit {
            state.process_permits.remove(&self.key);
        }
        let key_idle = !state.by_key.contains_key(&self.key);
        let all_idle = state.total == 0;
        drop(state);
        if key_idle || all_idle {
            self.admission.idle.notify_waiters();
        }
    }
}

/// Non-cloneable fence that rejects new leases for one key until dropped.
#[must_use]
pub struct KeyedExecutionRetirement {
    admission: Arc<KeyedExecutionAdmission>,
    key: String,
    active: bool,
}

/// Composition boundary for shared execution admission.
///
/// This combines keyed lifecycle tracking with an optional process-wide
/// semaphore. Consumers share this value across runtime and subagent entry
/// points; the keyed primitive remains the sole owner of lease accounting.
pub struct ExecutionAdmission {
    keyed: Arc<KeyedExecutionAdmission>,
    process_semaphore: Option<Arc<Semaphore>>,
}

impl std::fmt::Debug for ExecutionAdmission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecutionAdmission")
            .field("active_count", &self.active_count())
            .field("bounded", &self.process_semaphore.is_some())
            .finish()
    }
}

impl ExecutionAdmission {
    /// Create an admission with a process-wide capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            keyed: Arc::new(KeyedExecutionAdmission::default()),
            process_semaphore: Some(Arc::new(Semaphore::new(capacity.max(1)))),
        }
    }

    /// Create an admission without process-wide capacity enforcement.
    pub fn unbounded() -> Self {
        Self {
            keyed: Arc::new(KeyedExecutionAdmission::default()),
            process_semaphore: None,
        }
    }

    /// Return the keyed authority shared by this composition.
    pub fn keyed(&self) -> &Arc<KeyedExecutionAdmission> {
        &self.keyed
    }

    /// Issue one lease for an opaque execution key.
    pub fn issue(
        &self,
        key: impl Into<String>,
    ) -> Result<KeyedExecutionLease, KeyedExecutionAdmissionError> {
        match &self.process_semaphore {
            Some(semaphore) => self.keyed.issue_process_scoped(key, semaphore),
            None => self.keyed.issue(key),
        }
    }

    /// Number of active leases across all keys.
    pub fn active_count(&self) -> usize {
        self.keyed.active_count()
    }

    /// Close this admission and reject future leases.
    pub fn close(&self) {
        self.keyed.close();
    }

    /// Wait until all issued leases have settled.
    pub async fn wait_until_idle(&self) {
        self.keyed.wait_until_idle().await;
    }
}

impl Default for ExecutionAdmission {
    fn default() -> Self {
        Self::unbounded()
    }
}

impl KeyedExecutionRetirement {
    /// Whether this retirement belongs to the exact admission owner and key.
    pub fn owns(&self, admission: &Arc<KeyedExecutionAdmission>, key: &str) -> bool {
        self.active && Arc::ptr_eq(&self.admission, admission) && self.key == key
    }
}

impl Drop for KeyedExecutionRetirement {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        self.admission.lock_state().retiring.remove(&self.key);
        self.admission.idle.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn same_key_reuses_one_process_permit() -> Result<(), String> {
        let admission = Arc::new(KeyedExecutionAdmission::default());
        let semaphore = Arc::new(Semaphore::new(1));
        let first = admission
            .issue_process_scoped("same", &semaphore)
            .map_err(|error| error.to_string())?;
        let second = admission
            .issue_process_scoped("same", &semaphore)
            .map_err(|error| error.to_string())?;
        assert_eq!(admission.active_count(), 2);
        assert_eq!(semaphore.available_permits(), 0);
        drop(first);
        assert_eq!(admission.active_count(), 1);
        drop(second);
        admission.wait_until_idle().await;
        assert_eq!(semaphore.available_permits(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn different_keys_observe_process_capacity() -> Result<(), String> {
        let admission = Arc::new(KeyedExecutionAdmission::default());
        let semaphore = Arc::new(Semaphore::new(1));
        let first = admission
            .issue_process_scoped("first", &semaphore)
            .map_err(|error| error.to_string())?;
        assert!(matches!(
            admission.issue_process_scoped("second", &semaphore),
            Err(KeyedExecutionAdmissionError::ProcessCapacity)
        ));
        drop(first);
        let second = admission
            .issue_process_scoped("second", &semaphore)
            .map_err(|error| error.to_string())?;
        drop(second);
        Ok(())
    }

    #[tokio::test]
    async fn retirement_and_close_are_fail_closed() -> Result<(), String> {
        let admission = Arc::new(KeyedExecutionAdmission::default());
        let retirement = admission
            .begin_retirement("retiring")
            .map_err(|error| error.to_string())?;
        assert!(matches!(
            admission.issue("retiring"),
            Err(KeyedExecutionAdmissionError::Retiring { key }) if key == "retiring"
        ));
        drop(retirement);
        let lease = admission
            .issue("retiring")
            .map_err(|error| error.to_string())?;
        admission.close();
        assert!(matches!(
            admission.issue("closed"),
            Err(KeyedExecutionAdmissionError::Closed)
        ));
        drop(lease);
        admission.wait_until_idle().await;
        Ok(())
    }

    #[tokio::test]
    async fn execution_admission_shares_capacity_and_closes() -> Result<(), String> {
        let admission = ExecutionAdmission::with_capacity(1);
        let first = admission
            .issue("first")
            .map_err(|error| error.to_string())?;
        assert!(matches!(
            admission.issue("second"),
            Err(KeyedExecutionAdmissionError::ProcessCapacity)
        ));
        admission.close();
        drop(first);
        admission.wait_until_idle().await;
        assert!(matches!(
            admission.issue("third"),
            Err(KeyedExecutionAdmissionError::Closed)
        ));
        Ok(())
    }
}
