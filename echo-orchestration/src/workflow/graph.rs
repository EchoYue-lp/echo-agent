//! Graph workflow engine
//!
//! Models Agent execution as a **directed graph**:
//!
//! - **Node**: execution unit (Agent / function / router)
//! - **Edge**: transition logic between nodes (fixed / conditional / parallel fan-out + fan-in)
//! - **State (SharedState)**: shared KV store + message history across nodes
//!
//! ## Comparison with LangGraph
//!
//! | LangGraph | echo-agent workflow |
//! |-----------|---------------------|
//! | `StateGraph` | [`Graph`] |
//! | `add_node()` | [`GraphBuilder::add_node()`] |
//! | `add_edge()` | [`GraphBuilder::add_edge()`] |
//! | `add_conditional_edges()` | [`GraphBuilder::add_conditional_edge()`] |
//! | `END` | [`Graph::END`] |
//! | `compile()` | [`GraphBuilder::build()`] |
//! | `invoke()` | [`Graph::run()`] |
//!
//! ## Example
//!
//! ```rust,no_run
//! use echo_core::error::Result;
//! use echo_orchestration::workflow::{GraphBuilder, SharedState};
//!
//! # async fn example() -> Result<()> {
//! let graph = GraphBuilder::new("my_workflow")
//!     .add_function_node("start", |state| Box::pin(async move {
//!         let _ = state.set("greeting", "Hello, World!");
//!         Ok(())
//!     }))
//!     .add_function_node("end", |state| Box::pin(async move {
//!         let msg: String = state.get("greeting").unwrap_or_default();
//!         println!("{msg}");
//!         Ok(())
//!     }))
//!     .set_entry("start")
//!     .add_edge("start", "end")
//!     .set_finish("end")
//!     .build()?;
//!
//! let state = SharedState::new();
//! let result = graph.run(state).await?;
//! # Ok(())
//! # }
//! ```

use super::WorkflowEvent;
use super::checkpoint_store::{Checkpoint, CheckpointStore, InterruptType, MemoryCheckpointStore};
use super::node::Node;
use super::state::SharedState;
use crate::human_loop::ApprovalDecision;
use echo_core::agent::Agent;
use echo_core::error::{AgentError, ReactError, Result};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

// ── Edge ────────────────────────────────────────────────────────────────────

/// Edge routing logic
pub(crate) enum EdgeKind {
    /// Fixed transition: A -> B
    Fixed(String),
    /// Conditional transition: returns next node name based on state
    Conditional(Box<dyn ConditionFn>),
    /// Parallel fan-out: enter multiple nodes simultaneously; merge back to then node when all complete
    Parallel { targets: Vec<String>, then: String },
}

/// Condition function trait (object-safe)
pub(crate) trait ConditionFn: Send + Sync {
    fn evaluate<'a>(&'a self, state: &'a SharedState) -> BoxFuture<'a, String>;
}

struct CondWrapper<F>(F);

impl<F> ConditionFn for CondWrapper<F>
where
    F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, String> + Send + Sync,
{
    fn evaluate<'a>(&'a self, state: &'a SharedState) -> BoxFuture<'a, String> {
        (self.0)(state)
    }
}

/// All edges originating from a node
pub(crate) struct Edge {
    pub from: String,
    pub kind: EdgeKind,
}

// ── Interrupt Configuration ────────────────────────────────────────────────────

/// Interrupt configuration
#[derive(Debug, Clone, Default)]
pub struct InterruptConfig {
    /// Pause before entering these nodes
    pub before: Vec<String>,
    /// Pause after these nodes execute
    pub after: Vec<String>,
}

impl InterruptConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the node needs to pause before entering
    pub fn should_interrupt_before(&self, node_name: &str) -> bool {
        self.before.iter().any(|n| n == node_name || n == "*")
    }

    /// Check if the node needs to pause after execution
    pub fn should_interrupt_after(&self, node_name: &str) -> bool {
        self.after.iter().any(|n| n == node_name || n == "*")
    }

    /// Check if any interrupt is configured
    pub fn has_interrupts(&self) -> bool {
        !self.before.is_empty() || !self.after.is_empty()
    }
}

// ── Interrupt State ────────────────────────────────────────────────────────────

/// Interrupt state - state when execution is paused
#[derive(Debug)]
pub struct InterruptState {
    /// Checkpoint (used for recovery)
    pub checkpoint: Checkpoint,
    /// Interrupt type
    pub interrupt_type: InterruptType,
    /// Name of the paused node
    pub pending_node: String,
    /// Prompt for the user
    pub prompt: String,
}

impl InterruptState {
    /// Create a BeforeNode interrupt
    pub fn before_node(checkpoint: Checkpoint, node_name: String) -> Self {
        let prompt = format!(
            "Node '{}' requires confirmation before execution",
            node_name
        );
        Self {
            checkpoint,
            interrupt_type: InterruptType::BeforeNode,
            pending_node: node_name,
            prompt,
        }
    }

    /// Create an AfterNode interrupt
    pub fn after_node(checkpoint: Checkpoint, node_name: String) -> Self {
        let prompt = format!("Node '{}' requires confirmation after execution", node_name);
        Self {
            checkpoint,
            interrupt_type: InterruptType::AfterNode,
            pending_node: node_name,
            prompt,
        }
    }

    /// Create a ToolApproval interrupt
    pub fn tool_approval(checkpoint: Checkpoint, tool_name: String, args: Value) -> Self {
        let prompt = format!(
            "Tool '{}' requires approval\nParameters: {}",
            tool_name,
            serde_json::to_string_pretty(&args).unwrap_or_default()
        );
        Self {
            checkpoint,
            interrupt_type: InterruptType::ToolApproval,
            pending_node: tool_name,
            prompt,
        }
    }
}

/// Return type of run_until_interrupt
#[derive(Debug)]
pub enum RunUntilInterruptResult {
    /// Execution completed
    Completed(GraphResult),
    /// Paused at an interrupt point
    Interrupted(InterruptState),
}

// ── GraphBuilder ────────────────────────────────────────────────────────────

/// Graph workflow builder
///
/// Add nodes and edges through chained calls; finally `build()` produces an immutable [`Graph`].
pub struct GraphBuilder {
    name: String,
    nodes: HashMap<String, Node>,
    edges: Vec<Edge>,
    entry_node: Option<String>,
    finish_nodes: Vec<String>,
    /// Interrupt configuration
    interrupt_config: InterruptConfig,
}

