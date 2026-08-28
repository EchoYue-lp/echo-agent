//! Subagent registry — discovery, registration, and lifecycle management
//!
//! Wraps the existing `SubagentMap` with declarative definitions, factory support,
//! and lifecycle events. Backward compatible — `register_agent()` still works.

use crate::agent::SubagentMap;
use crate::error::Result;
use echo_core::agent::Agent;
use futures::future::BoxFuture;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Notify, RwLock};
use tracing::{debug, info, warn};

use super::events::SubagentEventBus;
use super::types::{RegisteredSubagent, SubagentDefinition};

struct RegistryEntry {
    definition: SubagentDefinition,
    agent: Option<Arc<dyn Agent>>,
    factory: Option<Arc<dyn AgentFactory>>,
    revision: u64,
}

#[derive(Default)]
struct RegistryState {
    entries: HashMap<String, RegistryEntry>,
    next_revision: u64,
}

impl RegistryState {
    fn next_revision(&mut self) -> u64 {
        self.next_revision = self.next_revision.saturating_add(1);
        self.next_revision
    }
}

// ── Agent Factory ─────────────────────────────────────────────────────────────

/// Factory trait for lazy agent instantiation.
///
/// Used when you want to register an agent definition but defer
/// the actual agent construction until it's first dispatched.
pub trait AgentFactory: Send + Sync {
    /// Create an agent instance asynchronously.
    ///
    /// # Returns
    /// A boxed future that resolves to a `Result<Box<dyn Agent>>`.
    fn create(&self) -> BoxFuture<'static, Result<Box<dyn Agent>>>;
}

/// Type-erased closure wrapper for `AgentFactory`.
pub struct FnAgentFactory<F>
where
    F: Fn() -> BoxFuture<'static, Result<Box<dyn Agent>>> + Send + Sync,
{
    f: F,
}

impl<F> FnAgentFactory<F>
where
    F: Fn() -> BoxFuture<'static, Result<Box<dyn Agent>>> + Send + Sync,
{
    /// Create a new function-based agent factory.
    ///
    /// # Parameters
    /// * `f` - Async closure that creates an agent when invoked.
    pub fn new(f: F) -> Self {
        Self { f }
    }
}

impl<F> AgentFactory for FnAgentFactory<F>
where
    F: Fn() -> BoxFuture<'static, Result<Box<dyn Agent>>> + Send + Sync,
{
    fn create(&self) -> BoxFuture<'static, Result<Box<dyn Agent>>> {
        (self.f)()
    }
}

// ── Subagent Registry ─────────────────────────────────────────────────────────

/// Registry for subagent definitions and instances.
///
/// Wraps the existing `SubagentMap` and adds:
/// - Definition-based lookup
/// - Factory support for lazy instantiation
/// - Lifecycle events
pub struct SubagentRegistry {
    /// One atomic record per name keeps definition and executable generation aligned.
    state: Arc<RwLock<RegistryState>>,
    executable_catalog: Arc<std::sync::RwLock<Vec<SubagentDefinition>>>,
    catalog_revision: Arc<AtomicU64>,
    /// Names currently being instantiated (prevents double-creation races).
    instantiating: Arc<RwLock<HashSet<String>>>,
    /// Notifier for waiters on factory instantiation completion.
    instantiating_done: Arc<Notify>,
    /// Event bus for lifecycle events.
    event_bus: SubagentEventBus,
}

