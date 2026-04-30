//! High-level declarative DSL — StateGraph
//!
//! Provides a LangGraph-style chain API for declaratively building multi-Agent topologies.
//!
//! # Example
//!
//! ```rust,no_run
//! use echo_agent::workflow::dsl::StateGraph;
//! use echo_agent::prelude::*;
//!
//! # async fn example() -> echo_agent::error::Result<()> {
//! let mut sg = StateGraph::new("research_flow");
//! sg.add_react_node("researcher", |b| {
//!         b.model("qwen3-max").system_prompt("Researcher").enable_tools()
//!     })
//!     .with_input("topic").with_output("research");
//! sg.add_react_node("writer", |b| {
//!         b.model("qwen3-max").system_prompt("Writer")
//!     })
//!     .with_input("research").with_output("draft");
//! sg.add_function_node("checker", |state| Box::pin(async move {
//!         let draft: String = state.get("draft").unwrap_or_default();
//!         state.set("review", if draft.len() > 100 { "pass" } else { "fail" });
//!         Ok(())
//!     }));
//! sg.add_conditional_edge("checker", |state| Box::pin(async {
//!         let r: String = state.get("review").unwrap_or_default();
//!         if r == "pass" { "done" } else { "writer" }.to_string()
//!     }));
//! sg.add_react_node("done", |b| {
//!         b.model("qwen3-max").system_prompt("Polisher")
//!     })
//!     .with_input("draft").with_output("final");
//! sg.entry("researcher").finish("done");
//! let graph = sg.compile()?;
//! # Ok(())
//! # }
//! ```

use crate::agent::react::builder::ReactAgentBuilder;
use crate::error::Result;
use crate::workflow::Graph;
use crate::workflow::GraphBuilder;
use crate::workflow::SharedState;
use futures::future::BoxFuture;

// ── Pending Node Definition ──────────────────────────────────────────────────

type AgentConfigFn = Box<dyn FnOnce(ReactAgentBuilder) -> ReactAgentBuilder>;
type FunctionNodeFn =
    Box<dyn for<'a> Fn(&'a SharedState) -> BoxFuture<'a, Result<()>> + Send + Sync>;
type ConditionFn = Box<dyn for<'a> Fn(&'a SharedState) -> BoxFuture<'a, String> + Send + Sync>;

enum PendingNode {
    Agent {
        name: String,
        config_fn: AgentConfigFn,
        input_key: String,
        output_key: String,
    },
    Function {
        name: String,
        f: FunctionNodeFn,
    },
    Router {
        name: String,
    },
}

enum PendingEdge {
    Fixed {
        from: String,
        to: String,
    },
    Conditional {
        from: String,
        f: ConditionFn,
    },
    Parallel {
        from: String,
        targets: Vec<String>,
        then: String,
    },
}

// ── StateGraph ───────────────────────────────────────────────────────────

/// Declarative Agent topology builder.
///
/// Defines nodes, edges, and state fields through a chain API, finally compiled
/// into a `Graph` via `compile()`.
///
/// All `add_*` and `with_*` methods return `&mut Self`, supporting chain calls.
pub struct StateGraph {
    name: String,
    pending_nodes: Vec<PendingNode>,
    edges: Vec<PendingEdge>,
    entry: Option<String>,
    finish_nodes: Vec<String>,
}