impl GraphBuilder {
    /// Create an empty graph builder
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            nodes: HashMap::new(),
            edges: Vec::new(),
            entry_node: None,
            finish_nodes: Vec::new(),
            interrupt_config: InterruptConfig::default(),
        }
    }

    // ── Add Nodes ─────────────────────────────────────────────────────────

    /// Add an Agent node
    ///
    /// `input_key`: key for reading the prompt from state
    /// `output_key`: key for writing the execution result to state
    ///
    /// Defaults to `execute` mode (multi-turn with tools).
    pub fn add_agent_node(
        mut self,
        name: impl Into<String>,
        agent: impl Agent + 'static,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
    ) -> Self {
        let name = name.into();
        self.nodes.insert(
            name.clone(),
            Node::agent(&name, agent, input_key, output_key),
        );
        self
    }

    /// Add an Agent node (configurable execute/chat mode)
    ///
    /// `use_execute`: true uses execute (multi-turn with tools),
    /// false uses chat (single turn).
    pub fn add_agent_node_with_mode(
        mut self,
        name: impl Into<String>,
        agent: impl Agent + 'static,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
        use_execute: bool,
    ) -> Self {
        let name = name.into();
        self.nodes.insert(
            name.clone(),
            Node::agent_with_mode(&name, agent, input_key, output_key, use_execute),
        );
        self
    }

    /// Add a shared Agent node (Arc<dyn Agent>)
    pub fn add_shared_agent_node(
        mut self,
        name: impl Into<String>,
        agent: Arc<dyn Agent>,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
    ) -> Self {
        let name = name.into();
        self.nodes.insert(
            name.clone(),
            Node::agent_shared(&name, agent, input_key, output_key),
        );
        self
    }

    /// Add a shared Agent node (configurable execute/chat mode)
    pub fn add_shared_agent_node_with_mode(
        mut self,
        name: impl Into<String>,
        agent: Arc<dyn Agent>,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
        use_execute: bool,
    ) -> Self {
        let name = name.into();
        self.nodes.insert(
            name.clone(),
            Node::agent_shared_with_mode(&name, agent, input_key, output_key, use_execute),
        );
        self
    }

    /// Add a function node
    pub fn add_function_node<F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, Result<()>> + Send + Sync + 'static,
    {
        let name = name.into();
        self.nodes.insert(name.clone(), Node::function(&name, f));
        self
    }

    /// Add a router node (no execution logic, only used as a convergence point for conditional branches)
    pub fn add_router_node(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.nodes.insert(name.clone(), Node::passthrough(&name));
        self
    }

    /// Add a subgraph node — embeds a pre-built `Graph` as a node in this graph.
    ///
    /// The subgraph shares the parent's `SharedState`. Its execution is counted
    /// toward the parent's step limit.
    pub fn add_subgraph_node(mut self, name: impl Into<String>, subgraph: Graph) -> Self {
        let name = name.into();
        self.nodes.insert(name.clone(), Node::subgraph(&name, subgraph));
        self
    }

    // ── Add Edges ─────────────────────────────────────────────────────────

    /// Add a fixed edge: from -> to
    pub fn add_edge(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.edges.push(Edge {
            from: from.into(),
            kind: EdgeKind::Fixed(to.into()),
        });
        self
    }

    /// Add a conditional edge: from -> f(state) returns the target node name
    ///
    /// The string returned by the condition function must be a registered node name or `"__end__"`.
    pub fn add_conditional_edge<F>(mut self, from: impl Into<String>, f: F) -> Self
    where
        F: for<'a> Fn(&'a SharedState) -> BoxFuture<'a, String> + Send + Sync + 'static,
    {
        self.edges.push(Edge {
            from: from.into(),
            kind: EdgeKind::Conditional(Box::new(CondWrapper(f))),
        });
        self
    }

    /// Add a parallel edge: from -> [targets...] -> then
    ///
    /// After `from` completes, all nodes in `targets` execute **in parallel**; when all complete, execution enters `then`.
    /// State modifications from parallel nodes are merged before the then node.
    ///
    /// **Note:** Currently, parallel branches execute **sequentially** within the
    /// same async task (not via `tokio::spawn`), because `dyn Agent` nodes are
    /// not `Send + 'static`. Each branch receives an isolated fork of the state
    /// via `SharedState::fork()`, and results are merged back via `deep_merge()`.
    /// True concurrent execution will be enabled once the `Node` type supports
    /// `Send + 'static` bounds.
    pub fn add_parallel_edge(
        mut self,
        from: impl Into<String>,
        targets: Vec<String>,
        then: impl Into<String>,
    ) -> Self {
        self.edges.push(Edge {
            from: from.into(),
            kind: EdgeKind::Parallel {
                targets,
                then: then.into(),
            },
        });
        self
    }

    // ── Entry and Finish ──────────────────────────────────────────────────

    /// Set the entry node
    pub fn set_entry(mut self, name: impl Into<String>) -> Self {
        self.entry_node = Some(name.into());
        self
    }

    /// Set a finish node (graph execution ends upon reaching this node; multiple can be set)
    pub fn set_finish(mut self, name: impl Into<String>) -> Self {
        self.finish_nodes.push(name.into());
        self
    }

    // ── Interrupt Configuration ──────────────────────────────────────────────────

    /// Set interrupt_before (pause before entering nodes)
    ///
    /// Supports "*" wildcard to represent all nodes.
    ///
    /// # Example
    ///
    /// ```rust
    /// use echo_orchestration::workflow::GraphBuilder;
    ///
    /// let graph = GraphBuilder::new("my_flow")
    ///     .add_function_node("step1", |_| Box::pin(async { Ok(()) }))
    ///     .add_function_node("step2", |_| Box::pin(async { Ok(()) }))
    ///     .set_entry("step1")
    ///     .add_edge("step1", "step2")
    ///     .interrupt_before(vec!["step2"])  // Pause before entering step2
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn interrupt_before(mut self, nodes: Vec<&str>) -> Self {
        self.interrupt_config.before = nodes.into_iter().map(String::from).collect();
        self
    }

    /// Set interrupt_after (pause after nodes execute)
    ///
    /// Supports "*" wildcard to represent all nodes.
    pub fn interrupt_after(mut self, nodes: Vec<&str>) -> Self {
        self.interrupt_config.after = nodes.into_iter().map(String::from).collect();
        self
    }

    /// Build an immutable Graph
    pub fn build(self) -> Result<Graph> {
        let entry = self.entry_node.ok_or_else(|| {
            ReactError::Agent(Box::new(AgentError::InitializationFailed(
                "Graph must have an entry node (call set_entry())".to_string(),
            )))
        })?;

        if !self.nodes.contains_key(&entry) {
            return Err(ReactError::Agent(Box::new(AgentError::InitializationFailed(
                format!("Entry node '{}' not found in graph", entry),
            ))));
        }

        // Verify all nodes referenced by edges exist
        for edge in &self.edges {
            if !self.nodes.contains_key(&edge.from) {
                return Err(ReactError::Agent(Box::new(AgentError::InitializationFailed(
                    format!("Edge from unknown node '{}'", edge.from),
                ))));
            }
            match &edge.kind {
                EdgeKind::Fixed(to) if to != Graph::END && !self.nodes.contains_key(to) => {
                    return Err(ReactError::Agent(Box::new(AgentError::InitializationFailed(
                        format!("Edge to unknown node '{}'", to),
                    ))));
                }
                EdgeKind::Fixed(_) => {}
                EdgeKind::Parallel { targets, then } => {
                    for t in targets {
                        if !self.nodes.contains_key(t) {
                            return Err(ReactError::Agent(Box::new(AgentError::InitializationFailed(
                                format!("Parallel target node '{}' not found", t),
                            ))));
                        }
                    }
                    if then != Graph::END && !self.nodes.contains_key(then) {
                        return Err(ReactError::Agent(Box::new(AgentError::InitializationFailed(
                            format!("Parallel 'then' node '{}' not found", then),
                        ))));
                    }
                }
                _ => {}
            }
        }

        // Build adjacency list and validate multiple outgoing edges
        let mut edge_map: HashMap<String, Vec<Edge>> = HashMap::new();
        for edge in self.edges {
            let from = edge.from.clone();
            let entry = edge_map.entry(from.clone()).or_default();
            // Disallow multiple ordinary outgoing edges (Fixed / Conditional) from the same node,
            // because resolve_next() only takes the first edge; the rest would be silently discarded.
            if !entry.is_empty() {
                return Err(ReactError::Agent(Box::new(AgentError::InitializationFailed(
                    format!(
                        "Node '{}' has multiple outgoing edges; only one edge per node is supported \
                         (use a Conditional edge for branching, or a Parallel edge for fan-out)",
                        from,
                    ),
                ))));
            }
            entry.push(edge);
        }

        Ok(Graph {
            name: self.name,
            nodes: self.nodes,
            edges: edge_map,
            entry,
            finish_nodes: self.finish_nodes,
            max_steps: 100,
            interrupt_config: self.interrupt_config,
            checkpoint_store: Arc::new(MemoryCheckpointStore::new()),
        })
    }

    // ── Convenience Methods ───────────────────────────────────────────────

    /// Convenience method to add a ReactAgent node (default input="task", output="result")
    pub fn add_react_node(self, name: impl Into<String>, agent: impl Agent + 'static) -> Self {
        self.add_agent_node(name, agent, "task", "result")
    }
}

