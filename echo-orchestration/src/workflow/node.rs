//! Graph workflow nodes
//!
//! Each node is an execution unit in the graph, which can be:
//! - **Agent node**: drains a cancellable Agent stream and writes its final answer to state
//! - **Function node**: arbitrary `async fn(SharedState) -> Result<()>`
//! - **Router node**: pure routing (no execution, conditional branching only)

use super::state::SharedState;
use echo_core::agent::{Agent, AgentEvent};
use echo_core::error::{AgentError, ReactError, Result};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

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
    fn commit_agent_output(state: &SharedState, output_key: &str, output: String) -> Result<()> {
        // Use merge_overwrite to support structural data merge semantics
        // rather than simple key-level overwrite.
        state.merge_overwrite(&SharedState::from_values(
            [(
                output_key.to_string(),
                serde_json::Value::String(output.clone()),
            )]
            .into_iter()
            .collect(),
        ))?;
        state.push_message(echo_core::llm::types::Message::assistant(output))?;
        Ok(())
    }

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

    /// Execute through the cancellable Agent stream and expose token deltas to Graph.
    pub(crate) fn execute_stream<'a>(
        &'a self,
        state: &'a SharedState,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<String>> {
        let stream = async_stream::try_stream! {
            match &self.action {
                NodeAction::Agent {
                    agent,
                    input_key,
                    output_key,
                    use_execute,
                } => {
                    let input = state.get::<String>(input_key).unwrap_or_default();
                    let mut events = if *use_execute {
                        agent.execute_stream_with_cancel(&input, cancel).await?
                    } else {
                        agent.chat_stream_with_cancel(&input, cancel).await?
                    };
                    let mut final_answer = None;
                    while let Some(event) = events.next().await {
                        match event? {
                            AgentEvent::Token(token) => yield token,
                            AgentEvent::FinalAnswer(answer) => final_answer = Some(answer),
                            AgentEvent::Cancelled => {
                                Err(ReactError::Agent(Box::new(AgentError::Cancelled(
                                    format!("Workflow Agent node '{}' cancelled", agent.name()),
                                ))))?;
                            }
                            AgentEvent::Error { message, .. } => {
                                Err(ReactError::Other(message))?;
                            }
                            _ => {}
                        }
                    }
                    let output = final_answer.ok_or_else(|| {
                        ReactError::Agent(Box::new(AgentError::NoResponse {
                            model: agent.model_name().to_string(),
                            agent: agent.name().to_string(),
                        }))
                    })?;
                    Self::commit_agent_output(state, output_key, output)?;
                }
                NodeAction::Subgraph(subgraph) => {
                    let result = Box::pin(subgraph.run(state.clone())).await?;
                    state.deep_merge(&result.state)?;
                }
                NodeAction::Function(f) => f.call(state).await?,
                NodeAction::Passthrough => {}
            }
        };
        Box::pin(stream)
    }
}

// ── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    async fn drain_node(node: &Node, state: &SharedState) -> Result<()> {
        let mut stream = node.execute_stream(state, CancellationToken::new());
        while let Some(token) = stream.next().await {
            let _ = token?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_function_node() -> Result<()> {
        let node = Node::function("double", |state: &SharedState| {
            Box::pin(async move {
                let x: i64 = state.get("input").unwrap_or(0);
                let _ = state.set("output", x * 2);
                Ok(())
            })
        });

        let state = SharedState::new();
        state.set("input", 21i64)?;
        drain_node(&node, &state).await?;
        assert_eq!(state.get::<i64>("output"), Some(42));
        Ok(())
    }

    #[tokio::test]
    async fn test_passthrough_node() -> Result<()> {
        let node = Node::passthrough("noop");
        let state = SharedState::new();
        state.set("x", 1)?;
        drain_node(&node, &state).await?;
        assert_eq!(state.get::<i64>("x"), Some(1)); // unchanged
        Ok(())
    }
}
