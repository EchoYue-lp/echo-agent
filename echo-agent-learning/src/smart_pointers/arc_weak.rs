//! `Arc<T>` for thread-safe sharing and `Weak<T>` for non-owning references.

use crate::errors::LearningError;
use std::collections::HashMap;
use std::sync::{Arc, RwLock, Weak};

#[derive(Debug, PartialEq, Eq)]
pub struct AgentHandle {
    pub name: String,
}

#[derive(Debug, Default)]
pub struct AgentRegistry {
    agents: RwLock<HashMap<String, Weak<AgentHandle>>>,
}

impl AgentRegistry {
    pub fn register(&self, agent: &Arc<AgentHandle>) -> Result<(), LearningError> {
        self.agents
            .write()
            .map_err(|_| LearningError::PoisonedLock)?
            .insert(agent.name.clone(), Arc::downgrade(agent));
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<Option<Arc<AgentHandle>>, LearningError> {
        Ok(self
            .agents
            .read()
            .map_err(|_| LearningError::PoisonedLock)?
            .get(name)
            .and_then(Weak::upgrade))
    }

    pub fn remove_expired(&self) -> Result<usize, LearningError> {
        let mut agents = self
            .agents
            .write()
            .map_err(|_| LearningError::PoisonedLock)?;
        let before = agents.len();
        agents.retain(|_, agent| agent.strong_count() > 0);
        Ok(before.saturating_sub(agents.len()))
    }

    pub fn counts(agent: &Arc<AgentHandle>) -> (usize, usize) {
        (Arc::strong_count(agent), Arc::weak_count(agent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weak_registry_does_not_keep_agents_alive() -> Result<(), LearningError> {
        let registry = AgentRegistry::default();
        let agent = Arc::new(AgentHandle {
            name: "reviewer".to_string(),
        });
        registry.register(&agent)?;
        assert_eq!(AgentRegistry::counts(&agent), (1, 1));
        assert!(registry.get("reviewer")?.is_some());
        drop(agent);
        assert!(registry.get("reviewer")?.is_none());
        assert_eq!(registry.remove_expired()?, 1);
        Ok(())
    }
}