impl SubagentRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(RegistryState::default())),
            executable_catalog: Arc::new(std::sync::RwLock::new(Vec::new())),
            catalog_revision: Arc::new(AtomicU64::new(0)),
            instantiating: Arc::new(RwLock::new(HashSet::new())),
            instantiating_done: Arc::new(Notify::new()),
            event_bus: SubagentEventBus::new(),
        }
    }

    /// Create with a specific event bus.
    pub fn with_event_bus(event_bus: SubagentEventBus) -> Self {
        Self {
            state: Arc::new(RwLock::new(RegistryState::default())),
            executable_catalog: Arc::new(std::sync::RwLock::new(Vec::new())),
            catalog_revision: Arc::new(AtomicU64::new(0)),
            instantiating: Arc::new(RwLock::new(HashSet::new())),
            instantiating_done: Arc::new(Notify::new()),
            event_bus,
        }
    }

    /// Migrate from an existing `SubagentMap` (backward compatible).
    ///
    /// Each agent gets a default Sync-mode `BuiltIn` definition.
    pub fn from_subagent_map(map: SubagentMap) -> Self {
        let registry = Self::new();
        if let Ok(agents) = map.read() {
            let Ok(mut state) = registry.state.try_write() else {
                return registry;
            };
            for (name, agent) in agents.iter() {
                let def = SubagentDefinition::simple_sync(name.clone());
                let revision = state.next_revision();
                state.entries.insert(
                    name.clone(),
                    RegistryEntry {
                        definition: def,
                        agent: Some(agent.clone()),
                        factory: None,
                        revision,
                    },
                );
            }
            registry.publish_catalog(&state);
        }
        registry
    }

    fn publish_catalog(&self, state: &RegistryState) {
        let mut definitions = state
            .entries
            .values()
            .filter(|entry| entry.agent.is_some() || entry.factory.is_some())
            .map(|entry| entry.definition.clone())
            .collect::<Vec<_>>();
        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        *self
            .executable_catalog
            .write()
            .unwrap_or_else(|error| error.into_inner()) = definitions;
        self.catalog_revision.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn executable_catalog_handle(
        &self,
    ) -> Arc<std::sync::RwLock<Vec<SubagentDefinition>>> {
        Arc::clone(&self.executable_catalog)
    }

    pub(crate) fn catalog_revision_handle(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.catalog_revision)
    }

    // ── Registration ──────────────────────────────────────────────────────

    /// Register a pre-built agent with its definition.
    pub async fn register(&self, def: SubagentDefinition, agent: Box<dyn Agent>) {
        self.register_shared(def, Arc::from(agent)).await;
    }

    /// Register a shared pre-built Agent without wrapping it in a second
    /// execution container. This is the programmatic composition path used by
    /// Team and agents-as-tools style integrations.
    pub async fn register_shared(&self, def: SubagentDefinition, agent: Arc<dyn Agent>) {
        let name = def.name.clone();
        info!(subagent = %name, mode = %def.execution_mode, "Registering subagent");

        let mut state = self.state.write().await;
        let revision = state.next_revision();
        state.entries.insert(
            name.clone(),
            RegistryEntry {
                definition: def,
                agent: Some(agent),
                factory: None,
                revision,
            },
        );
        self.publish_catalog(&state);
        drop(state);

        self.event_bus
            .emit(super::events::SubagentEvent::Registered { name: name.clone() });
    }

    /// Sync registration — uses `try_write` to avoid `block_on` deadlock.
    ///
    /// Use this from synchronous contexts (e.g., builder pattern, `main()`).
    /// Falls back to logging a warning if locks are contended.
    pub fn register_sync(&self, def: SubagentDefinition, agent: Box<dyn Agent>) -> bool {
        self.register_shared_sync(def, Arc::from(agent))
    }

    /// Synchronous counterpart to [`register_shared`](Self::register_shared).
    /// Returns `false` rather than blocking when the registry is contended.
    pub fn register_shared_sync(&self, def: SubagentDefinition, agent: Arc<dyn Agent>) -> bool {
        let name = def.name.clone();

        let ok = match self.state.try_write() {
            Ok(mut state) => {
                let revision = state.next_revision();
                state.entries.insert(
                    name.clone(),
                    RegistryEntry {
                        definition: def,
                        agent: Some(agent),
                        factory: None,
                        revision,
                    },
                );
                self.publish_catalog(&state);
                true
            }
            Err(_) => {
                warn!(subagent = %name, "Lock contention on subagent registry, registration deferred");
                false
            }
        };

        if ok {
            self.event_bus
                .emit(super::events::SubagentEvent::Registered { name });
        }

        ok
    }

    /// Register a definition with a factory for lazy instantiation.
    pub async fn register_factory(&self, def: SubagentDefinition, factory: Arc<dyn AgentFactory>) {
        let name = def.name.clone();
        debug!(subagent = %name, "Registering subagent factory");

        let mut state = self.state.write().await;
        let revision = state.next_revision();
        state.entries.insert(
            name,
            RegistryEntry {
                definition: def,
                agent: None,
                factory: Some(factory),
                revision,
            },
        );
        self.publish_catalog(&state);
    }

    /// Register a **definition only** — no agent instance and no factory.
    ///
    /// This is a low-level late-binding path for runtimes that can guarantee
    /// later hydration: the definition becomes discoverable
    /// (`list_available`, `agent_names`, dispatch catalog) before any
    /// executable instance exists. A later call to
    /// [`register`](Self::register) / [`register_sync`](Self::register_sync)
    /// / [`register_factory`](Self::register_factory) under the same name
    /// supplies (or lazily produces) the instance, overwriting the definition.
    ///
    /// Dispatching a definition registered this way **without** subsequently
    /// providing an instance will fail at execution time (no agent to run) —
    /// this is intentional, so callers cannot silently get a no-op agent.
    ///
    /// Returns `true` if inserted (always, under uncontended locks).
    pub async fn register_definition(&self, def: SubagentDefinition) -> bool {
        let name = def.name.clone();
        debug!(subagent = %name, "Registering subagent definition (no instance)");
        let mut state = self.state.write().await;
        let revision = state.next_revision();
        state.entries.insert(
            name,
            RegistryEntry {
                definition: def,
                agent: None,
                factory: None,
                revision,
            },
        );
        self.publish_catalog(&state);
        true
    }

    /// Sync variant of [`register_definition`](Self::register_definition).
    ///
    /// Uses `try_write` to avoid `block_on` deadlock from synchronous
    /// contexts that cannot await this registry.
    /// Logs a warning and returns `false` on lock contention.
    pub fn register_definition_sync(&self, def: SubagentDefinition) -> bool {
        let name = def.name.clone();
        match self.state.try_write() {
            Ok(mut state) => {
                let revision = state.next_revision();
                state.entries.insert(
                    name,
                    RegistryEntry {
                        definition: def,
                        agent: None,
                        factory: None,
                        revision,
                    },
                );
                self.publish_catalog(&state);
                true
            }
            Err(error) => {
                warn!(subagent = %name, %error, "Cannot register subagent definition");
                false
            }
        }
    }

    /// Register a definition factory while constructing an agent synchronously.
    pub fn register_factory_sync(
        &self,
        def: SubagentDefinition,
        factory: Arc<dyn AgentFactory>,
    ) -> bool {
        let name = def.name.clone();
        let mut state = match self.state.try_write() {
            Ok(state) => state,
            Err(error) => {
                warn!(subagent = %name, %error, "Cannot register subagent definition");
                return false;
            }
        };

        let revision = state.next_revision();
        state.entries.insert(
            name,
            RegistryEntry {
                definition: def,
                agent: None,
                factory: Some(factory),
                revision,
            },
        );
        self.publish_catalog(&state);
        true
    }

    /// Remove a subagent by name.
    pub async fn remove(&self, name: &str) {
        let mut state = self.state.write().await;
        state.entries.remove(name);
        self.publish_catalog(&state);
        drop(state);

        self.event_bus
            .emit(super::events::SubagentEvent::Unregistered {
                name: name.to_string(),
            });
    }

    // ── Lookup ────────────────────────────────────────────────────────────

    /// Look up a registered subagent. Returns None if not found.
    pub async fn get(&self, name: &str) -> Option<RegisteredSubagent> {
        let state = self.state.read().await;
        let entry = state.entries.get(name)?;

        Some(RegisteredSubagent {
            definition: entry.definition.clone(),
            has_instance: entry.agent.is_some(),
        })
    }

    /// Get the agent instance for immediate execution.
    ///
    /// If the agent was registered via factory and not yet instantiated,
    /// this will create it on demand.
    ///
    /// Uses a loop with timeout to handle concurrent instantiation attempts
    /// rather than relying on a single `notified()` call.
    pub async fn get_agent(&self, name: &str) -> Option<Arc<dyn Agent>> {
        use std::time::Duration;

        // Check if already instantiated
        {
            let state = self.state.read().await;
            if let Some(agent) = state
                .entries
                .get(name)
                .and_then(|entry| entry.agent.clone())
            {
                return Some(agent);
            }
        }

        // Try factory
        let factory_revision = {
            let state = self.state.read().await;
            state.entries.get(name).and_then(|entry| {
                entry
                    .factory
                    .clone()
                    .map(|factory| (factory, entry.revision))
            })
        };

        if let Some((factory, factory_revision)) = factory_revision {
            // Prevent concurrent double-instantiation
            {
                let mut in_progress = self.instantiating.write().await;
                if in_progress.contains(name) {
                    debug!(subagent = %name, "Factory instantiation already in progress, waiting");
                    drop(in_progress);
                    // Loop-check with timeout instead of single notified()
                    let timeout = Duration::from_secs(30);
                    let start = std::time::Instant::now();
                    loop {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        // Check if agent has been created
                        {
                            let state = self.state.read().await;
                            if let Some(agent) = state
                                .entries
                                .get(name)
                                .and_then(|entry| entry.agent.clone())
                            {
                                return Some(agent);
                            }
                        }
                        // Check if instantiation failed (removed from instantiating but not in agents)
                        {
                            let in_progress = self.instantiating.read().await;
                            if !in_progress.contains(name) {
                                // Instantiation finished (success or failure), re-check agents
                                let state = self.state.read().await;
                                return state
                                    .entries
                                    .get(name)
                                    .and_then(|entry| entry.agent.clone());
                            }
                        }
                        if start.elapsed() > timeout {
                            warn!(subagent = %name, "Timeout waiting for agent instantiation");
                            return None;
                        }
                    }
                }
                in_progress.insert(name.to_string());
            }

            info!(subagent = %name, "Instantiating agent from factory");
            let result = factory.create().await;

            // Clean up instantiating guard and notify waiters
            {
                let mut in_progress = self.instantiating.write().await;
                in_progress.remove(name);
            }
            self.instantiating_done.notify_waiters();

            match result {
                Ok(agent) => {
                    let arc_agent = Arc::new(agent);
                    let mut state = self.state.write().await;
                    let entry = state.entries.get_mut(name)?;
                    if entry.revision != factory_revision {
                        warn!(subagent = %name, "Discarding stale subagent factory result");
                        return entry.agent.clone();
                    }
                    entry.agent = Some(arc_agent.clone());
                    self.publish_catalog(&state);
                    return Some(arc_agent);
                }
                Err(e) => {
                    warn!(subagent = %name, error = %e, "Factory instantiation failed");
                    return None;
                }
            }
        }

        None
    }

    /// Create a fresh agent when a factory is registered for `name`.
    ///
    /// Unlike `get_agent`, this never updates or reuses the cached agent.
    pub async fn create_fresh_agent(&self, name: &str) -> Result<Option<Arc<dyn Agent>>> {
        let factory = {
            let state = self.state.read().await;
            state
                .entries
                .get(name)
                .and_then(|entry| entry.factory.clone())
        };

        match factory {
            Some(factory) => factory
                .create()
                .await
                .map(|agent| Some(Arc::new(agent) as Arc<dyn Agent>)),
            None => Ok(None),
        }
    }

    /// Check if a subagent is registered.
    pub async fn contains(&self, name: &str) -> bool {
        self.state.read().await.entries.contains_key(name)
    }

    /// List all available subagent definitions.
    pub async fn list_available(&self) -> Vec<SubagentDefinition> {
        let state = self.state.read().await;
        state
            .entries
            .values()
            .filter(|entry| entry.agent.is_some() || entry.factory.is_some())
            .map(|entry| entry.definition.clone())
            .collect()
    }

    /// List subagent definitions matching a tag.
    pub async fn list_by_tag(&self, tag: &str) -> Vec<SubagentDefinition> {
        let state = self.state.read().await;
        state
            .entries
            .values()
            .filter(|entry| entry.agent.is_some() || entry.factory.is_some())
            .map(|entry| &entry.definition)
            .filter(|definition| definition.tags.iter().any(|entry_tag| entry_tag == tag))
            .cloned()
            .collect()
    }

    /// Get agent names for tool description (convenience).
    pub async fn agent_names(&self) -> Vec<String> {
        let state = self.state.read().await;
        state
            .entries
            .iter()
            .filter(|(_, entry)| entry.agent.is_some() || entry.factory.is_some())
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Get the event bus reference.
    pub fn event_bus(&self) -> &SubagentEventBus {
        &self.event_bus
    }
}