// ── Graph ───────────────────────────────────────────────────────────────────

/// Compiled graph workflow (immutable)
///
/// Built via [`GraphBuilder`], executed by calling [`run()`](Graph::run).
pub struct Graph {
    /// Graph name
    pub name: String,
    /// Node registry
    nodes: HashMap<String, Node>,
    /// Adjacency list: from -> [Edge]
    edges: HashMap<String, Vec<Edge>>,
    /// Entry node
    entry: String,
    /// List of finish nodes
    finish_nodes: Vec<String>,
    /// Maximum execution steps (prevents infinite loops)
    max_steps: usize,
    /// Interrupt configuration
    interrupt_config: InterruptConfig,
    /// Checkpoint store
    checkpoint_store: Arc<dyn CheckpointStore>,
}

/// Graph execution result
#[derive(Debug)]
pub struct GraphResult {
    /// Final state
    pub state: SharedState,
    /// Execution path (sequence of node names)
    pub path: Vec<String>,
    /// Total steps
    pub steps: usize,
}

impl Graph {
    /// Terminal marker node name
    pub const END: &'static str = "__end__";

    /// Set the maximum number of execution steps
    pub fn set_max_steps(&mut self, max: usize) {
        self.max_steps = max;
    }

    /// Execute the graph workflow
    ///
    /// Starting from the entry node, execute nodes in sequence following edge routing logic, until reaching a finish node or `__end__`.
    pub async fn run(&self, state: SharedState) -> Result<GraphResult> {
        let mut current = self.entry.clone();
        let mut path = Vec::new();
        let mut step_count = 0;

        info!(graph = %self.name, entry = %current, "Starting graph execution");

        loop {
            // Prevent infinite loop
            if step_count >= self.max_steps {
                warn!(
                    graph = %self.name,
                    steps = step_count,
                    "Graph execution exceeded max steps"
                );
                return Err(ReactError::Agent(Box::new(AgentError::MaxIterationsExceeded(
                    self.max_steps,
                ))));
            }

            // Check termination condition
            if current == Self::END || self.finish_nodes.contains(&current) {
                // If this is a finish node (not __end__), execute it first
                if current != Self::END
                    && let Some(node) = self.nodes.get(&current)
                {
                    state.set_current_node(&current);
                    debug!(graph = %self.name, node = %current, "Executing finish node");
                    node.execute(&state).await?;
                    path.push(current.clone());
                    step_count += 1;
                }
                info!(
                    graph = %self.name,
                    steps = step_count,
                    path = ?path,
                    "Graph execution completed"
                );
                return Ok(GraphResult {
                    state,
                    path,
                    steps: step_count,
                });
            }

            // Execute the current node
            let node = self.nodes.get(&current).ok_or_else(|| {
                ReactError::Agent(Box::new(AgentError::InitializationFailed(format!(
                    "Node '{}' not found in graph '{}'",
                    current, self.name
                ))))
            })?;

            state.set_current_node(&current);
            debug!(graph = %self.name, node = %current, step = step_count, "Executing node");
            node.execute(&state).await?;
            path.push(current.clone());
            step_count += 1;

            // Route to the next node
            let next = self.resolve_next(&current, &state).await?;

            match next {
                NextStep::Single(name) => {
                    current = name;
                }
                NextStep::Parallel { targets, then } => {
                    debug!(
                        graph = %self.name,
                        targets = ?targets,
                        then = %then,
                        "Executing parallel fan-out"
                    );

                    // Parallel branch execution (sequential execution, using deep_merge to merge results)
                    // Note: Because Node contains dyn traits (not Send + 'static),
                    // tokio::spawn cannot be used directly. Sequential execution is used here to guarantee correctness.
                    //
                    // To prevent later branches from overwriting earlier modifications, each branch uses an independent state clone,
                    // after execution completes, merge back to the main state via deep_merge semantics:
                    // - Nested object fields are recursively merged rather than wholesale overwritten
                    // - Simple keys (non-object) are still overwritten by later branches
                    for target_name in &targets {
                        if let Some(target_node) = self.nodes.get(target_name) {
                            // Clone the current state as the branch's independent state
                            let branch_state = state.fork()?;
                            branch_state.set_current_node(target_name);
                            debug!(graph = %self.name, node = %target_name, "Executing parallel branch");
                            target_node.execute(&branch_state).await?;

                            // deep_merge the branch state back into the main state
                            state.deep_merge(&branch_state)?;

                            path.push(target_name.clone());
                            step_count += 1;
                        }
                    }

                    current = then;
                }
                NextStep::End => {
                    info!(
                        graph = %self.name,
                        steps = step_count,
                        path = ?path,
                        "Graph execution completed (reached END)"
                    );
                    return Ok(GraphResult {
                        state,
                        path,
                        steps: step_count,
                    });
                }
            }
        }
    }

