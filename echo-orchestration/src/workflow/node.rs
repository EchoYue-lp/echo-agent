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
        control: Arc<super::graph::GraphExecutionControl>,
        node_name: &'a str,
        execution_id: &'a str,
    ) -> BoxStream<'a, Result<String>> {
        let stream = async_stream::try_stream! {
            match &self.action {
                NodeAction::Agent { agent, input_key, output_key, use_execute } => {
                    let input = state.get::<String>(input_key).unwrap_or_default();
                    let run_id = control.run_id().ok_or_else(|| ReactError::Other("Workflow invocation has no bound run identity".into()))?;
                    let invocation = echo_core::agent::AgentInvocationContext {
                        runtime: Some(echo_core::tools::ExternalRunContext {
                            run_id: Some(run_id.clone()),
                            turn_id: Some(control.operation_id.clone()),
                            execution_id: Some(execution_id.to_string()),
                            cancel: Some(Arc::new(cancel.clone())),
                            ..Default::default()
                        }),
                        cancel: Some(cancel.clone()),
                        ..Default::default()
                    };
                    let raw = if *use_execute {
                        agent.execute_stream_with_invocation_context(&input, cancel.clone(), invocation).await
                    } else {
                        agent.chat_stream_message_with_invocation_context(echo_core::llm::types::Message::user(input), cancel.clone(), invocation).await
                    };
                    let mut events = match raw {
                        Ok(events) => events.fuse(),
                        Err(error @ ReactError::ExecutionNotSettled { .. }) => { Err(error)?; return; }
                        Err(error) => { control.record_error(Some(node_name), error); return; }
                    };
                    let mut final_answer = None;
                    let mut observation = None;
                    while let Some(event) = events.next().await {
                        match event {
                            Err(error @ ReactError::ExecutionNotSettled { .. }) => {
                                // A foreign observation cannot be overwritten by
                                // matching local progress on this source.
                                let foreign_pending = observation.as_ref().is_some_and(|error: &ReactError| !matches!(error, ReactError::ExecutionNotSettled { run_id: observed_run, execution_id: Some(observed_execution) } if observed_run == &run_id && observed_execution == execution_id));
                                if !foreign_pending { observation = Some(error); }
                            }
                            Err(error) => control.record_error(Some(node_name), error),
                            Ok(event) => {
                                if event.is_terminal() && observation.as_ref().is_some_and(|error| matches!(error, ReactError::ExecutionNotSettled { run_id: observed_run, execution_id: Some(observed_execution) } if observed_run == &run_id && observed_execution == execution_id)) {
                                    observation = None;
                                }
                                match event {
                                    AgentEvent::Token(token) if !control.has_error() => yield token,
                                    AgentEvent::FinalAnswer(answer) => if final_answer.is_none() { final_answer = Some(answer); },
                                    AgentEvent::Cancelled => control.record_error(Some(node_name), ReactError::Agent(Box::new(AgentError::Cancelled(format!("Workflow Agent node '{}' cancelled", agent.name()))))),
                                    AgentEvent::Error { message, .. } => control.record_error(Some(node_name), ReactError::Other(message)),
                                    _ => {}
                                }
                            }
                        }
                    }
                    if let Some(error) = observation { Err(error)?; }
                    if !control.has_error() {
                        match final_answer {
                            Some(output) => if let Err(error) = Self::commit_agent_output(state, output_key, output) { control.record_error(Some(node_name), error); },
                            None => control.record_error(Some(node_name), ReactError::Agent(Box::new(AgentError::NoResponse { model: agent.model_name().to_string(), agent: agent.name().to_string() }))),
                        }
                    }
                }
                NodeAction::Subgraph(subgraph) => {
                    match subgraph.run_nested(state.clone(), control.clone()).await? {
                        super::graph::GraphLoopOutcome::Settled(result) => match *result {
                            super::graph::RunUntilInterruptResult::Completed(result) => { state.deep_merge(&result.state)?; }
                            _ => control.record_error(Some(node_name), ReactError::Other("Nested Graph returned an unexpected interrupt".into())),
                        },
                        super::graph::GraphLoopOutcome::CauseRecorded => {}
                    }
                }
                NodeAction::Function(f) => {
                    match f.call(state).await {
                        Err(error @ ReactError::ExecutionNotSettled { .. }) => Err(error)?,
                        Err(error) => control.record_error(Some(node_name), error),
                        Ok(()) => {}
                    }
                }
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
        let control =
            super::super::graph::GraphExecutionControl::new(Some(uuid::Uuid::new_v4().to_string()));
        let execution_id = uuid::Uuid::new_v4().to_string();
        let mut stream = node.execute_stream(
            state,
            CancellationToken::new(),
            control,
            "test-node",
            &execution_id,
        );
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