impl Default for SubagentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for SubagentRegistry {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            executable_catalog: self.executable_catalog.clone(),
            catalog_revision: self.catalog_revision.clone(),
            instantiating: self.instantiating.clone(),
            instantiating_done: self.instantiating_done.clone(),
            event_bus: self.event_bus.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockAgent;

    #[tokio::test]
    async fn test_register_and_get() {
        let registry = SubagentRegistry::new();
        let agent = MockAgent::new("researcher");
        let def = SubagentDefinition::new("researcher", "Researches topics");

        registry.register(def, Box::new(agent)).await;

        assert!(registry.contains("researcher").await);
        let registered = registry.get("researcher").await.unwrap();
        assert_eq!(registered.definition.name, "researcher");
        assert!(registered.has_instance);
    }

    #[tokio::test]
    async fn shared_registration_preserves_agent_identity() -> Result<()> {
        let registry = SubagentRegistry::new();
        let shared: Arc<dyn Agent> = Arc::new(MockAgent::new("shared"));
        registry
            .register_shared(
                SubagentDefinition::new("shared", "Shared programmatic Agent"),
                shared.clone(),
            )
            .await;

        let resolved = registry.get_agent("shared").await.ok_or_else(|| {
            crate::error::ReactError::Other("shared Agent was not registered".to_string())
        })?;
        assert!(Arc::ptr_eq(&shared, &resolved));
        Ok(())
    }