    // ── Interrupt + Checkpoint Methods ──────────────────────────────────────────

    /// Execute until an interrupt point, then pause
    ///
    /// If `interrupt_before` or `interrupt_after` is configured, execution pauses when reaching the corresponding node
    /// and returns `InterruptState`. Execution can be continued via the `resume()` method.
    ///
    /// # Returns
    ///
    /// - `RunUntilInterruptResult::Completed(GraphResult)` - Execution completed
    /// - `RunUntilInterruptResult::Interrupted(InterruptState)` - Paused at an interrupt point
    pub async fn run_until_interrupt(&self, state: SharedState) -> Result<RunUntilInterruptResult> {
        let mut current = self.entry.clone();
        let mut path = Vec::new();
        let mut step_count = 0;

        info!(graph = %self.name, entry = %current, "Starting graph execution (with interrupt)");

        loop {
            // Prevent infinite loop
            if step_count >= self.max_steps {
                warn!(
                    graph = %self.name,
                    steps = step_count,
                    "Graph execution exceeded max steps"
                );
                return Err(ReactError::Agent(Box::new(AgentError::MaxIterationsExceeded(
                    self.max_steps,
                ))));
            }

            // Check interrupt_before
            if self.interrupt_config.should_interrupt_before(&current) {
                debug!(graph = %self.name, node = %current, "Interrupt before node");

                let checkpoint = Checkpoint::new(
                    self.name.clone(),
                    current.clone(),
                    &state,
                    path.clone(),
                    step_count,
                    InterruptType::BeforeNode,
                );

                // Save the checkpoint
                self.checkpoint_store.save(&checkpoint).await?;

                let interrupt_state = InterruptState::before_node(checkpoint, current);
                return Ok(RunUntilInterruptResult::Interrupted(interrupt_state));
            }

            // Check termination condition
            if current == Self::END || self.finish_nodes.contains(&current) {
                if current != Self::END
                    && let Some(node) = self.nodes.get(&current)
                {
                    state.set_current_node(&current);
                    debug!(graph = %self.name, node = %current, "Executing finish node");
                    node.execute(&state).await?;
                    path.push(current.clone());
                    step_count += 1;
                }
                info!(
                    graph = %self.name,
                    steps = step_count,
                    path = ?path,
                    "Graph execution completed"
                );
                return Ok(RunUntilInterruptResult::Completed(GraphResult {
                    state,
                    path,
                    steps: step_count,
                }));
            }

            // Execute the current node
            let node = self.nodes.get(&current).ok_or_else(|| {
                ReactError::Agent(Box::new(AgentError::InitializationFailed(format!(
                    "Node '{}' not found in graph '{}'",
                    current, self.name
                ))))
            })?;

            state.set_current_node(&current);
            debug!(graph = %self.name, node = %current, step = step_count, "Executing node");
            node.execute(&state).await?;
            path.push(current.clone());
            step_count += 1;

            // Check interrupt_after
            if self.interrupt_config.should_interrupt_after(&current) {
                debug!(graph = %self.name, node = %current, "Interrupt after node");

                // Get the next node
                let next = self.resolve_next(&current, &state).await?;

                let checkpoint = Checkpoint::new(
                    self.name.clone(),
                    match next {
                        NextStep::Single(ref name) => name.clone(),
                        NextStep::Parallel { ref then, .. } => then.clone(),
                        NextStep::End => "__end__".to_string(),
                    },
                    &state,
                    path.clone(),
                    step_count,
                    InterruptType::AfterNode,
                );

                self.checkpoint_store.save(&checkpoint).await?;

                let interrupt_state = InterruptState::after_node(checkpoint, current);
                return Ok(RunUntilInterruptResult::Interrupted(interrupt_state));
            }

            // Route to the next node
            let next = self.resolve_next(&current, &state).await?;

            match next {
                NextStep::Single(name) => {
                    current = name;
                }
                NextStep::Parallel { targets, then } => {
                    debug!(
                        graph = %self.name,
                        targets = ?targets,
                        then = %then,
                        "Executing parallel fan-out"
                    );

                    // Parallel branch execution (using deep_merge to merge branch results)
                    for target_name in &targets {
                        if let Some(target_node) = self.nodes.get(target_name) {
                            // Clone state as branch-independent state
                            let branch_state = state.fork()?;
                            branch_state.set_current_node(target_name);
                            debug!(graph = %self.name, node = %target_name, "Executing parallel branch");
                            target_node.execute(&branch_state).await?;

                            // deep_merge back to main state
                            state.deep_merge(&branch_state)?;

                            path.push(target_name.clone());
                            step_count += 1;
                        }
                    }

                    current = then;
                }
                NextStep::End => {
                    info!(
                        graph = %self.name,
                        steps = step_count,
                        path = ?path,
                        "Graph execution completed (reached END)"
                    );
                    return Ok(RunUntilInterruptResult::Completed(GraphResult {
                        state,
                        path,
                        steps: step_count,
                    }));
                }
            }
        }
    }