impl StateGraph {
    /// Create a new StateGraph
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            pending_nodes: Vec::new(),
            edges: Vec::new(),
            entry: None,
            finish_nodes: Vec::new(),
        }
    }

    /// Add a ReactAgent node.
    ///
    /// The `configure` closure receives a `ReactAgentBuilder` and returns the
    /// configured builder. Default input="task", output="result".
    pub fn add_react_node<F>(&mut self, name: impl Into<String>, configure: F) -> &mut Self
    where
        F: FnOnce(ReactAgentBuilder) -> ReactAgentBuilder + 'static,
    {
        self.pending_nodes.push(PendingNode::Agent {
            name: name.into(),
            config_fn: Box::new(configure),
            input_key: "task".to_string(),
            output_key: "result".to_string(),
        });
        self
    }

    /// Set the input key of the last agent node
    pub fn with_input(&mut self, key: impl Into<String>) -> &mut Self {
        if let Some(PendingNode::Agent { input_key, .. }) = self.pending_nodes.last_mut() {
            *input_key = key.into();
        }
        self
    }

    /// Set the output key of the last agent node
    pub fn with_output(&mut self, key: impl Into<String>) -> &mut Self {
        if let Some(PendingNode::Agent { output_key, .. }) = self.pending_nodes.last_mut() {
            *output_key = key.into();
        }
        self
    }

    /// Add a function node
    pub fn add_function_node<F>(&mut self, name: impl Into<String>, f: F) -> &mut Self
    where
        F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, Result<()>> + Send + Sync + 'static,
    {
        self.pending_nodes.push(PendingNode::Function {
            name: name.into(),
            f: Box::new(f),
        });
        self
    }

    /// Add a router node
    pub fn add_router(&mut self, name: impl Into<String>) -> &mut Self {
        self.pending_nodes
            .push(PendingNode::Router { name: name.into() });
        self
    }

    /// Add a fixed edge: from → to
    pub fn add_edge(&mut self, from: impl Into<String>, to: impl Into<String>) -> &mut Self {
        self.edges.push(PendingEdge::Fixed {
            from: from.into(),
            to: to.into(),
        });
        self
    }

    /// Add a conditional edge
    pub fn add_conditional_edge<F>(&mut self, from: impl Into<String>, f: F) -> &mut Self
    where
        F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, String> + Send + Sync + 'static,
    {
        self.edges.push(PendingEdge::Conditional {
            from: from.into(),
            f: Box::new(f),
        });
        self
    }

    /// Add parallel edges: from → [targets...] → then
    pub fn add_parallel_edge(
        &mut self,
        from: impl Into<String>,
        targets: Vec<String>,
        then: impl Into<String>,
    ) -> &mut Self {
        self.edges.push(PendingEdge::Parallel {
            from: from.into(),
            targets,
            then: then.into(),
        });
        self
    }

    /// Set the entry node
    pub fn entry(&mut self, name: impl Into<String>) -> &mut Self {
        self.entry = Some(name.into());
        self
    }

    /// Add a finish node
    pub fn finish(&mut self, name: impl Into<String>) -> &mut Self {
        self.finish_nodes.push(name.into());
        self
    }

    /// Compile into a Graph
    ///
    /// Iterate over all pending nodes, build agents, add edges and entry/finish nodes, then build.
    pub fn compile(self) -> Result<Graph> {
        let mut builder = GraphBuilder::new(&self.name);

        // Build all pending nodes
        for node in self.pending_nodes {
            match node {
                PendingNode::Agent {
                    name,
                    config_fn,
                    input_key,
                    output_key,
                } => {
                    let base = ReactAgentBuilder::new();
                    let configured = config_fn(base);
                    let agent = configured.build()?;
                    builder = builder.add_agent_node(&name, agent, &input_key, &output_key);
                }
                PendingNode::Function { name, f } => {
                    builder = builder.add_function_node(&name, f);
                }
                PendingNode::Router { name } => {
                    builder = builder.add_router_node(&name);
                }
            }
        }

        // Add all edges
        for edge in self.edges {
            match edge {
                PendingEdge::Fixed { from, to } => {
                    builder = builder.add_edge(&from, &to);
                }
                PendingEdge::Conditional { from, f } => {
                    builder = builder.add_conditional_edge(&from, f);
                }
                PendingEdge::Parallel {
                    from,
                    targets,
                    then,
                } => {
                    builder = builder.add_parallel_edge(&from, targets, &then);
                }
            }
        }

        // Set entry and finish
        if let Some(entry) = self.entry {
            builder = builder.set_entry(&entry);
        }
        for finish in self.finish_nodes {
            builder = builder.set_finish(&finish);
        }

        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_state_graph_function_nodes() {
        let mut sg = StateGraph::new("test_flow");
        sg.add_function_node("step1", |state| {
            Box::pin(async move {
                let _ = state.set("x", "hello");
                Ok(())
            })
        })
        .add_function_node("step2", |state| {
            Box::pin(async move {
                let x: String = state.get("x").unwrap_or_default();
                let _ = state.set("y", format!("{} world", x));
                Ok(())
            })
        })
        .add_edge("step1", "step2")
        .add_edge("step2", "__end__")
        .entry("step1")
        .finish("step2");

        let graph = sg.compile().unwrap();
        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        let y: String = result.state.get("y").unwrap_or_default();
        assert_eq!(y, "hello world");
    }

    #[tokio::test]
    async fn test_state_graph_run() {
        let mut sg = StateGraph::new("hello_flow");
        sg.add_function_node("greet", |state| {
            Box::pin(async move {
                let _ = state.set("msg", "hello");
                Ok(())
            })
        })
        .add_function_node("shout", |state| {
            Box::pin(async move {
                let msg: String = state.get("msg").unwrap_or_default();
                let _ = state.set("msg", format!("{}!", msg.to_uppercase()));
                Ok(())
            })
        })
        .add_edge("greet", "shout")
        .add_edge("shout", "__end__")
        .entry("greet")
        .finish("shout");

        let graph = sg.compile().unwrap();
        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        let msg: String = result.state.get("msg").unwrap_or_default();
        assert_eq!(msg, "HELLO!");
    }

    #[tokio::test]
    async fn test_state_graph_conditional() {
        let mut sg = StateGraph::new("cond_flow");
        sg.add_function_node("check", |state| {
            Box::pin(async move {
                let _ = state.set("status", "ok");
                Ok(())
            })
        })
        .add_function_node("pass", |state| {
            Box::pin(async move {
                let _ = state.set("result", "passed");
                Ok(())
            })
        })
        .add_function_node("fail", |state| {
            Box::pin(async move {
                let _ = state.set("result", "failed");
                Ok(())
            })
        })
        .add_conditional_edge("check", |state| {
            Box::pin(async {
                let s: String = state.get("status").unwrap_or_default();
                if s == "ok" { "pass" } else { "fail" }.to_string()
            })
        })
        .add_edge("pass", "__end__")
        .add_edge("fail", "__end__")
        .entry("check")
        .finish("pass")
        .finish("fail");

        let graph = sg.compile().unwrap();
        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        assert_eq!(result.steps, 2); // check + pass
    }
}
