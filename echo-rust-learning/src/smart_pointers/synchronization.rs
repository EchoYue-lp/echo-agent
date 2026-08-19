//! `Arc<tokio::sync::RwLock<T>>` for asynchronous shared state.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Default)]
pub struct SharedProgress {
    inner: Arc<RwLock<HashMap<String, u8>>>,
}

/// Atomics are preferable to a lock for one independent numeric value.
#[derive(Debug, Clone, Default)]
pub struct AtomicProgress {
    percent: Arc<AtomicUsize>,
}

impl AtomicProgress {
    pub fn set(&self, percent: usize) {
        self.percent.store(percent.min(100), Ordering::Release);
    }

    pub fn get(&self) -> usize {
        self.percent.load(Ordering::Acquire)
    }
}

impl SharedProgress {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set(&self, task: impl Into<String>, percent: u8) {
        self.inner
            .write()
            .await
            .insert(task.into(), percent.min(100));
    }

    pub async fn get(&self, task: &str) -> Option<u8> {
        self.inner.read().await.get(task).copied()
    }

    pub async fn snapshot(&self) -> HashMap<String, u8> {
        self.inner.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clones_share_async_state() {
        let first = SharedProgress::new();
        let second = first.clone();
        second.set("compile", 120).await;
        assert_eq!(first.get("compile").await, Some(100));
    }

    #[test]
    fn atomic_progress_is_shared_without_a_lock() {
        let first = AtomicProgress::default();
        let second = first.clone();
        second.set(75);
        assert_eq!(first.get(), 75);
    }
}
