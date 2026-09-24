use std::collections::HashSet;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub(super) struct ResourceOwnerRegistry {
    state: Mutex<ResourceOwnerState>,
    cleanup: tokio::sync::Mutex<()>,
}

#[derive(Debug, Default)]
struct ResourceOwnerState {
    active: HashSet<String>,
    debt: HashSet<String>,
}

impl ResourceOwnerRegistry {
    pub(super) async fn lock_cleanup(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.cleanup.lock().await
    }

    pub(super) fn reserve(&self, name: String) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active.insert(name);
    }

    pub(super) fn settle(&self, name: &str) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active.remove(name);
        state.debt.remove(name);
    }

    pub(super) fn retain_debt(&self, name: &str) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active.remove(name);
        state.debt.insert(name.to_string());
    }

    pub(super) fn snapshot(&self) -> (Vec<String>, Vec<String>) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut active = state.active.iter().cloned().collect::<Vec<_>>();
        let mut debt = state.debt.iter().cloned().collect::<Vec<_>>();
        active.sort();
        debt.sort();
        (active, debt)
    }
}
