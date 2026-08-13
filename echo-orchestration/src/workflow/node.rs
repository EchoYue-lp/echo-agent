//! Graph workflow nodes
//!
//! Each node is an execution unit in the graph, which can be:
//! - **Agent node**: calls `Agent::execute()` and writes the result to state
//! - **Function node**: arbitrary `async fn(SharedState) -> Result<()>`
//! - **Router node**: pure routing (no execution, conditional branching only)

use super::state::SharedState;
use echo_core::agent::Agent;
use echo_core::error::Result;
use futures::future::BoxFuture;
use std::sync::Arc;

// ── NodeAction ──────────────────────────────────────────────────────────────

/// Type-safe wrapper for node execution logic
pub(crate) enum NodeAction {
    /// Agent execution: reads input_key from state as prompt, writes output to output_key
    Agent {
        agent: Arc<dyn Agent>,
        input_key: String,
        output_key: String,
        /// Whether to use execute (multi-turn with tools) or chat (single turn)
        use_execute: bool,
    },
    /// Nested subgraph execution — the node itself is a complete compiled Graph
    Subgraph(Box<super::graph::Graph>),
    /// Custom async function
    Function(Box<dyn NodeFn>),
    /// No-op (used for router nodes)
    Passthrough,
}

/// Custom node function trait (object-safe)
pub(crate) trait NodeFn: Send + Sync {
    fn call<'a>(&'a self, state: &'a SharedState) -> BoxFuture<'a, Result<()>>;
}

/// Implements NodeFn using a closure
struct FnWrapper<F>(F);

impl<F> NodeFn for FnWrapper<F>
where
    F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, Result<()>> + Send + Sync,
{
    fn call<'a>(&'a self, state: &'a SharedState) -> BoxFuture<'a, Result<()>> {
        (self.0)(state)
    }
}

// ── Node ────────────────────────────────────────────────────────────────────

/// Node definition in the graph
pub(crate) struct Node {
    /// Execution logic
    pub action: NodeAction,
}

impl Node {
    /// Create an Agent node (defaults to execute, i.e., multi-turn with tools)
    pub fn agent(
        _name: impl Into<String>,
        agent: impl Agent + 'static,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
    ) -> Self {
        Self {
            action: NodeAction::Agent {
                agent: Arc::new(agent),
                input_key: input_key.into(),
                output_key: output_key.into(),
                use_execute: true,
            },
        }
    }

    /// Create an Agent node (configurable execute/chat mode)
    pub fn agent_with_mode(
        _name: impl Into<String>,
        agent: impl Agent + 'static,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
        use_execute: bool,
    ) -> Self {
        Self {
            action: NodeAction::Agent {
                agent: Arc::new(agent),
                input_key: input_key.into(),
                output_key: output_key.into(),
                use_execute,
            },
        }
    }

    /// Create an Agent node (pre-wrapped as Arc<dyn Agent>)
    pub fn agent_shared(
        _name: impl Into<String>,
        agent: Arc<dyn Agent>,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
    ) -> Self {
        Self {
            action: NodeAction::Agent {
                agent,
                input_key: input_key.into(),
                output_key: output_key.into(),
                use_execute: true,
            },
        }
    }

    /// Create an Agent node (pre-wrapped + configurable execute/chat)
    pub fn agent_shared_with_mode(
        _name: impl Into<String>,
        agent: Arc<dyn Agent>,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
        use_execute: bool,
    ) -> Self {
        Self {
            action: NodeAction::Agent {
                agent,
                input_key: input_key.into(),
                output_key: output_key.into(),
                use_execute,
            },
        }
    }

    /// Create a function node
    pub fn function<F>(_name: impl Into<String>, f: F) -> Self
    where
        F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, Result<()>> + Send + Sync + 'static,
    {
        Self {
            action: NodeAction::Function(Box::new(FnWrapper(f))),
        }
    }

    /// Create a passthrough (router) node
    pub fn passthrough(_name: impl Into<String>) -> Self {
        Self {
            action: NodeAction::Passthrough,
        }
    }

    /// Create a subgraph node — embeds a compiled Graph as a node.
    ///
    /// The subgraph shares the parent's `SharedState` and executes within
    /// the parent's step count limit.
    pub fn subgraph(_name: impl Into<String>, graph: super::graph::Graph) -> Self {
        Self {
            action: NodeAction::Subgraph(Box::new(graph)),
        }
    }

    /// Execute the node
    pub async fn execute(&self, state: &SharedState) -> Result<()> {
        match &self.action {
            NodeAction::Agent {
                agent,
                input_key,
                output_key,
                use_execute,
            } => {
                let input = state.get::<String>(input_key).unwrap_or_default();

                let agent = agent.as_ref();
                let output = if *use_execute {
                    agent.execute(&input).await?
                } else {
                    agent.chat(&input).await?
                };

                // Use merge_overwrite to support structural data merge semantics
                // rather than simple key-level overwrite
                state.merge_overwrite(&SharedState::from_values(
                    [(
                        output_key.to_string(),
                        serde_json::Value::String(output.clone()),
                    )]
                    .into_iter()
                    .collect(),
                ))?;
                // Also append to message history
                state.push_message(echo_core::llm::types::Message::assistant(output))?;
                Ok(())
            }
            NodeAction::Subgraph(subgraph) => {
                // Box::pin to break recursive async call chain
                let state_clone = state.clone();
                let result = Box::pin(subgraph.run(state_clone)).await?;
                let _ = state.deep_merge(&result.state);
                Ok(())
            }
            NodeAction::Function(f) => f.call(state).await,
            NodeAction::Passthrough => Ok(()),
        }
    }
}

// ── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_function_node() {
        let node = Node::function("double", |state: &SharedState| {
            Box::pin(async move {
                let x: i64 = state.get("input").unwrap_or(0);
                let _ = state.set("output", x * 2);
                Ok(())
            })
        });

        let state = SharedState::new();
        let _ = state.set("input", 21i64);
        node.execute(&state).await.unwrap();
        assert_eq!(state.get::<i64>("output"), Some(42));
    }

    #[tokio::test]
    async fn test_passthrough_node() {
        let node = Node::passthrough("noop");
        let state = SharedState::new();
        let _ = state.set("x", 1);
        node.execute(&state).await.unwrap();
        assert_eq!(state.get::<i64>("x"), Some(1)); // unchanged
    }
}
