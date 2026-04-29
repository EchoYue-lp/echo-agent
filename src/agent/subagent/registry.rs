//! Subagent registry — discovery, registration, and lifecycle management
//!
//! Wraps the existing `SubAgentMap` with declarative definitions, factory support,
//! and lifecycle events. Backward compatible — `register_agent()` still works.

use crate::agent::SubAgentMap;
use crate::error::Result;
use echo_core::agent::Agent;
use futures::future::BoxFuture;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, Notify, RwLock};
use tracing::{debug, info, warn};

use super::events::SubagentEventBus;
use super::types::{RegisteredSubagent, SubagentDefinition};

type AgentMap = Arc<RwLock<HashMap<String, Arc<AsyncMutex<Box<dyn Agent>>>>>>;

// ── Agent Factory ─────────────────────────────────────────────────────────────

/// Factory trait for lazy agent instantiation.
///
/// Used when you want to register an agent definition but defer
/// the actual agent construction until it's first dispatched.
pub trait AgentFactory: Send + Sync {
    /// Create an agent instance asynchronously.
    ///
    /// # 返回
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
    /// # 参数
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
/// Wraps the existing `SubAgentMap` and adds:
/// - Definition-based lookup
/// - Factory support for lazy instantiation
/// - Lifecycle events
pub struct SubagentRegistry {
    /// Agent instances (compatible with existing SubAgentMap).
    agents: AgentMap,
    /// Definitions for each registered agent.
    definitions: Arc<RwLock<HashMap<String, SubagentDefinition>>>,
    /// Factory functions for lazy instantiation.
    factories: Arc<RwLock<HashMap<String, Arc<dyn AgentFactory>>>>,
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
            agents: AgentMap::new(RwLock::new(HashMap::new())),
            definitions: Arc::new(RwLock::new(HashMap::new())),
            factories: Arc::new(RwLock::new(HashMap::new())),
            instantiating: Arc::new(RwLock::new(HashSet::new())),
            instantiating_done: Arc::new(Notify::new()),
            event_bus: SubagentEventBus::new(),
        }
    }

    /// Create with a specific event bus.
    pub fn with_event_bus(event_bus: SubagentEventBus) -> Self {
        Self {
            agents: AgentMap::new(RwLock::new(HashMap::new())),
            definitions: Arc::new(RwLock::new(HashMap::new())),
            factories: Arc::new(RwLock::new(HashMap::new())),
            instantiating: Arc::new(RwLock::new(HashSet::new())),
            instantiating_done: Arc::new(Notify::new()),
            event_bus,
        }
    }

    /// Migrate from an existing `SubAgentMap` (backward compatible).
    ///
    /// Each agent gets a default Sync-mode `BuiltIn` definition.
    pub fn from_subagent_map(map: SubAgentMap) -> Self {
        let registry = Self::new();
        if let Ok(agents) = map.read() {
            for (name, agent) in agents.iter() {
                let def = SubagentDefinition::simple_sync(name.clone());
                // We can't do async writes here, so we use blocking inserts
                // into the Arc<RwLock> maps. Since we just created the registry,
                // there are no other references yet.
                let agents_map = registry.agents.clone();
                let definitions_map = registry.definitions.clone();
                if let Ok(mut a) = agents_map.try_write() {
                    a.insert(name.clone(), agent.clone());
                }
                if let Ok(mut d) = definitions_map.try_write() {
                    d.insert(name.clone(), def);
                }
            }
        }
        registry
    }

    // ── Registration ──────────────────────────────────────────────────────

    /// Register a pre-built agent with its definition.
    pub async fn register(&self, def: SubagentDefinition, agent: Box<dyn Agent>) {
        let name = def.name.clone();
        info!(subagent = %name, mode = %def.execution_mode, "Registering subagent");

        let arc_agent = Arc::new(AsyncMutex::new(agent));
        {
            let mut agents = self.agents.write().await;
            agents.insert(name.clone(), arc_agent);
        }
        {
            let mut defs = self.definitions.write().await;
            defs.insert(name.clone(), def);
        }

        self.event_bus
            .emit(super::events::SubagentEvent::Registered { name: name.clone() });
    }

    /// Sync registration — uses `try_write` to avoid `block_on` deadlock.
    ///
    /// Use this from synchronous contexts (e.g., builder pattern, `main()`).
    /// Falls back to logging a warning if locks are contended.
    pub fn register_sync(&self, def: SubagentDefinition, agent: Box<dyn Agent>) -> bool {
        let name = def.name.clone();
        let arc_agent = Arc::new(AsyncMutex::new(agent));

        let ok = match self.agents.try_write() {
            Ok(mut agents) => {
                agents.insert(name.clone(), arc_agent);
                true
            }
            Err(_) => {
                warn!(subagent = %name, "Lock contention on agents map, registration deferred");
                false
            }
        };

        if ok {
            if let Ok(mut defs) = self.definitions.try_write() {
                defs.insert(name.clone(), def);
            } else {
                warn!(subagent = %name, "Lock contention on definitions map");
            }
            self.event_bus
                .emit(super::events::SubagentEvent::Registered { name });
        }

        ok
    }

    /// Register a definition with a factory for lazy instantiation.
    pub async fn register_factory(&self, def: SubagentDefinition, factory: Arc<dyn AgentFactory>) {
        let name = def.name.clone();
        debug!(subagent = %name, "Registering subagent factory");

        {
            let mut defs = self.definitions.write().await;
            defs.insert(name.clone(), def);
        }
        {
            let mut facts = self.factories.write().await;
            facts.insert(name.clone(), factory);
        }
    }

    /// Remove a subagent by name.
    pub async fn remove(&self, name: &str) {
        {
            let mut agents = self.agents.write().await;
            agents.remove(name);
        }
        {
            let mut defs = self.definitions.write().await;
            defs.remove(name);
        }
        {
            let mut facts = self.factories.write().await;
            facts.remove(name);
        }

        self.event_bus
            .emit(super::events::SubagentEvent::Unregistered {
                name: name.to_string(),
            });
    }

    // ── Lookup ────────────────────────────────────────────────────────────

    /// Look up a registered subagent. Returns None if not found.
    pub async fn get(&self, name: &str) -> Option<RegisteredSubagent> {
        let agents = self.agents.read().await;
        let defs = self.definitions.read().await;

        let definition = defs.get(name).cloned()?;
        let has_instance = agents.contains_key(name);

        Some(RegisteredSubagent {
            definition,
            has_instance,
        })
    }

    /// Get the agent instance for immediate execution.
    ///
    /// If the agent was registered via factory and not yet instantiated,
    /// this will create it on demand.
    ///
    /// Uses a loop with timeout to handle concurrent instantiation attempts
    /// rather than relying on a single `notified()` call.
    pub async fn get_agent(&self, name: &str) -> Option<Arc<AsyncMutex<Box<dyn Agent>>>> {
        use std::time::Duration;

        // Check if already instantiated
        {
            let agents = self.agents.read().await;
            if let Some(agent) = agents.get(name) {
                return Some(agent.clone());
            }
        }

        // Try factory
        let factory_arc = {
            let factories = self.factories.read().await;
            factories.get(name).cloned()
        };

        if let Some(factory) = factory_arc {
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
                            let agents = self.agents.read().await;
                            if let Some(agent) = agents.get(name) {
                                return Some(agent.clone());
                            }
                        }
                        // Check if instantiation failed (removed from instantiating but not in agents)
                        {
                            let in_progress = self.instantiating.read().await;
                            if !in_progress.contains(name) {
                                // Instantiation finished (success or failure), re-check agents
                                let agents = self.agents.read().await;
                                return agents.get(name).cloned();
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
                    let arc_agent = Arc::new(AsyncMutex::new(agent));
                    let mut agents = self.agents.write().await;
                    agents.insert(name.to_string(), arc_agent.clone());
                    // Remove factory after successful instantiation
                    drop(agents);
                    let mut facts = self.factories.write().await;
                    facts.remove(name);
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

    /// Check if a subagent is registered.
    pub async fn contains(&self, name: &str) -> bool {
        let defs = self.definitions.read().await;
        defs.contains_key(name)
    }

    /// List all available subagent definitions.
    pub async fn list_available(&self) -> Vec<SubagentDefinition> {
        let defs = self.definitions.read().await;
        defs.values().cloned().collect()
    }

    /// List subagent definitions matching a tag.
    pub async fn list_by_tag(&self, tag: &str) -> Vec<SubagentDefinition> {
        let defs = self.definitions.read().await;
        defs.values()
            .filter(|d| d.tags.iter().any(|t| t == tag))
            .cloned()
            .collect()
    }

    /// Get agent names for tool description (convenience).
    pub async fn agent_names(&self) -> Vec<String> {
        let defs = self.definitions.read().await;
        defs.keys().cloned().collect()
    }

    /// Get the event bus reference.
    pub fn event_bus(&self) -> &SubagentEventBus {
        &self.event_bus
    }

    /// Get the underlying agents map (for backward compat).
    pub fn agents_map(&self) -> AgentMap {
        self.agents.clone()
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
            agents: self.agents.clone(),
            definitions: self.definitions.clone(),
            factories: self.factories.clone(),
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
    async fn test_remove() {
        let registry = SubagentRegistry::new();
        let agent = MockAgent::new("worker");
        let def = SubagentDefinition::new("worker", "Worker agent");

        registry.register(def, Box::new(agent)).await;
        assert!(registry.contains("worker").await);

        registry.remove("worker").await;
        assert!(!registry.contains("worker").await);
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
        let agent = agent.lock().await;
        let result = agent.execute("test").await.unwrap();
        assert_eq!(result, "lazy result");
    }

    #[tokio::test]
    async fn test_from_subagent_map() {
        use crate::agent::SubAgentMap;

        let map: SubAgentMap = Arc::new(std::sync::RwLock::new(HashMap::new()));
        {
            let mut m = map.write().unwrap();
            m.insert(
                "migrated".to_string(),
                Arc::new(AsyncMutex::new(
                    Box::new(MockAgent::new("migrated")) as Box<dyn Agent>
                )),
            );
        }

        let registry = SubagentRegistry::from_subagent_map(map);
        assert!(registry.contains("migrated").await);

        let handle = registry.get_agent("migrated").await;
        assert!(handle.is_some());
    }
}