    /// Resume execution from a Checkpoint
    ///
    /// When the user approves to continue, resume execution from the saved checkpoint.
    ///
    /// Fix: returns `RunUntilInterruptResult::Interrupted` instead of `Err` when encountering an interrupt point.
    pub async fn resume(
        &self,
        checkpoint: Checkpoint,
        decision: ApprovalDecision,
    ) -> Result<RunUntilInterruptResult> {
        // Check approval decision -- Rejected and Deferred must not continue execution
        match &decision {
            ApprovalDecision::Rejected { reason } => {
                info!(
                    graph = %self.name,
                    checkpoint_id = %checkpoint.id,
                    reason = reason.as_deref().unwrap_or("no reason"),
                    "Resume rejected, aborting workflow"
                );
                return Ok(RunUntilInterruptResult::Completed(GraphResult {
                    state: checkpoint.restore_state()?,
                    path: checkpoint.path,
                    steps: checkpoint.step_count,
                }));
            }
            ApprovalDecision::Deferred => {
                info!(
                    graph = %self.name,
                    checkpoint_id = %checkpoint.id,
                    "Resume deferred, aborting workflow"
                );
                return Ok(RunUntilInterruptResult::Completed(GraphResult {
                    state: checkpoint.restore_state()?,
                    path: checkpoint.path,
                    steps: checkpoint.step_count,
                }));
            }
            _ => {} // Approved, ApprovedWithScope, Modified — continue
        }

        // Restore state
        let state = checkpoint.restore_state()?;
        let mut current = checkpoint.current_node;
        let mut path = checkpoint.path;
        let mut step_count = checkpoint.step_count;

        info!(
            graph = %self.name,
            checkpoint_id = %checkpoint.id,
            node = %current,
            "Resuming from checkpoint"
        );

        // Continue execution
        loop {
            if step_count >= self.max_steps {
                return Err(ReactError::Agent(Box::new(AgentError::MaxIterationsExceeded(
                    self.max_steps,
                ))));
            }

            // Check interrupt_before (skip, as it has already been handled)
            // If resuming from a BeforeNode interrupt, the current node must be executed

            if current == Self::END || self.finish_nodes.contains(&current) {
                if current != Self::END
                    && let Some(node) = self.nodes.get(&current)
                {
                    state.set_current_node(&current);
                    node.execute(&state).await?;
                    path.push(current.clone());
                    step_count += 1;
                }
                // Delete checkpoint after successful completion
                self.checkpoint_store.delete(&checkpoint.id).await?;
                return Ok(RunUntilInterruptResult::Completed(GraphResult {
                    state,
                    path,
                    steps: step_count,
                }));
            }

            // Execute the current node
            let node = self.nodes.get(&current).ok_or_else(|| {
                ReactError::Agent(Box::new(AgentError::InitializationFailed(format!(
                    "Node '{}' not found",
                    current
                ))))
            })?;

            state.set_current_node(&current);
            node.execute(&state).await?;
            path.push(current.clone());
            step_count += 1;

            // Check interrupt_after
            if self.interrupt_config.should_interrupt_after(&current) {
                let next = self.resolve_next(&current, &state).await?;

                let next_node_name = match &next {
                    NextStep::Single(name) => name.clone(),
                    NextStep::Parallel { then, .. } => then.clone(),
                    NextStep::End => "__end__".to_string(),
                };

                let new_checkpoint = Checkpoint::new(
                    self.name.clone(),
                    next_node_name,
                    &state,
                    path.clone(),
                    step_count,
                    InterruptType::AfterNode,
                );

                self.checkpoint_store.save(&new_checkpoint).await?;

                // Delete the old checkpoint (the new one has been saved successfully)
                self.checkpoint_store.delete(&checkpoint.id).await?;

                let interrupt_state = InterruptState::after_node(new_checkpoint, current);
                return Ok(RunUntilInterruptResult::Interrupted(interrupt_state));
            }

            let next = self.resolve_next(&current, &state).await?;

            match next {
                NextStep::Single(name) => {
                    // Check interrupt_before for the next node
                    if self.interrupt_config.should_interrupt_before(&name) {
                        let new_checkpoint = Checkpoint::new(
                            self.name.clone(),
                            name.clone(),
                            &state,
                            path.clone(),
                            step_count,
                            InterruptType::BeforeNode,
                        );
                        self.checkpoint_store.save(&new_checkpoint).await?;

                        // Delete the old checkpoint (the new one has been saved successfully)
                        self.checkpoint_store.delete(&checkpoint.id).await?;

                        let interrupt_state = InterruptState::before_node(new_checkpoint, name);
                        return Ok(RunUntilInterruptResult::Interrupted(interrupt_state));
                    }
                    current = name;
                }
                NextStep::Parallel { targets, then } => {
                    // Parallel branch execution, shared SharedState (sequential execution)
                    for target_name in &targets {
                        if let Some(target_node) = self.nodes.get(target_name) {
                            state.set_current_node(target_name);
                            target_node.execute(&state).await?;
                            path.push(target_name.clone());
                            step_count += 1;
                        }
                    }
                    current = then;
                }
                NextStep::End => {
                    self.checkpoint_store.delete(&checkpoint.id).await?;
                    return Ok(RunUntilInterruptResult::Completed(GraphResult {
                        state,
                        path,
                        steps: step_count,
                    }));
                }
            }
        }
    }

    /// Resume execution from a checkpoint while injecting state modifications.
    ///
    /// Merge `state_updates` into the checkpoint's state before resuming,
    /// suitable for scenarios where the workflow state needs to be modified externally before continuing execution.
    pub async fn resume_with_state(
        &self,
        checkpoint: Checkpoint,
        state_updates: std::collections::HashMap<String, Value>,
    ) -> Result<RunUntilInterruptResult> {
        // Apply state updates to the checkpoint before resuming
        let state = checkpoint.restore_state()?;
        for (key, value) in &state_updates {
            let _ = state.set(key, value.clone());
        }

        // Reuse the original checkpoint identity so subsequent save/delete
        // operations still target the persisted checkpoint entry.
        let mut modified_checkpoint = checkpoint;
        modified_checkpoint.state_snapshot = state.to_json_value().map_err(|e| {
            ReactError::Other(format!("Failed to serialize updated workflow state: {}", e))
        })?;

        self.resume(modified_checkpoint, ApprovalDecision::Approved)
            .await
    }

    /// Restore a previous checkpoint's state WITHOUT executing further.
    ///
    /// Returns the restored state and the checkpoint metadata. Useful for
    /// inspecting historical states or preparing to branch.
    pub async fn restore_to_checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> Result<(SharedState, Checkpoint)> {
        let checkpoint = self
            .checkpoint_store
            .load(checkpoint_id)
            .await?
            .ok_or_else(|| {
                ReactError::Agent(Box::new(AgentError::InitializationFailed(format!(
                    "Checkpoint '{}' not found",
                    checkpoint_id
                ))))
            })?;
        let state = checkpoint.restore_state()?;
        Ok((state, checkpoint))
    }

    /// Fork a new execution from an existing checkpoint.
    ///
    /// Creates a new branch checkpoint with `state_updates` merged into the forked
    /// state, preserving lineage via `parent_checkpoint_id`. The new branch is
    /// stored and execution continues from the checkpointed node.
    pub async fn branch_from(
        &self,
        checkpoint_id: &str,
        state_updates: std::collections::HashMap<String, Value>,
        branch_name: Option<String>,
    ) -> Result<RunUntilInterruptResult> {
        let (state, parent_cp) = self.restore_to_checkpoint(checkpoint_id).await?;

        // Apply state updates
        for (key, value) in &state_updates {
            let _ = state.set(key, value.clone());
        }

        // Create a branch checkpoint
        let branch_name = branch_name.unwrap_or_else(|| format!("branch-{}", uuid::Uuid::new_v4()));
        let fork_cp = Checkpoint::branch(
            &parent_cp.id,
            &branch_name,
            self.name.clone(),
            parent_cp.current_node.clone(),
            &state,
            parent_cp.path.clone(),
            parent_cp.step_count,
            parent_cp.interrupt_type,
        );

        // Save fork checkpoint
        self.checkpoint_store.save(&fork_cp).await?;

        // Resume from the fork
        self.resume(fork_cp, ApprovalDecision::Approved).await
    }

