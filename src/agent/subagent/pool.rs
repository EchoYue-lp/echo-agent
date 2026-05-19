//! SubAgentPool — reusable pool of lightweight sub-agents.
//!
//! Avoids repeated construction overhead by keeping warm sub-agent instances
//! that are reset between uses.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::lightweight::{LightweightConfig, LightweightSubagent};
use echo_core::error::Result;

/// Factory type for creating lightweight sub-agents on demand.
pub type LightweightFactory =
    Arc<dyn Fn(LightweightConfig) -> Result<LightweightSubagent> + Send + Sync>;

/// A pool of reusable lightweight sub-agents.
///
/// Sub-agents are kept warm between uses; their message history is reset
/// on each reuse. The pool grows up to `max_idle` entries per name.
pub struct SubAgentPool {
    /// Pooled agents by name.
    agents: RwLock<HashMap<String, Vec<Arc<LightweightSubagent>>>>,
    /// Factory for creating new agents when the pool is empty.
    factory: LightweightFactory,
    /// Maximum number of idle agents per name.
    max_idle: usize,
}

impl SubAgentPool {
    /// Create a new pool with the given factory and max idle count.
    pub fn new(factory: LightweightFactory, max_idle: usize) -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
            factory,
            max_idle,
        }
    }

    /// Get or create a lightweight sub-agent by config.
    ///
    /// If a pooled agent is available, it is reset and returned.
    /// Otherwise, a new agent is created via the factory.
    pub async fn acquire(&self, config: LightweightConfig) -> Result<Arc<LightweightSubagent>> {
        let name = config.name.clone();

        // Try to get from pool first
        {
            let mut agents = self.agents.write().await;
            if let Some(pool) = agents.get_mut(&name) {
                if let Some(agent) = pool.pop() {
                    agent.reset().await;
                    return Ok(agent);
                }
            }
        }

        // Create new via factory
        let agent = (self.factory)(config)?;
        Ok(Arc::new(agent))
    }

    /// Return a sub-agent to the pool for reuse.
    pub async fn release(&self, agent: Arc<LightweightSubagent>) {
        let name = agent.config.name.clone();
        let mut agents = self.agents.write().await;
        let pool = agents.entry(name).or_insert_with(Vec::new);
        if pool.len() < self.max_idle {
            agent.reset().await;
            pool.push(agent);
        }
        // If pool is full, agent is dropped (Arc::drop)
    }

    /// Evict all pooled agents for a given name.
    pub async fn evict(&self, name: &str) {
        let mut agents = self.agents.write().await;
        agents.remove(name);
    }

    /// Clear the entire pool.
    pub async fn clear(&self) {
        self.agents.write().await.clear();
    }

    /// Number of idle agents in the pool.
    pub async fn idle_count(&self) -> usize {
        self.agents
            .read()
            .await
            .values()
            .map(|pool| pool.len())
            .sum()
    }

    /// Number of distinct agent names in the pool.
    pub async fn name_count(&self) -> usize {
        self.agents.read().await.len()
    }
}

impl std::fmt::Debug for SubAgentPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubAgentPool")
            .field("max_idle", &self.max_idle)
            .finish_non_exhaustive()
    }
}