    #[tokio::test]
    async fn test_remove() {
        let registry = SubagentRegistry::new();
        let agent = MockAgent::new("subagent");
        let def = SubagentDefinition::new("subagent", "Subagent agent");

        registry.register(def, Box::new(agent)).await;
        assert!(registry.contains("subagent").await);

        registry.remove("subagent").await;
        assert!(!registry.contains("subagent").await);
    }

    #[tokio::test]
    async fn test_list_available() {
        let registry = SubagentRegistry::new();

        let a1 = MockAgent::new("a1");
        let a2 = MockAgent::new("a2");

        registry
            .register(SubagentDefinition::new("a1", "Agent 1"), Box::new(a1))
            .await;
        registry
            .register(SubagentDefinition::new("a2", "Agent 2"), Box::new(a2))
            .await;

        let available = registry.list_available().await;
        assert_eq!(available.len(), 2);
    }

    #[tokio::test]
    async fn test_list_by_tag() {
        let registry = SubagentRegistry::new();

        let mut def1 = SubagentDefinition::new("researcher", "Research");
        def1.tags.push("research".into());
        let mut def2 = SubagentDefinition::new("writer", "Write");
        def2.tags.push("writing".into());

        registry
            .register(def1, Box::new(MockAgent::new("researcher")))
            .await;
        registry
            .register(def2, Box::new(MockAgent::new("writer")))
            .await;

        let research = registry.list_by_tag("research").await;
        assert_eq!(research.len(), 1);
        assert_eq!(research[0].name, "researcher");
    }