    /// Attach a human-readable label and/or tags to an existing checkpoint.
    pub async fn tag_checkpoint(
        &self,
        checkpoint_id: &str,
        label: Option<&str>,
        tags: Vec<&str>,
    ) -> Result<()> {
        let mut checkpoint = self
            .checkpoint_store
            .load(checkpoint_id)
            .await?
            .ok_or_else(|| {
                ReactError::Agent(Box::new(AgentError::InitializationFailed(format!(
                    "Checkpoint '{}' not found",
                    checkpoint_id
                ))))
            })?;

        if let Some(l) = label {
            checkpoint.label = Some(l.to_string());
        }
        if !tags.is_empty() {
            checkpoint.tags = tags.iter().map(|t| t.to_string()).collect();
        }

        self.checkpoint_store.save(&checkpoint).await
    }

    /// Load a Checkpoint
    pub async fn load_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>> {
        self.checkpoint_store.load(id).await
    }

    /// List all Checkpoints
    pub async fn list_checkpoints(&self) -> Result<Vec<super::checkpoint_store::CheckpointInfo>> {
        self.checkpoint_store.list().await
    }

    /// List checkpoints filtered by graph name (this graph).
    pub async fn list_checkpoints_by_graph(&self) -> Result<Vec<super::checkpoint_store::CheckpointInfo>> {
        self.checkpoint_store.list_by_graph(&self.name).await
    }

    /// Set a custom Checkpoint store
    pub fn with_checkpoint_store(mut self, store: Arc<dyn CheckpointStore>) -> Self {
        self.checkpoint_store = store;
        self
    }

    /// Execute the graph workflow in streaming mode, emitting [`WorkflowEvent`] events node by node.
    ///
    /// Each node's Start/End events are emitted, and finally a `Completed` event is emitted.
    pub async fn run_stream(
        &self,
        state: SharedState,
    ) -> Result<BoxStream<'_, Result<WorkflowEvent>>> {
        let state_clone = state.clone();
        let stream = async_stream::try_stream! {
            let mut current = self.entry.clone();
            let mut path = Vec::new();
            let mut step_count = 0usize;
            let workflow_start = Instant::now();

            loop {
                if step_count >= self.max_steps {
                    Err(ReactError::Agent(Box::new(AgentError::MaxIterationsExceeded(self.max_steps))))?;
                }

                if current == Self::END || self.finish_nodes.contains(&current) {
                    if current != Self::END
                        && let Some(node) = self.nodes.get(&current)
                    {
                        state_clone.set_current_node(&current);
                        yield WorkflowEvent::NodeStart {
                            node_name: current.clone(),
                            step_index: step_count,
                        };
                        let node_start = Instant::now();
                        node.execute(&state_clone).await?;
                        yield WorkflowEvent::NodeEnd {
                            node_name: current.clone(),
                            step_index: step_count,
                            elapsed: node_start.elapsed(),
                        };
                        path.push(current.clone());
                        step_count += 1;
                    }

                    let final_result = state_clone
                        .get::<String>("result")
                        .or_else(|| state_clone.get::<String>("output"))
                        .unwrap_or_default();

                    yield WorkflowEvent::Completed {
                        result: final_result,
                        total_steps: step_count,
                        elapsed: workflow_start.elapsed(),
                    };
                    return;
                }

                let node = self.nodes.get(&current).ok_or_else(|| {
                    ReactError::Agent(Box::new(AgentError::InitializationFailed(format!(
                        "Node '{}' not found in graph '{}'",
                        current, self.name
                    ))))
                })?;

                state_clone.set_current_node(&current);
                yield WorkflowEvent::NodeStart {
                    node_name: current.clone(),
                    step_index: step_count,
                };
                let node_start = Instant::now();
                node.execute(&state_clone).await?;
                yield WorkflowEvent::NodeEnd {
                    node_name: current.clone(),
                    step_index: step_count,
                    elapsed: node_start.elapsed(),
                };
                path.push(current.clone());
                step_count += 1;

                let next = self.resolve_next(&current, &state_clone).await?;
                match next {
                    NextStep::Single(name) => {
                        current = name;
                    }
                    NextStep::Parallel { targets, then } => {
                        // Parallel branch execution (using deep_merge to merge branch results)
                        for target_name in &targets {
                            if let Some(target_node) = self.nodes.get(target_name) {
                                // Clone state as branch-independent state
                                let branch_state = state_clone.fork()?;
                                branch_state.set_current_node(target_name);
                                yield WorkflowEvent::NodeStart {
                                    node_name: target_name.clone(),
                                    step_index: step_count,
                                };
                                let branch_start = Instant::now();
                                target_node.execute(&branch_state).await?;
                                yield WorkflowEvent::NodeEnd {
                                    node_name: target_name.clone(),
                                    step_index: step_count,
                                    elapsed: branch_start.elapsed(),
                                };

                                // deep_merge back to main state
                                state_clone.deep_merge(&branch_state)?;

                                path.push(target_name.clone());
                                step_count += 1;
                            }
                        }
                        current = then;
                    }
                    NextStep::End => {
                        let final_result = state_clone
                            .get::<String>("result")
                            .or_else(|| state_clone.get::<String>("output"))
                            .unwrap_or_default();
                        yield WorkflowEvent::Completed {
                            result: final_result,
                            total_steps: step_count,
                            elapsed: workflow_start.elapsed(),
                        };
                        return;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    /// Resolve the next node
    async fn resolve_next(&self, current: &str, state: &SharedState) -> Result<NextStep> {
        let edges = match self.edges.get(current) {
            Some(e) => e,
            None => {
                // No outgoing edges -> if it is a finish node then end, otherwise report an error
                if self.finish_nodes.contains(&current.to_string()) {
                    return Ok(NextStep::End);
                }
                return Err(ReactError::Agent(Box::new(AgentError::InitializationFailed(
                    format!(
                        "Node '{}' has no outgoing edges and is not a finish node",
                        current
                    ),
                ))));
            }
        };

        // Take the first matching edge
        if let Some(edge) = edges.iter().next() {
            match &edge.kind {
                EdgeKind::Fixed(to) => {
                    if to == Self::END {
                        return Ok(NextStep::End);
                    }
                    return Ok(NextStep::Single(to.clone()));
                }
                EdgeKind::Conditional(f) => {
                    let target = f.evaluate(state).await;
                    if target == Self::END {
                        return Ok(NextStep::End);
                    }
                    return Ok(NextStep::Single(target));
                }
                EdgeKind::Parallel { targets, then } => {
                    return Ok(NextStep::Parallel {
                        targets: targets.clone(),
                        then: then.clone(),
                    });
                }
            }
        }

        // Should not be reached
        Ok(NextStep::End)
    }
}

/// Internal routing result
enum NextStep {
    Single(String),
    Parallel { targets: Vec<String>, then: String },
    End,
}

// ── Unit Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_linear_graph() {
        let graph = GraphBuilder::new("linear")
            .add_function_node("a", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("x", 1i64);
                    Ok(())
                })
            })
            .add_function_node("b", |state: &SharedState| {
                Box::pin(async move {
                    let x: i64 = state.get("x").unwrap();
                    let _ = state.set("x", x + 10);
                    Ok(())
                })
            })
            .add_function_node("c", |state: &SharedState| {
                Box::pin(async move {
                    let x: i64 = state.get("x").unwrap();
                    let _ = state.set("x", x * 2);
                    Ok(())
                })
            })
            .set_entry("a")
            .add_edge("a", "b")
            .add_edge("b", "c")
            .set_finish("c")
            .build()
            .unwrap();

        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();

