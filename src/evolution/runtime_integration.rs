//! Runtime wiring helpers for layered memory.
//!
//! This module owns framework-level plumbing only: layer manager creation,
//! change-log construction, shared write counters, and write observers. Product
//! lifecycle policy such as session-end review scheduling remains in app code.

use super::{ChangeLog, JsonlChangeLog, MemoryLayerManager, MemoryWriteObserver};
use crate::memory::Store;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// Builds a [`MemoryLayerManager`] with consistent runtime wiring.
pub struct MemoryRuntimeIntegrationBuilder {
    echo_agent_dir: PathBuf,
    store: Arc<dyn Store>,
    write_counter: Option<Arc<AtomicU64>>,
    review_every_n_writes: u64,
    write_observer: Option<Arc<dyn MemoryWriteObserver>>,
    change_log_path: Option<PathBuf>,
}

impl MemoryRuntimeIntegrationBuilder {
    /// Create a new builder for the given `.echo-agent/` directory and Store.
    pub fn new(echo_agent_dir: PathBuf, store: Arc<dyn Store>) -> Self {
        Self {
            echo_agent_dir,
            store,
            write_counter: None,
            review_every_n_writes: 50,
            write_observer: None,
            change_log_path: None,
        }
    }

    /// Share an external write counter with the layer manager.
    pub fn write_counter(mut self, counter: Arc<AtomicU64>) -> Self {
        self.write_counter = Some(counter);
        self
    }

    /// Configure the review trigger threshold observed by the layer manager.
    pub fn review_every_n_writes(mut self, every_n: u64) -> Self {
        self.review_every_n_writes = every_n.max(1);
        self
    }

    /// Register an observer called after successful real memory writes.
    pub fn write_observer(mut self, observer: Arc<dyn MemoryWriteObserver>) -> Self {
        self.write_observer = Some(observer);
        self
    }

    /// Override the JSONL change-log path.
    pub fn change_log_path(mut self, path: PathBuf) -> Self {
        self.change_log_path = Some(path);
        self
    }

    /// Return the default change-log path for this integration.
    pub fn default_change_log_path(&self) -> PathBuf {
        self.echo_agent_dir
            .join("evolution")
            .join("change-log.jsonl")
    }

    /// Create a JSONL change log, ensuring the parent directory exists.
    pub fn create_change_log(&self) -> Box<dyn ChangeLog> {
        let log_path = self
            .change_log_path
            .clone()
            .unwrap_or_else(|| self.default_change_log_path());
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Box::new(JsonlChangeLog::new(log_path))
    }

    /// Create the fully wired layer manager.
    pub fn build_layer_manager(&self) -> MemoryLayerManager {
        let counter = self
            .write_counter
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicU64::new(0)));
        let mut layer_manager = MemoryLayerManager::new(
            self.echo_agent_dir.clone(),
            self.store.clone(),
            self.create_change_log(),
        )
        .with_review_trigger(counter, self.review_every_n_writes);

        if let Some(observer) = &self.write_observer {
            layer_manager = layer_manager.with_write_observer(observer.clone());
        }

        layer_manager
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::memory::types::{MemoryMeta, MemorySource, MemoryType};
    use echo_state::memory::store::InMemoryStore;
    use futures::future::BoxFuture;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingObserver {
        count: Arc<AtomicUsize>,
    }

    impl MemoryWriteObserver for CountingObserver {
        fn on_memory_write<'a>(&'a self) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                self.count.fetch_add(1, Ordering::Relaxed);
            })
        }
    }

    #[tokio::test]
    async fn builder_wires_counter_observer_and_change_log() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let echo_agent_dir = temp_dir.path().join(".echo-agent");
        let store = Arc::new(InMemoryStore::new());
        let counter = Arc::new(AtomicU64::new(0));
        let observer_count = Arc::new(AtomicUsize::new(0));
        let observer = Arc::new(CountingObserver {
            count: observer_count.clone(),
        });

        let manager = MemoryRuntimeIntegrationBuilder::new(echo_agent_dir.clone(), store)
            .write_counter(counter.clone())
            .review_every_n_writes(7)
            .write_observer(observer)
            .build_layer_manager();

        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "runtime",
        )
        .with_confidence(0.8);
        manager
            .write_memory("builder_test", "Use layered memory", meta)
            .await
            .expect("write memory");

        assert_eq!(counter.load(Ordering::Relaxed), 1);
        assert_eq!(observer_count.load(Ordering::Relaxed), 1);
        assert!(
            echo_agent_dir
                .join("evolution")
                .join("change-log.jsonl")
                .exists()
        );
    }
}