    #[tokio::test]
    async fn test_get_agent() {
        let registry = SubagentRegistry::new();
        let agent = MockAgent::new("a");
        registry
            .register(SubagentDefinition::new("a", "A"), Box::new(agent))
            .await;

        let handle = registry.get_agent("a").await;
        assert!(handle.is_some());
    }

    #[tokio::test]
    async fn test_agent_names() {
        let registry = SubagentRegistry::new();
        assert!(registry.agent_names().await.is_empty());

        registry
            .register(
                SubagentDefinition::new("x", "X"),
                Box::new(MockAgent::new("x")),
            )
            .await;

        let names = registry.agent_names().await;
        assert_eq!(names, vec!["x"]);
    }

    #[tokio::test]
    async fn test_register_definition_only_is_not_advertised_as_executable() {
        let registry = SubagentRegistry::new();
        let def = SubagentDefinition::new("plugin_agent", "Plugin-defined agent");

        let inserted = registry.register_definition(def).await;
        assert!(inserted);

        assert!(registry.contains("plugin_agent").await);
        let available = registry.list_available().await;
        assert!(!available.iter().any(|d| d.name == "plugin_agent"));
        assert!(registry.agent_names().await.is_empty());

        // No instance: get_agent must return None (no factory to invoke).
        let registered = registry.get("plugin_agent").await.unwrap();
        assert!(!registered.has_instance);
        assert!(registry.get_agent("plugin_agent").await.is_none());
    }