        assert_eq!(result.state.get::<i64>("x"), Some(22)); // (1+10)*2
        assert_eq!(result.path, vec!["a", "b", "c"]);
        assert_eq!(result.steps, 3);
    }

    #[tokio::test]
    async fn test_conditional_graph() {
        let graph = GraphBuilder::new("conditional")
            .add_function_node("check", |_state: &SharedState| {
                Box::pin(async move {
                    // score is set externally
                    Ok(())
                })
            })
            .add_function_node("pass", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("result", "passed");
                    Ok(())
                })
            })
            .add_function_node("fail", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("result", "failed");
                    Ok(())
                })
            })
            .set_entry("check")
            .add_conditional_edge("check", |state: &SharedState| {
                Box::pin(async move {
                    let score: i64 = state.get("score").unwrap_or(0);
                    if score >= 60 {
                        "pass".to_string()
                    } else {
                        "fail".to_string()
                    }
                })
            })
            .set_finish("pass")
            .set_finish("fail")
            .build()
            .unwrap();

        // Test the pass path
        let state = SharedState::new();
        let _ = state.set("score", 80i64);
        let result = graph.run(state).await.unwrap();
        assert_eq!(
            result.state.get::<String>("result"),
            Some("passed".to_string())
        );
        assert_eq!(result.path, vec!["check", "pass"]);

        // Test the fail path
        let state = SharedState::new();
        let _ = state.set("score", 40i64);
        let result = graph.run(state).await.unwrap();
        assert_eq!(
            result.state.get::<String>("result"),
            Some("failed".to_string())
        );
        assert_eq!(result.path, vec!["check", "fail"]);
    }

    #[tokio::test]
    async fn test_loop_graph() {
        // Simulate a loop: counter increments from 0 to 5
        let graph = GraphBuilder::new("loop")
            .add_function_node("init", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("counter", 0i64);
                    Ok(())
                })
            })
            .add_function_node("increment", |state: &SharedState| {
                Box::pin(async move {
                    let c: i64 = state.get("counter").unwrap();
                    let _ = state.set("counter", c + 1);
                    Ok(())
                })
            })
            .add_function_node("done", |_state: &SharedState| {
                Box::pin(async move { Ok(()) })
            })
            .set_entry("init")
            .add_edge("init", "increment")
            .add_conditional_edge("increment", |state: &SharedState| {
                Box::pin(async move {
                    let c: i64 = state.get("counter").unwrap_or(0);
                    if c >= 5 {
                        "done".to_string()
                    } else {
                        "increment".to_string()
                    }
                })
            })
            .set_finish("done")
            .build()
            .unwrap();

        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        assert_eq!(result.state.get::<i64>("counter"), Some(5));
        // init + 5*increment + done = 7 steps
        assert_eq!(result.steps, 7);
    }

    #[tokio::test]
    async fn test_parallel_graph() {
        let graph = GraphBuilder::new("parallel")
            .add_function_node("start", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("input", "hello");
                    Ok(())
                })
            })
            .add_function_node("upper", |state: &SharedState| {
                Box::pin(async move {
                    let s: String = state.get("input").unwrap();
                    let _ = state.set("upper_result", s.to_uppercase());
                    Ok(())
                })
            })
            .add_function_node("length", |state: &SharedState| {
                Box::pin(async move {
                    let s: String = state.get("input").unwrap();
                    let _ = state.set("length_result", s.len() as i64);
                    Ok(())
                })
            })
            .add_function_node("combine", |state: &SharedState| {
                Box::pin(async move {
                    let u: String = state.get("upper_result").unwrap();
                    let l: i64 = state.get("length_result").unwrap();
                    let _ = state.set("final", format!("{u} (len={l})"));
                    Ok(())
                })
            })
            .set_entry("start")
            .add_parallel_edge(
                "start",
                vec!["upper".to_string(), "length".to_string()],
                "combine",
            )
            .set_finish("combine")
            .build()
            .unwrap();

        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        assert_eq!(
            result.state.get::<String>("final"),
            Some("HELLO (len=5)".to_string())
        );
    }

    #[tokio::test]
    async fn test_end_edge() {
        let graph = GraphBuilder::new("end_test")
            .add_function_node("only", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("done", true);
                    Ok(())
                })
            })
            .set_entry("only")
            .add_edge("only", "__end__")
            .build()
            .unwrap();

        let state = SharedState::new();
        let result = graph.run(state).await.unwrap();
        assert_eq!(result.state.get::<bool>("done"), Some(true));
        assert_eq!(result.path, vec!["only"]);
    }

    #[tokio::test]
    async fn test_max_steps_exceeded() {
        let mut graph = GraphBuilder::new("infinite")
            .add_function_node("loop_node", |_state: &SharedState| {
                Box::pin(async move { Ok(()) })
            })
            .set_entry("loop_node")
            .add_edge("loop_node", "loop_node") // infinite loop
            .build()
            .unwrap();

        graph.set_max_steps(10);

        let state = SharedState::new();
        let result = graph.run(state).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_build_validation_missing_entry() {
        let result = GraphBuilder::new("bad")
            .add_function_node("a", |_: &SharedState| Box::pin(async { Ok(()) }))
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_validation_unknown_entry() {
        let result = GraphBuilder::new("bad")
            .add_function_node("a", |_: &SharedState| Box::pin(async { Ok(()) }))
            .set_entry("nonexistent")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_validation_unknown_edge_target() {
        let result = GraphBuilder::new("bad")
            .add_function_node("a", |_: &SharedState| Box::pin(async { Ok(()) }))
            .set_entry("a")
            .add_edge("a", "nonexistent")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_rejects_multiple_outgoing_edges() {
        // Same node has two outgoing edges -> build phase should error
        let result = GraphBuilder::new("multi_edge")
            .add_function_node("a", |_: &SharedState| Box::pin(async { Ok(()) }))
            .add_function_node("b", |_: &SharedState| Box::pin(async { Ok(()) }))
            .add_function_node("c", |_: &SharedState| Box::pin(async { Ok(()) }))
            .set_entry("a")
            .add_edge("a", "b")
            .add_edge("a", "c") // second outgoing edge
            .build();
        assert!(
            result.is_err(),
            "Multiple outgoing edges should be rejected"
        );
        let err_msg = match result {
            Err(e) => e.to_string(),
            _ => String::new(),
        };
        assert!(
            err_msg.contains("multiple outgoing edges"),
            "Error should mention 'multiple outgoing edges', got: {err_msg}"
        );
    }

    // ── Feature 4: Workflow streaming output (run_stream) ──────────────────────

    #[tokio::test]
    async fn test_run_stream_linear() {
        use super::WorkflowEvent;
        use futures::StreamExt;

        let graph = GraphBuilder::new("stream_linear")
            .add_function_node("a", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("x", 1i64);
                    Ok(())
                })
            })
            .add_function_node("b", |state: &SharedState| {
                Box::pin(async move {
                    let x: i64 = state.get("x").unwrap();
                    let _ = state.set("result", format!("x={}", x));
                    Ok(())
                })
            })
            .set_entry("a")
            .add_edge("a", "b")
            .set_finish("b")
            .build()
            .unwrap();

        let state = SharedState::new();
        let mut stream = graph.run_stream(state).await.unwrap();

        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }

        let node_starts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                WorkflowEvent::NodeStart { node_name, .. } => Some(node_name.clone()),
                _ => None,
            })
            .collect();
        let node_ends: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                WorkflowEvent::NodeEnd { node_name, .. } => Some(node_name.clone()),
                _ => None,
            })
            .collect();
        let completed = events
            .iter()
            .any(|e| matches!(e, WorkflowEvent::Completed { .. }));

        assert_eq!(node_starts, vec!["a", "b"]);
        assert_eq!(node_ends, vec!["a", "b"]);
        assert!(completed, "Should receive Completed event");
    }

    #[tokio::test]
    async fn test_run_stream_parallel() {
        use super::WorkflowEvent;
        use futures::StreamExt;

        let graph = GraphBuilder::new("stream_parallel")
            .add_function_node("start", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("val", "ok");
                    Ok(())
                })
            })
            .add_function_node("b1", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("b1_done", true);
                    Ok(())
                })
            })
            .add_function_node("b2", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("b2_done", true);
                    Ok(())
                })
            })
            .add_function_node("merge", |state: &SharedState| {
                Box::pin(async move {
                    let b1: bool = state.get("b1_done").unwrap_or(false);
                    let b2: bool = state.get("b2_done").unwrap_or(false);
                    let _ = state.set("result", format!("b1={b1},b2={b2}"));
                    Ok(())
                })
            })
            .set_entry("start")
            .add_parallel_edge("start", vec!["b1".into(), "b2".into()], "merge")
            .set_finish("merge")
            .build()
            .unwrap();

        let state = SharedState::new();
        let mut stream = graph.run_stream(state).await.unwrap();

        let mut node_start_names = Vec::new();
        let mut completed_result = None;

        while let Some(event) = stream.next().await {
            match event.unwrap() {
                WorkflowEvent::NodeStart { node_name, .. } => {
                    node_start_names.push(node_name);
                }
                WorkflowEvent::Completed { result, .. } => {
                    completed_result = Some(result);
                }
                _ => {}
            }
        }

        assert!(node_start_names.contains(&"start".to_string()));
        assert!(node_start_names.contains(&"b1".to_string()));
        assert!(node_start_names.contains(&"b2".to_string()));
        assert!(node_start_names.contains(&"merge".to_string()));
        assert_eq!(completed_result, Some("b1=true,b2=true".to_string()));
    }

    #[tokio::test]
    async fn test_run_stream_conditional() {
        use super::WorkflowEvent;
        use futures::StreamExt;

        let graph = GraphBuilder::new("stream_cond")
            .add_function_node("check", |_state: &SharedState| {
                Box::pin(async move { Ok(()) })
            })
            .add_function_node("yes", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("result", "took_yes_path");
                    Ok(())
                })
            })
            .add_function_node("no", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("result", "took_no_path");
                    Ok(())
                })
            })
            .set_entry("check")
            .add_conditional_edge("check", |state: &SharedState| {
                Box::pin(async move {
                    let flag: bool = state.get("flag").unwrap_or(false);
                    if flag {
                        "yes".to_string()
                    } else {
                        "no".to_string()
                    }
                })
            })
            .set_finish("yes")
            .set_finish("no")
            .build()
            .unwrap();

        let state = SharedState::new();
        let _ = state.set("flag", true);
        let mut stream = graph.run_stream(state).await.unwrap();

        let mut visited = Vec::new();
        while let Some(event) = stream.next().await {
            if let WorkflowEvent::NodeStart { node_name, .. } = event.unwrap() {
                visited.push(node_name);
            }
        }

        assert_eq!(visited, vec!["check", "yes"]);
    }

    #[tokio::test]
    async fn test_resume_with_state_reuses_checkpoint_identity() {
        use std::collections::HashMap;

        let checkpoint_store = Arc::new(MemoryCheckpointStore::new());
        let graph = GraphBuilder::new("resume_with_state")
            .add_function_node("start", |state: &SharedState| {
                Box::pin(async move {
                    let _ = state.set("message", "original");
                    Ok(())
                })
            })
            .add_function_node("finish", |state: &SharedState| {
                Box::pin(async move {
                    let msg: String = state.get("message").unwrap_or_default();
                    let _ = state.set("result", format!("seen={msg}"));
                    Ok(())
                })
            })
            .set_entry("start")
            .add_edge("start", "finish")
            .set_finish("finish")
            .interrupt_before(vec!["finish"])
            .build()
            .unwrap()
            .with_checkpoint_store(checkpoint_store.clone());

        let state = SharedState::new();
        let interrupted = graph.run_until_interrupt(state).await.unwrap();
        let checkpoint = match interrupted {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            RunUntilInterruptResult::Completed(_) => panic!("expected interrupt"),
        };

        assert_eq!(graph.list_checkpoints().await.unwrap().len(), 1);

        let mut updates = HashMap::new();
        updates.insert("message".to_string(), Value::String("patched".to_string()));

        let resumed = graph.resume_with_state(checkpoint, updates).await.unwrap();
        let result = match resumed {
            RunUntilInterruptResult::Completed(result) => result,
            RunUntilInterruptResult::Interrupted(_) => panic!("expected completed result"),
        };

        let seen: String = result.state.get("result").unwrap_or_default();
        assert_eq!(seen, "seen=patched");
        assert!(
            graph.list_checkpoints().await.unwrap().is_empty(),
            "checkpoint should be deleted after successful resume"
        );
    }
}
