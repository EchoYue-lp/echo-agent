//! Runtime wiring helpers for layered memory.
//!
//! This module owns framework-level plumbing only: layer manager creation,
//! change-log construction and write observers. Product
//! lifecycle policy and scheduling remain in app code.

use super::{ChangeLog, EvolutionObserver, JsonlChangeLog, MemoryLayerManager};
use crate::memory::Store;
use crate::skills::hooks::{HookContext, HookRegistry};
use futures::future::BoxFuture;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Builds a [`MemoryLayerManager`] with consistent runtime wiring.
pub struct MemoryRuntimeIntegrationBuilder {
    echo_agent_dir: PathBuf,
    store: Arc<dyn Store>,
    evolution_observer: Option<Arc<dyn EvolutionObserver>>,
    change_log_path: Option<PathBuf>,
}

impl MemoryRuntimeIntegrationBuilder {
    /// Create a new builder for the given `.echo-agent/` directory and Store.
    pub fn new(echo_agent_dir: PathBuf, store: Arc<dyn Store>) -> Self {
        Self {
            echo_agent_dir,
            store,
            evolution_observer: None,
            change_log_path: None,
        }
    }

    /// Register an observer called after successful durable evolution events.
    pub fn evolution_observer(mut self, observer: Arc<dyn EvolutionObserver>) -> Self {
        self.evolution_observer = Some(observer);
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
        let mut layer_manager = MemoryLayerManager::new(
            self.echo_agent_dir.clone(),
            self.store.clone(),
            self.create_change_log(),
        );

        if let Some(observer) = &self.evolution_observer {
            layer_manager = layer_manager.with_evolution_observer(observer.clone());
        }

        layer_manager
    }
}

/// Adapter that publishes framework evolution events through a HookRegistry.
pub struct HookEvolutionObserver {
    hook_registry: Arc<RwLock<HookRegistry>>,
    session_id: String,
    agent_name: String,
}

impl HookEvolutionObserver {
    /// Bind evolution events to one agent's shared hook registry.
    pub fn new(
        hook_registry: Arc<RwLock<HookRegistry>>,
        session_id: impl Into<String>,
        agent_name: impl Into<String>,
    ) -> Self {
        Self {
            hook_registry,
            session_id: session_id.into(),
            agent_name: agent_name.into(),
        }
    }

    async fn fire(&self, context: HookContext) {
        let registry = self.hook_registry.read().await.clone();
        let _ = registry.run_lifecycle_hooks(&context).await;
    }

    fn memory_write_context(&self, key: &str, source: &str) -> HookContext {
        HookContext::for_post_memory_write(key, source, &self.session_id, &self.agent_name)
    }

    fn memory_layer_context(&self, key: &str, from_layer: &str, to_layer: &str) -> HookContext {
        HookContext::for_memory_layer_change(
            key,
            from_layer,
            to_layer,
            &self.session_id,
            &self.agent_name,
        )
    }

    fn skill_context(&self, event: echo_core::hooks::HookEvent, skill_name: &str) -> HookContext {
        HookContext::for_lifecycle(event, skill_name, &self.session_id, &self.agent_name)
    }
}

impl EvolutionObserver for HookEvolutionObserver {
    fn on_memory_write<'a>(&'a self, key: &'a str, source: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.fire(self.memory_write_context(key, source)).await;
        })
    }

    fn on_memory_layer_change<'a>(
        &'a self,
        key: &'a str,
        from_layer: &'a str,
        to_layer: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.fire(self.memory_layer_context(key, from_layer, to_layer))
                .await;
        })
    }

    fn on_skill_candidate_detected<'a>(&'a self, skill_name: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.fire(self.skill_context(
                echo_core::hooks::HookEvent::SkillCandidateDetected,
                skill_name,
            ))
            .await;
        })
    }

    fn on_skill_health_check<'a>(&'a self, skill_name: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.fire(
                self.skill_context(echo_core::hooks::HookEvent::SkillHealthCheck, skill_name),
            )
            .await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::memory::types::{MemoryMeta, MemorySource, MemoryType};
    use echo_state::memory::store::InMemoryStore;
    use std::sync::Mutex;

    struct RecordingObserver {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl EvolutionObserver for RecordingObserver {
        fn on_memory_write<'a>(&'a self, key: &'a str, source: &'a str) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                self.events
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(format!("write:{key}:{source}"));
            })
        }

        fn on_memory_layer_change<'a>(
            &'a self,
            key: &'a str,
            from_layer: &'a str,
            to_layer: &'a str,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                self.events
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(format!("layer:{key}:{from_layer}->{to_layer}"));
            })
        }
    }

    #[tokio::test]
    async fn builder_wires_typed_observer_and_change_log()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let echo_agent_dir = temp_dir.path().join(".echo-agent");
        let store = Arc::new(InMemoryStore::new());
        let events = Arc::new(Mutex::new(Vec::new()));
        let observer = Arc::new(RecordingObserver {
            events: events.clone(),
        });

        let manager = MemoryRuntimeIntegrationBuilder::new(echo_agent_dir.clone(), store)
            .evolution_observer(observer)
            .build_layer_manager();

        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "runtime",
        )
        .with_confidence(0.9)
        .with_stability(0.8);
        manager
            .write_memory("builder_test", "Use layered memory", meta)
            .await?;

        assert_eq!(
            *events.lock().unwrap_or_else(|error| error.into_inner()),
            vec![
                "write:builder_test:auto_extracted".to_string(),
                "layer:builder_test:warm->hot".to_string(),
            ]
        );
        assert!(
            echo_agent_dir
                .join("evolution")
                .join("change-log.jsonl")
                .exists()
        );
        Ok(())
    }

    #[test]
    fn hook_observer_builds_matcher_complete_contexts() {
        let observer = HookEvolutionObserver::new(
            Arc::new(RwLock::new(HookRegistry::new())),
            "session-1",
            "agent-1",
        );

        let write = observer.memory_write_context("memory-1", "explicit_save");
        assert_eq!(write.event, echo_core::hooks::HookEvent::PostMemoryWrite);
        assert_eq!(write.matcher.as_deref(), Some("explicit_save"));
        assert_eq!(write.memory_key.as_deref(), Some("memory-1"));
        assert_eq!(write.memory_source.as_deref(), Some("explicit_save"));

        let layer = observer.memory_layer_context("memory-1", "warm", "hot");
        assert_eq!(layer.event, echo_core::hooks::HookEvent::MemoryLayerChange);
        assert_eq!(layer.matcher.as_deref(), Some("warm->hot"));
        assert_eq!(layer.memory_from_layer.as_deref(), Some("warm"));
        assert_eq!(layer.memory_to_layer.as_deref(), Some("hot"));

        for event in [
            echo_core::hooks::HookEvent::SkillCandidateDetected,
            echo_core::hooks::HookEvent::SkillHealthCheck,
        ] {
            let context = observer.skill_context(event, "cargo-build");
            assert_eq!(context.event, event);
            assert_eq!(context.matcher.as_deref(), Some("cargo-build"));
            assert_eq!(context.session_id, "session-1");
            assert_eq!(context.agent_name, "agent-1");
        }
    }
}