    #[tokio::test]
    async fn test_register_definition_then_promote_to_instance() {
        // A later register() under the same name supplies the instance,
        // overwriting the definition — this is how the application layer
        // hydrates a plugin-discovered definition with a real agent.
        let registry = SubagentRegistry::new();
        registry
            .register_definition(SubagentDefinition::new("hydrated", "H"))
            .await;
        assert!(!registry.get("hydrated").await.unwrap().has_instance);

        registry
            .register(
                SubagentDefinition::new("hydrated", "Hydrated"),
                Box::new(MockAgent::new("hydrated")),
            )
            .await;

        let registered = registry.get("hydrated").await.unwrap();
        assert!(registered.has_instance);
        assert_eq!(registered.definition.description, "Hydrated");
    }

    #[test]
    fn test_register_definition_sync_inserts_under_uncontended_locks() {
        let registry = SubagentRegistry::new();
        let inserted = registry.register_definition_sync(SubagentDefinition::new("sync", "S"));
        assert!(inserted);
        let state = registry.state.try_read().ok();
        assert!(
            state
                .as_ref()
                .is_some_and(|state| state.entries.contains_key("sync"))
        );
    }

    #[tokio::test]
    async fn test_factory_instantiation() {
        let registry = SubagentRegistry::new();

        let factory = Arc::new(FnAgentFactory::new(|| {
            Box::pin(async {
                Ok(
                    Box::new(MockAgent::new("lazy_agent").with_response("lazy result"))
                        as Box<dyn Agent>,
                )
            })
        }));

        let def = SubagentDefinition::new("lazy_agent", "Lazy agent");
        registry.register_factory(def, factory).await;

        // Should be registered but not yet instantiated
        let registered = registry.get("lazy_agent").await.unwrap();
        assert_eq!(registered.definition.name, "lazy_agent");
        assert!(!registered.has_instance);

        // get_agent should trigger factory instantiation
        let handle = registry.get_agent("lazy_agent").await;
        assert!(handle.is_some());

        // Now it should show as having an instance
        let registered = registry.get("lazy_agent").await.unwrap();
        assert!(registered.has_instance);

        // Verify the agent actually works
        let agent = handle.unwrap();
        let agent = agent.as_ref();
        let result = agent.execute("test").await.unwrap();
        assert_eq!(result, "lazy result");
    }

    #[tokio::test]
    async fn test_from_subagent_map() {
        use crate::agent::SubagentMap;

        let map: SubagentMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        {
            let mut m = map.write().unwrap_or_else(|e| e.into_inner());
            m.insert(
                "migrated".to_string(),
                Arc::new(MockAgent::new("migrated")) as Arc<dyn Agent>,
            );
        }

        let registry = SubagentRegistry::from_subagent_map(map);
        assert!(registry.contains("migrated").await);

        let handle = registry.get_agent("migrated").await;
        assert!(handle.is_some());
    }
}
