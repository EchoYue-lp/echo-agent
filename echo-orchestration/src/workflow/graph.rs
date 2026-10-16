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
use super::checkpoint_store::{
    Checkpoint, CheckpointStore, InterruptType, MemoryCheckpointStore, WorkflowContinuation,
};
use super::node::Node;
use super::state::SharedState;
use crate::human_loop::ApprovalDecision;
use echo_core::agent::Agent;
use echo_core::error::{AgentError, ReactError, Result};
use futures::future::BoxFuture;
use futures::stream::{BoxStream, FuturesUnordered};
use futures::{FutureExt, StreamExt};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
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
}

/// Return type of run_until_interrupt
#[derive(Debug)]
// Keep the public result variants unboxed to preserve the workflow API.
#[allow(clippy::large_enum_variant)]
pub enum RunUntilInterruptResult {
    /// Execution completed
    Completed(GraphResult),
    /// Paused at an interrupt point
    Interrupted(InterruptState),
    /// Approval was deferred; the checkpoint remains claimable.
    Deferred(InterruptState),
    /// Approval was rejected and the checkpoint was consumed.
    Rejected {
        state: SharedState,
        path: Vec<String>,
        steps: usize,
        reason: Option<String>,
    },
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
    duplicate_nodes: HashSet<String>,
    explicit_revision: Option<String>,
    node_timeout: Option<Duration>,
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
            duplicate_nodes: HashSet::new(),
            explicit_revision: None,
            node_timeout: None,
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
        if self.nodes.contains_key(&name) {
            self.duplicate_nodes.insert(name.clone());
        }
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
        if self.nodes.contains_key(&name) {
            self.duplicate_nodes.insert(name.clone());
        }
        self.nodes.insert(
            name.clone(),
            Node::agent_with_mode(&name, agent, input_key, output_key, use_execute),
        );
        self
    }

    /// Add a shared Agent node (`Arc<dyn Agent>`).
    pub fn add_shared_agent_node(
        mut self,
        name: impl Into<String>,
        agent: Arc<dyn Agent>,
        input_key: impl Into<String>,
        output_key: impl Into<String>,
    ) -> Self {
        let name = name.into();
        if self.nodes.contains_key(&name) {
            self.duplicate_nodes.insert(name.clone());
        }
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
        if self.nodes.contains_key(&name) {
            self.duplicate_nodes.insert(name.clone());
        }
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
        if self.nodes.contains_key(&name) {
            self.duplicate_nodes.insert(name.clone());
        }
        self.nodes.insert(name.clone(), Node::function(&name, f));
        self
    }

    /// Add a router node (no execution logic, only used as a convergence point for conditional branches)
    pub fn add_router_node(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if self.nodes.contains_key(&name) {
            self.duplicate_nodes.insert(name.clone());
        }
        self.nodes.insert(name.clone(), Node::passthrough(&name));
        self
    }

    /// Add a subgraph node — embeds a pre-built `Graph` as a node in this graph.
    ///
    /// The subgraph shares the parent's `SharedState`. Its execution is counted
    /// toward the parent's step limit.
    pub fn add_subgraph_node(mut self, name: impl Into<String>, subgraph: Graph) -> Self {
        let name = name.into();
        if self.nodes.contains_key(&name) {
            self.duplicate_nodes.insert(name.clone());
        }
        self.nodes
            .insert(name.clone(), Node::subgraph(&name, subgraph));
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

    /// Set an explicit revision when node behavior changes without a topology change.
    pub fn revision(mut self, revision: impl Into<String>) -> Self {
        self.explicit_revision = Some(revision.into());
        self
    }

    /// Apply a deadline to every node execution, including fan-out branches.
    pub fn node_timeout(mut self, timeout: Duration) -> Self {
        self.node_timeout = Some(timeout);
        self
    }

    /// Build an immutable Graph
    pub fn build(self) -> Result<Graph> {
        if !self.duplicate_nodes.is_empty() {
            let mut duplicates = self.duplicate_nodes.into_iter().collect::<Vec<_>>();
            duplicates.sort();
            return Err(ReactError::Agent(Box::new(
                AgentError::InitializationFailed(format!(
                    "Graph contains duplicate node IDs: {}",
                    duplicates.join(", ")
                )),
            )));
        }
        let entry = self.entry_node.ok_or_else(|| {
            ReactError::Agent(Box::new(AgentError::InitializationFailed(
                "Graph must have an entry node (call set_entry())".to_string(),
            )))
        })?;

        if !self.nodes.contains_key(&entry) {
            return Err(ReactError::Agent(Box::new(
                AgentError::InitializationFailed(format!(
                    "Entry node '{}' not found in graph",
                    entry
                )),
            )));
        }

        for finish in &self.finish_nodes {
            if !self.nodes.contains_key(finish) {
                return Err(ReactError::Agent(Box::new(
                    AgentError::InitializationFailed(format!(
                        "Finish node '{finish}' not found in graph"
                    )),
                )));
            }
        }
        for interrupt in self
            .interrupt_config
            .before
            .iter()
            .chain(self.interrupt_config.after.iter())
        {
            if interrupt != "*" && !self.nodes.contains_key(interrupt) {
                return Err(ReactError::Agent(Box::new(
                    AgentError::InitializationFailed(format!(
                        "Interrupt node '{interrupt}' not found in graph"
                    )),
                )));
            }
        }

        // Verify all nodes referenced by edges exist
        for edge in &self.edges {
            if !self.nodes.contains_key(&edge.from) {
                return Err(ReactError::Agent(Box::new(
                    AgentError::InitializationFailed(format!(
                        "Edge from unknown node '{}'",
                        edge.from
                    )),
                )));
            }
            match &edge.kind {
                EdgeKind::Fixed(to) if to != Graph::END && !self.nodes.contains_key(to) => {
                    return Err(ReactError::Agent(Box::new(
                        AgentError::InitializationFailed(format!("Edge to unknown node '{}'", to)),
                    )));
                }
                EdgeKind::Fixed(_) => {}
                EdgeKind::Parallel { targets, then } => {
                    if targets.is_empty() {
                        return Err(ReactError::Agent(Box::new(
                            AgentError::InitializationFailed(
                                "Parallel edge must contain at least one target".to_string(),
                            ),
                        )));
                    }
                    if targets.iter().collect::<HashSet<_>>().len() != targets.len() {
                        return Err(ReactError::Agent(Box::new(
                            AgentError::InitializationFailed(
                                "Parallel edge contains duplicate targets".to_string(),
                            ),
                        )));
                    }
                    for t in targets {
                        if !self.nodes.contains_key(t) {
                            return Err(ReactError::Agent(Box::new(
                                AgentError::InitializationFailed(format!(
                                    "Parallel target node '{}' not found",
                                    t
                                )),
                            )));
                        }
                    }
                    if then != Graph::END && !self.nodes.contains_key(then) {
                        return Err(ReactError::Agent(Box::new(
                            AgentError::InitializationFailed(format!(
                                "Parallel 'then' node '{}' not found",
                                then
                            )),
                        )));
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
                return Err(ReactError::Agent(Box::new(
                    AgentError::InitializationFailed(format!(
                        "Node '{}' has multiple outgoing edges; only one edge per node is supported \
                         (use a Conditional edge for branching, or a Parallel edge for fan-out)",
                        from,
                    )),
                )));
            }
            entry.push(edge);
        }

        let graph_revision = self.explicit_revision.unwrap_or_else(|| {
            let mut nodes = self.nodes.keys().cloned().collect::<Vec<_>>();
            nodes.sort();
            let mut topology = format!("{}|{}|{}", self.name, entry, nodes.join(","));
            let mut sources = edge_map.keys().cloned().collect::<Vec<_>>();
            sources.sort();
            for source in sources {
                if let Some(edges) = edge_map.get(&source) {
                    for edge in edges {
                        let target = match &edge.kind {
                            EdgeKind::Fixed(to) => format!("fixed:{to}"),
                            EdgeKind::Conditional(_) => "conditional".to_string(),
                            EdgeKind::Parallel { targets, then } => {
                                format!("fanout:{}:{then}", targets.join(","))
                            }
                        };
                        topology.push_str(&format!("|{source}>{target}"));
                    }
                }
            }
            format!("{:x}", Sha256::digest(topology.as_bytes()))
        });

        Ok(Graph {
            name: self.name,
            nodes: self
                .nodes
                .into_iter()
                .map(|(name, node)| (name, Arc::new(node)))
                .collect(),
            edges: edge_map
                .into_iter()
                .map(|(name, edges)| (name, edges.into_iter().map(Arc::new).collect()))
                .collect(),
            entry,
            finish_nodes: self.finish_nodes,
            max_steps: 100,
            interrupt_config: self.interrupt_config,
            checkpoint_store: Arc::new(MemoryCheckpointStore::new()),
            cancel_token: None,
            graph_revision,
            node_timeout: self.node_timeout,
            #[cfg(test)]
            before_stream_observation: None,
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
    nodes: HashMap<String, Arc<Node>>,
    /// Adjacency list: from -> [Edge]
    edges: HashMap<String, Vec<Arc<Edge>>>,
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
    /// Optional cancellation token for cooperative cancellation between nodes.
    cancel_token: Option<CancellationToken>,
    /// Stable hash or caller-supplied revision of this graph definition.
    graph_revision: String,
    /// Optional per-node execution deadline.
    node_timeout: Option<Duration>,
    #[cfg(test)]
    before_stream_observation: Option<Arc<tokio::sync::Notify>>,
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

// This is invocation-local observation/cause state, never a checkpoint or
// execution registry. The sole owned loop consumes the original error once.
pub(crate) struct GraphExecutionControl {
    pub(crate) cancel: CancellationToken,
    run_id: std::sync::Mutex<Option<String>>,
    attempt_id: std::sync::Mutex<Option<String>>,
    pub(crate) operation_id: String,
    cause: std::sync::Mutex<Option<(Option<String>, ReactError)>>,
}

pub(crate) enum GraphLoopOutcome {
    Settled(Box<RunUntilInterruptResult>),
    CauseRecorded,
}

impl GraphExecutionControl {
    pub(crate) fn new(run_id: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            cancel: CancellationToken::new(),
            run_id: std::sync::Mutex::new(run_id),
            attempt_id: std::sync::Mutex::new(None),
            operation_id: uuid::Uuid::new_v4().to_string(),
            cause: std::sync::Mutex::new(None),
        })
    }

    fn bind_run(&self, run_id: &str) {
        *self
            .run_id
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(run_id.to_string());
    }

    pub(crate) fn run_id(&self) -> Option<String> {
        self.run_id
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub(crate) fn record_error(&self, node: Option<&str>, error: ReactError) {
        let mut candidate = Some((node.map(str::to_string), error));
        {
            let mut cause = self.cause.lock().unwrap_or_else(|error| error.into_inner());
            if cause.is_none() {
                *cause = candidate.take();
            }
        }
        drop(candidate);
        self.cancel.cancel();
    }

    pub(crate) fn has_error(&self) -> bool {
        self.cause
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }

    fn node_error(&self, node: &str) -> Option<String> {
        self.cause
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .and_then(|(origin, error)| {
                (origin.as_deref() == Some(node)).then(|| error.to_string())
            })
    }

    fn finish(&self, result: Result<GraphLoopOutcome>) -> Result<RunUntilInterruptResult> {
        if let Err(error @ ReactError::ExecutionNotSettled { .. }) = result {
            return Err(error);
        }
        let cause = self
            .cause
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some((_, error)) = cause {
            return Err(error);
        }
        match result {
            Ok(GraphLoopOutcome::Settled(result)) => Ok(*result),
            Ok(GraphLoopOutcome::CauseRecorded) => Err(ReactError::Other(
                "Graph stopped without its recorded cause".into(),
            )),
            Err(error) => Err(error),
        }
    }
}

struct GraphExecution {
    control: Arc<GraphExecutionControl>,
    state: SharedState,
    current: String,
    path: Vec<String>,
    step_count: usize,
    continuation: WorkflowContinuation,
    parent_checkpoint: Option<Checkpoint>,
    skip_interrupt_before_once: bool,
}

impl Graph {
    fn retained_definition(&self) -> Self {
        Self {
            name: self.name.clone(),
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
            entry: self.entry.clone(),
            finish_nodes: self.finish_nodes.clone(),
            max_steps: self.max_steps,
            interrupt_config: self.interrupt_config.clone(),
            checkpoint_store: self.checkpoint_store.clone(),
            cancel_token: self.cancel_token.clone(),
            graph_revision: self.graph_revision.clone(),
            node_timeout: self.node_timeout,
            #[cfg(test)]
            before_stream_observation: self.before_stream_observation.clone(),
        }
    }

    async fn observe_owned(
        owner: tokio::task::JoinHandle<Result<GraphLoopOutcome>>,
        control: Arc<GraphExecutionControl>,
    ) -> Result<RunUntilInterruptResult> {
        let guard = control.cancel.clone().drop_guard();
        let result = match owner.await {
            Ok(result) => control.finish(result),
            Err(error) => match control.run_id() {
                Some(run_id) => Err(ReactError::ExecutionNotSettled {
                    run_id,
                    execution_id: control
                        .attempt_id
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .clone(),
                }),
                None => Err(ReactError::Other(format!(
                    "Graph owner stopped before effect admission: {error}"
                ))),
            },
        };
        let _ = guard.disarm();
        result
    }

    async fn run_owned(
        &self,
        state: SharedState,
        honor_interrupts: bool,
    ) -> Result<RunUntilInterruptResult> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            ReactError::Other(format!("Graph execution requires a Tokio runtime: {error}"))
        })?;
        let control = GraphExecutionControl::new(Some(uuid::Uuid::new_v4().to_string()));
        let execution = self.fresh_execution(state, control.clone());
        let owner = self.retained_definition();
        let task = runtime
            .spawn(async move { owner.execute_loop(execution, honor_interrupts, None).await });
        Self::observe_owned(task, control).await
    }

    pub(crate) async fn run_nested(
        &self,
        state: SharedState,
        control: Arc<GraphExecutionControl>,
    ) -> Result<GraphLoopOutcome> {
        self.execute_loop(self.fresh_execution(state, control), false, None)
            .await
    }

    /// Whether this Graph's bound Store supports durable physical claim protection.
    pub fn supports_protected_claims(&self) -> bool {
        self.checkpoint_store.supports_protected_claims()
    }

    fn emit_event(
        events: Option<&UnboundedSender<WorkflowEvent>>,
        event: WorkflowEvent,
    ) -> Result<()> {
        let Some(events) = events else {
            return Ok(());
        };
        events
            .send(event)
            .map_err(|_| ReactError::Other("Workflow event stream closed".to_string()))
    }

    async fn execute_node(
        &self,
        node_name: &str,
        node: &Node,
        state: &SharedState,
        events: Option<&UnboundedSender<WorkflowEvent>>,
        control: Arc<GraphExecutionControl>,
    ) -> Result<bool> {
        if control.cancel.is_cancelled() || self.is_cancelled() {
            control.record_error(
                Some(node_name),
                ReactError::Agent(Box::new(AgentError::Cancelled(format!(
                    "Graph '{}' cancelled before node admission",
                    self.name
                )))),
            );
            return Ok(false);
        }
        let node_cancel = control.cancel.child_token();
        let execution_id = uuid::Uuid::new_v4().to_string();
        let execution = async {
            let mut tokens = node.execute_stream(
                state,
                node_cancel.clone(),
                control.clone(),
                node_name,
                &execution_id,
            );
            let mut uncertainty = None;
            while let Some(token) = tokens.next().await {
                match token {
                    Ok(token) if !control.has_error() => {
                        if let Err(error) = Self::emit_event(
                            events,
                            WorkflowEvent::Token {
                                node_name: node_name.to_string(),
                                token,
                            },
                        ) {
                            control.record_error(Some(node_name), error);
                            node_cancel.cancel();
                        }
                    }
                    Ok(_) => {}
                    Err(error @ ReactError::ExecutionNotSettled { .. }) => {
                        uncertainty = Some(error)
                    }
                    Err(error) => control.record_error(Some(node_name), error),
                }
            }
            if let Some(error) = uncertainty {
                return Err(error);
            }
            if let Some(message) = control.node_error(node_name)
                && let Err(error) = Self::emit_event(
                    events,
                    WorkflowEvent::NodeError {
                        node_name: node_name.to_string(),
                        error: message,
                    },
                )
            {
                control.record_error(Some(node_name), error);
            }
            Ok(!control.has_error())
        };
        tokio::pin!(execution);
        let deadline = self
            .node_timeout
            .and_then(|duration| tokio::time::Instant::now().checked_add(duration));
        let interrupted = tokio::select! {
            biased;
            result = &mut execution => return result,
            () = control.cancel.cancelled() => ReactError::Agent(Box::new(AgentError::Cancelled(format!("Graph '{}' node execution cancelled", self.name)))),
            () = async { match &self.cancel_token { Some(token) => token.cancelled().await, None => std::future::pending().await } } => ReactError::Agent(Box::new(AgentError::Cancelled(format!("Graph '{}' node execution cancelled", self.name)))),
            () = async { match deadline { Some(deadline) => tokio::time::sleep_until(deadline).await, None => std::future::pending().await } } => ReactError::Other(format!("Workflow node timed out after {:?}", self.node_timeout)),
        };
        control.record_error(Some(node_name), interrupted);
        node_cancel.cancel();
        execution.await
    }

    fn validate_checkpoint(&self, checkpoint: &Checkpoint) -> Result<()> {
        if checkpoint.graph_name != self.name {
            return Err(ReactError::Other(format!(
                "Checkpoint belongs to graph '{}', not '{}'",
                checkpoint.graph_name, self.name
            )));
        }
        if checkpoint.graph_revision != self.graph_revision {
            return Err(ReactError::Other(format!(
                "Checkpoint graph revision '{}' does not match '{}'",
                checkpoint.graph_revision, self.graph_revision
            )));
        }
        if checkpoint.workflow_run_id.is_empty() {
            return Err(ReactError::Other(
                "Checkpoint has no workflow run identity".to_string(),
            ));
        }
        if checkpoint.current_node != Self::END
            && !self.nodes.contains_key(&checkpoint.current_node)
        {
            return Err(ReactError::Other(format!(
                "Checkpoint references unknown node '{}'",
                checkpoint.current_node
            )));
        }
        if let WorkflowContinuation::FanOut { targets, then } = &checkpoint.continuation
            && (targets.is_empty()
                || targets
                    .iter()
                    .any(|target| !self.nodes.contains_key(target))
                || (then != Self::END && !self.nodes.contains_key(then)))
        {
            return Err(ReactError::Other(
                "Checkpoint contains an invalid fan-out continuation".to_string(),
            ));
        }
        Ok(())
    }

    fn interrupt_state(checkpoint: Checkpoint) -> InterruptState {
        let node = checkpoint.current_node.clone();
        match checkpoint.interrupt_type {
            InterruptType::BeforeNode => InterruptState::before_node(checkpoint, node),
            InterruptType::AfterNode => InterruptState::after_node(checkpoint, node),
        }
    }

    fn checkpoint(
        &self,
        current_node: String,
        state: &SharedState,
        path: Vec<String>,
        step_count: usize,
        interrupt_type: InterruptType,
        continuation: WorkflowContinuation,
    ) -> Result<Checkpoint> {
        Checkpoint::try_new(
            self.name.clone(),
            current_node,
            state,
            path,
            step_count,
            interrupt_type,
        )
        .map(|checkpoint| checkpoint.bind_execution(self.graph_revision.clone(), continuation))
    }

    async fn execute_parallel_route(
        &self,
        state: &SharedState,
        targets: &[String],
        path: &mut Vec<String>,
        step_count: &mut usize,
        events: Option<&UnboundedSender<WorkflowEvent>>,
        control: Arc<GraphExecutionControl>,
    ) -> Result<bool> {
        let remaining = self.max_steps.saturating_sub(*step_count);
        if targets.len() > remaining {
            return Err(ReactError::Agent(Box::new(
                AgentError::MaxIterationsExceeded(self.max_steps),
            )));
        }
        let mut branches = Vec::with_capacity(targets.len());
        for (offset, target_name) in targets.iter().enumerate() {
            let target_node = self.nodes.get(target_name).ok_or_else(|| {
                ReactError::Agent(Box::new(AgentError::InitializationFailed(format!(
                    "Parallel node '{target_name}' not found in graph '{}'",
                    self.name
                ))))
            })?;
            let branch_state = state.fork()?;
            branch_state.set_current_node(target_name);
            branches.push((target_name.clone(), target_node, branch_state));
            Self::emit_event(
                events,
                WorkflowEvent::NodeStart {
                    node_name: target_name.clone(),
                    step_index: step_count.saturating_add(offset),
                },
            )?;
        }
        let source_run = control
            .run_id()
            .ok_or_else(|| ReactError::Other("Graph fanout has no bound run identity".into()))?;
        let source_attempt = control
            .attempt_id
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let mut branch_results = vec![false; branches.len()];
        let mut pending = FuturesUnordered::new();
        for (index, (target_name, node, branch_state)) in branches.iter().enumerate() {
            let control = control.clone();
            let target_name = target_name.clone();
            let source_run = source_run.clone();
            let source_attempt = source_attempt.clone();
            pending.push(async move {
                let started = Instant::now();
                // Consume the panicked future once. Its own Rust locals may
                // already be unwound; that is not closure proof for its peer.
                let result = match std::panic::AssertUnwindSafe(self.execute_node(
                    &target_name,
                    node,
                    branch_state,
                    events,
                    control,
                ))
                .catch_unwind()
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(ReactError::ExecutionNotSettled {
                        run_id: source_run,
                        execution_id: source_attempt,
                    }),
                };
                (index, target_name, started.elapsed(), result)
            });
        }
        let mut uncertainty = None;
        while let Some((index, target_name, elapsed, result)) = pending.next().await {
            match result {
                Ok(true) => {
                    let completed = branch_results.get_mut(index).ok_or_else(|| {
                        ReactError::Other(
                            "Parallel branch index escaped its result set".to_string(),
                        )
                    })?;
                    *completed = true;
                    if let Err(error) = Self::emit_event(
                        events,
                        WorkflowEvent::NodeEnd {
                            node_name: target_name,
                            step_index: step_count.saturating_add(index),
                            elapsed,
                        },
                    ) {
                        control.record_error(None, error);
                    }
                }
                Ok(false) => {}
                Err(error @ ReactError::ExecutionNotSettled { .. }) => {
                    uncertainty = Some(error);
                    control.cancel.cancel();
                }
                Err(error) => control.record_error(Some(&target_name), error),
            }
        }
        drop(pending);
        if let Some(error) = uncertainty {
            return Err(error);
        }
        if control.has_error() {
            return Ok(false);
        }
        for ((target_name, _, branch_state), completed) in branches.into_iter().zip(branch_results)
        {
            if !completed {
                return Err(ReactError::Agent(Box::new(AgentError::Cancelled(
                    "parallel branch did not settle".to_string(),
                ))));
            }
            state.deep_merge(&branch_state)?;
            path.push(target_name);
            *step_count = step_count.saturating_add(1);
        }
        Ok(true)
    }

    /// Terminal marker node name
    pub const END: &'static str = "__end__";

    /// Set the maximum number of execution steps
    pub fn set_max_steps(&mut self, max: usize) {
        self.max_steps = max;
    }

    /// Attach a cancellation token for cooperative cancellation between nodes.
    ///
    /// When the token is cancelled, the graph will stop at the next node boundary
    /// and return an `AgentError::Cancelled` error. This allows long-running
    /// workflows to be cancelled gracefully between node executions.
    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = Some(token);
        self
    }

    /// Check if the cancellation token has been triggered.
    fn is_cancelled(&self) -> bool {
        self.cancel_token
            .as_ref()
            .map(|t| t.is_cancelled())
            .unwrap_or(false)
    }

    fn fresh_execution(
        &self,
        state: SharedState,
        control: Arc<GraphExecutionControl>,
    ) -> GraphExecution {
        GraphExecution {
            control,
            state,
            current: self.entry.clone(),
            path: Vec::new(),
            step_count: 0,
            continuation: WorkflowContinuation::Node,
            parent_checkpoint: None,
            skip_interrupt_before_once: false,
        }
    }

    async fn complete_execution(
        &self,
        execution: GraphExecution,
        workflow_start: Instant,
        events: Option<&UnboundedSender<WorkflowEvent>>,
    ) -> Result<RunUntilInterruptResult> {
        info!(
            graph = %self.name,
            steps = execution.step_count,
            path = ?execution.path,
            "Graph execution completed"
        );
        let final_result = execution
            .state
            .get::<String>("result")
            .or_else(|| execution.state.get::<String>("output"))
            .unwrap_or_default();
        Self::emit_event(
            events,
            WorkflowEvent::Completed {
                result: final_result,
                total_steps: execution.step_count,
                elapsed: workflow_start.elapsed(),
            },
        )?;
        Ok(RunUntilInterruptResult::Completed(GraphResult {
            state: execution.state,
            path: execution.path,
            steps: execution.step_count,
        }))
    }

    /// Canonical Graph execution authority used by run, interrupt, resume, and stream entry points.
    async fn execute_loop(
        &self,
        mut execution: GraphExecution,
        honor_interrupts: bool,
        events: Option<UnboundedSender<WorkflowEvent>>,
    ) -> Result<GraphLoopOutcome> {
        let workflow_start = Instant::now();
        info!(graph = %self.name, entry = %execution.current, "Starting graph execution");

        loop {
            if self.is_cancelled() || execution.control.cancel.is_cancelled() {
                warn!(graph = %self.name, steps = execution.step_count, "Graph execution cancelled");
                execution.control.record_error(
                    None,
                    ReactError::Agent(Box::new(AgentError::Cancelled(format!(
                        "Graph '{}' cancelled after {} steps",
                        self.name, execution.step_count
                    )))),
                );
                return Ok(GraphLoopOutcome::CauseRecorded);
            }

            let continuation =
                std::mem::replace(&mut execution.continuation, WorkflowContinuation::Node);
            match continuation {
                WorkflowContinuation::Node => {}
                WorkflowContinuation::FanOut { targets, then } => {
                    debug!(
                        graph = %self.name,
                        targets = ?targets,
                        then = %then,
                        "Executing parallel fan-out"
                    );
                    if !self
                        .execute_parallel_route(
                            &execution.state,
                            &targets,
                            &mut execution.path,
                            &mut execution.step_count,
                            events.as_ref(),
                            execution.control.clone(),
                        )
                        .await?
                    {
                        return Ok(GraphLoopOutcome::CauseRecorded);
                    }
                    execution.current = then;
                    continue;
                }
                WorkflowContinuation::End => {
                    execution.current = Self::END.to_string();
                }
            }

            if execution.current == Self::END {
                return self
                    .complete_execution(execution, workflow_start, events.as_ref())
                    .await
                    .map(|result| GraphLoopOutcome::Settled(Box::new(result)));
            }

            if execution.step_count >= self.max_steps {
                warn!(
                    graph = %self.name,
                    steps = execution.step_count,
                    "Graph execution exceeded max steps"
                );
                return Err(ReactError::Agent(Box::new(
                    AgentError::MaxIterationsExceeded(self.max_steps),
                )));
            }

            if honor_interrupts
                && !execution.skip_interrupt_before_once
                && self
                    .interrupt_config
                    .should_interrupt_before(&execution.current)
            {
                debug!(graph = %self.name, node = %execution.current, "Interrupt before node");
                let checkpoint = self.checkpoint(
                    execution.current.clone(),
                    &execution.state,
                    execution.path.clone(),
                    execution.step_count,
                    InterruptType::BeforeNode,
                    WorkflowContinuation::Node,
                )?;
                let checkpoint = match execution.parent_checkpoint.as_ref() {
                    Some(parent) => checkpoint.continue_run(parent),
                    None => {
                        let mut checkpoint = checkpoint;
                        if let Some(run_id) = execution.control.run_id() {
                            checkpoint.workflow_run_id = run_id;
                        }
                        checkpoint
                    }
                };
                self.checkpoint_store.save(&checkpoint).await?;
                return Ok(GraphLoopOutcome::Settled(Box::new(
                    RunUntilInterruptResult::Interrupted(InterruptState::before_node(
                        checkpoint,
                        execution.current,
                    )),
                )));
            }
            execution.skip_interrupt_before_once = false;

            let node = self.nodes.get(&execution.current).ok_or_else(|| {
                ReactError::Agent(Box::new(AgentError::InitializationFailed(format!(
                    "Node '{}' not found in graph '{}'",
                    execution.current, self.name
                ))))
            })?;
            execution.state.set_current_node(&execution.current);
            Self::emit_event(
                events.as_ref(),
                WorkflowEvent::NodeStart {
                    node_name: execution.current.clone(),
                    step_index: execution.step_count,
                },
            )?;
            let node_start = Instant::now();
            if !self
                .execute_node(
                    &execution.current,
                    node,
                    &execution.state,
                    events.as_ref(),
                    execution.control.clone(),
                )
                .await?
            {
                return Ok(GraphLoopOutcome::CauseRecorded);
            }
            Self::emit_event(
                events.as_ref(),
                WorkflowEvent::NodeEnd {
                    node_name: execution.current.clone(),
                    step_index: execution.step_count,
                    elapsed: node_start.elapsed(),
                },
            )?;
            execution.path.push(execution.current.clone());
            execution.step_count = execution.step_count.saturating_add(1);

            let next = if self.finish_nodes.contains(&execution.current) {
                NextStep::End
            } else {
                self.resolve_next(&execution.current, &execution.state)
                    .await?
            };

            if honor_interrupts
                && self
                    .interrupt_config
                    .should_interrupt_after(&execution.current)
            {
                debug!(graph = %self.name, node = %execution.current, "Interrupt after node");
                let (next_node, continuation) = match next {
                    NextStep::Single(name) => (name, WorkflowContinuation::Node),
                    NextStep::Parallel { targets, then } => {
                        (then.clone(), WorkflowContinuation::FanOut { targets, then })
                    }
                    NextStep::End => (Self::END.to_string(), WorkflowContinuation::End),
                };
                let checkpoint = self.checkpoint(
                    next_node,
                    &execution.state,
                    execution.path.clone(),
                    execution.step_count,
                    InterruptType::AfterNode,
                    continuation,
                )?;
                let checkpoint = match execution.parent_checkpoint.as_ref() {
                    Some(parent) => checkpoint.continue_run(parent),
                    None => {
                        let mut checkpoint = checkpoint;
                        if let Some(run_id) = execution.control.run_id() {
                            checkpoint.workflow_run_id = run_id;
                        }
                        checkpoint
                    }
                };
                self.checkpoint_store.save(&checkpoint).await?;
                return Ok(GraphLoopOutcome::Settled(Box::new(
                    RunUntilInterruptResult::Interrupted(InterruptState::after_node(
                        checkpoint,
                        execution.current,
                    )),
                )));
            }

            match next {
                NextStep::Single(name) => execution.current = name,
                NextStep::Parallel { targets, then } => {
                    execution.current = then.clone();
                    execution.continuation = WorkflowContinuation::FanOut { targets, then };
                }
                NextStep::End => execution.current = Self::END.to_string(),
            }
        }
    }

    /// Execute the graph workflow.
    ///
    /// Starting from the entry node, execute nodes in sequence following edge routing logic, until reaching a finish node or `__end__`.
    pub async fn run(&self, state: SharedState) -> Result<GraphResult> {
        match self.run_owned(state, false).await? {
            RunUntilInterruptResult::Completed(result) => Ok(result),
            _ => Err(ReactError::Other(
                "Non-interrupting graph execution returned a non-terminal outcome".to_string(),
            )),
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
        self.run_owned(state, true).await
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
        self.resume_with_updates(checkpoint, decision, None).await
    }

    async fn resume_with_updates(
        &self,
        checkpoint_reference: Checkpoint,
        decision: ApprovalDecision,
        state_updates: Option<std::collections::HashMap<String, Value>>,
    ) -> Result<RunUntilInterruptResult> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            ReactError::Other(format!("Graph execution requires a Tokio runtime: {error}"))
        })?;
        let owner = self.retained_definition();
        let control = GraphExecutionControl::new(None);
        let original_control = control.clone();
        let task = runtime.spawn(async move {
            owner
                .resume_owned(
                    checkpoint_reference,
                    decision,
                    state_updates,
                    original_control,
                )
                .await
        });
        Self::observe_owned(task, control).await
    }

    async fn resume_owned(
        &self,
        checkpoint_reference: Checkpoint,
        decision: ApprovalDecision,
        state_updates: Option<HashMap<String, Value>>,
        control: Arc<GraphExecutionControl>,
    ) -> Result<GraphLoopOutcome> {
        if matches!(decision, ApprovalDecision::Deferred) {
            let checkpoint = self
                .checkpoint_store
                .load(&checkpoint_reference.id)
                .await?
                .ok_or_else(|| {
                    ReactError::Other("Checkpoint is missing or consumed".to_string())
                })?;
            self.validate_checkpoint(&checkpoint)?;
            info!(graph = %self.name, checkpoint_id = %checkpoint.id, "Resume deferred");
            return Ok(GraphLoopOutcome::Settled(Box::new(
                RunUntilInterruptResult::Deferred(Self::interrupt_state(checkpoint)),
            )));
        }

        if !matches!(decision, ApprovalDecision::Rejected { .. })
            && !self.supports_protected_claims()
        {
            return Err(ReactError::Other(
                "Checkpoint store does not provide protected claims".into(),
            ));
        }
        if !self.checkpoint_store.supports_claim_settlement() {
            return Err(ReactError::Other(
                "Checkpoint store does not provide atomic claim settlement".to_string(),
            ));
        }

        let persisted = self
            .checkpoint_store
            .load(&checkpoint_reference.id)
            .await?
            .ok_or_else(|| ReactError::Other("Checkpoint is missing or consumed".to_string()))?;
        self.validate_checkpoint(&persisted)?;
        let checkpoint = self
            .checkpoint_store
            .claim(&checkpoint_reference.id)
            .await?
            .ok_or_else(|| {
                ReactError::Other("Checkpoint is missing or already claimed".to_string())
            })?;
        let attempt_id = checkpoint.resume_attempt_id.clone().ok_or_else(|| {
            ReactError::Other(
                "Checkpoint store returned a claim without a resume attempt identity".to_string(),
            )
        })?;
        *control
            .attempt_id
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(attempt_id.clone());
        if let Err(error) = self.validate_checkpoint(&checkpoint) {
            if let Err(requeue_error) = self
                .checkpoint_store
                .requeue_claim(&checkpoint.id, &attempt_id)
                .await
            {
                if matches!(requeue_error, ReactError::ExecutionNotSettled { .. }) {
                    return Err(requeue_error);
                }
                return Err(ReactError::Other(format!(
                    "Checkpoint validation failed: {error}; checkpoint requeue failed: {requeue_error}"
                )));
            }
            return Err(error);
        }

        // The won claim, not the earlier load, is the execution authority.
        control.bind_run(&checkpoint.workflow_run_id);
        if !matches!(decision, ApprovalDecision::Rejected { .. }) {
            self.checkpoint_store
                .protect_claim(&checkpoint.id, &attempt_id)
                .await?;
        }
        let result = self
            .resume_claimed_with_heartbeat(
                checkpoint.clone(),
                decision,
                state_updates,
                &attempt_id,
                control.clone(),
            )
            .await;
        match result {
            Ok(GraphLoopOutcome::CauseRecorded) => {
                if let Err(error) = self
                    .checkpoint_store
                    .requeue_claim(&checkpoint.id, &attempt_id)
                    .await
                {
                    if matches!(error, ReactError::ExecutionNotSettled { .. }) {
                        return Err(error);
                    }
                    // Preserve the first typed business cause while keeping the
                    // auxiliary failure visible; neither error is a retry receipt.
                    warn!(checkpoint_id = %checkpoint.id, attempt_id = %attempt_id, error = %error,
                        "Checkpoint requeue failed after the original workflow cause");
                }
                Ok(GraphLoopOutcome::CauseRecorded)
            }
            Ok(result) => {
                self.checkpoint_store
                    .ack_claim(&checkpoint.id, &attempt_id)
                    .await?;
                Ok(result)
            }
            Err(error @ ReactError::ExecutionNotSettled { .. }) => Err(error),
            Err(error) => {
                if let Err(requeue_error) = self
                    .checkpoint_store
                    .requeue_claim(&checkpoint.id, &attempt_id)
                    .await
                {
                    if matches!(requeue_error, ReactError::ExecutionNotSettled { .. }) {
                        return Err(requeue_error);
                    }
                    return Err(ReactError::Other(format!(
                        "Workflow resume failed: {error}; checkpoint requeue failed: {requeue_error}"
                    )));
                }
                Err(error)
            }
        }
    }

    async fn resume_claimed_with_heartbeat(
        &self,
        checkpoint: Checkpoint,
        decision: ApprovalDecision,
        state_updates: Option<std::collections::HashMap<String, Value>>,
        attempt_id: &str,
        control: Arc<GraphExecutionControl>,
    ) -> Result<GraphLoopOutcome> {
        let Some(interval) = self.checkpoint_store.claim_heartbeat_interval() else {
            return self
                .resume_claimed(checkpoint, decision, state_updates, control.clone())
                .await;
        };
        if interval.is_zero() {
            return Err(ReactError::Other(
                "Checkpoint claim heartbeat interval must be greater than zero".to_string(),
            ));
        }

        let checkpoint_id = checkpoint.id.clone();
        let resume = self.resume_claimed(checkpoint, decision, state_updates, control.clone());
        tokio::pin!(resume);
        let first_tick = tokio::time::Instant::now()
            .checked_add(interval)
            .ok_or_else(|| ReactError::Other("Checkpoint heartbeat deadline overflow".into()))?;
        let mut heartbeat = tokio::time::interval_at(first_tick, interval);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;
                result = &mut resume => break result,
                _ = heartbeat.tick() => {
                    if let Err(error) = self
                        .checkpoint_store
                        .renew_claim(&checkpoint_id, attempt_id)
                        .await
                    {
                        if matches!(error, ReactError::ExecutionNotSettled { .. }) {
                            // Store knowledge is a different physical authority.
                            // A Node terminal cannot discharge this observation.
                            control.cancel.cancel();
                            let _ = resume.await;
                            break Err(error);
                        }
                        control.record_error(None, ReactError::Other(format!("Checkpoint claim heartbeat failed: {error}")));
                        // Retain the exact same pinned resume future. Protection
                        // survives restart; cancellation alone is not closure.
                        break resume.await;
                    }
                }
            }
        }
    }

    async fn resume_claimed(
        &self,
        checkpoint: Checkpoint,
        decision: ApprovalDecision,
        state_updates: Option<std::collections::HashMap<String, Value>>,
        control: Arc<GraphExecutionControl>,
    ) -> Result<GraphLoopOutcome> {
        if let ApprovalDecision::Rejected { reason } = decision {
            info!(
                graph = %self.name,
                checkpoint_id = %checkpoint.id,
                reason = reason.as_deref().unwrap_or("no reason"),
                "Resume rejected, aborting workflow"
            );
            return Ok(GraphLoopOutcome::Settled(Box::new(
                RunUntilInterruptResult::Rejected {
                    state: checkpoint.restore_state()?,
                    path: checkpoint.path,
                    steps: checkpoint.step_count,
                    reason,
                },
            )));
        }

        let state = checkpoint.restore_state()?;
        if let Some(updates) = state_updates {
            for (key, value) in updates {
                state.set(key, value)?;
            }
        }
        info!(
            graph = %self.name,
            checkpoint_id = %checkpoint.id,
            node = %checkpoint.current_node,
            "Resuming from checkpoint"
        );
        let skip_interrupt_before_once = checkpoint.interrupt_type == InterruptType::BeforeNode
            && matches!(checkpoint.continuation, WorkflowContinuation::Node);
        let execution = GraphExecution {
            control,
            state,
            current: checkpoint.current_node.clone(),
            path: checkpoint.path.clone(),
            step_count: checkpoint.step_count,
            continuation: checkpoint.continuation.clone(),
            parent_checkpoint: Some(checkpoint),
            skip_interrupt_before_once,
        };
        self.execute_loop(execution, true, None).await
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
        self.resume_with_updates(checkpoint, ApprovalDecision::Approved, Some(state_updates))
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
        if !self.supports_protected_claims() {
            return Err(ReactError::Other(
                "Checkpoint store does not provide protected claims".into(),
            ));
        }
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
        )
        .bind_execution(self.graph_revision.clone(), parent_cp.continuation.clone());

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

        let expected_generation = checkpoint.generation;

        if let Some(l) = label {
            checkpoint.label = Some(l.to_string());
        }
        if !tags.is_empty() {
            checkpoint.tags = tags.iter().map(|t| t.to_string()).collect();
        }
        checkpoint.generation = expected_generation.saturating_add(1);

        let committed = self
            .checkpoint_store
            .save_if_generation(&checkpoint, expected_generation)
            .await?;
        if committed {
            Ok(())
        } else {
            Err(ReactError::Other(format!(
                "Checkpoint '{checkpoint_id}' changed or was claimed while tagging"
            )))
        }
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
    pub async fn list_checkpoints_by_graph(
        &self,
    ) -> Result<Vec<super::checkpoint_store::CheckpointInfo>> {
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
        let owner = self.retained_definition();
        #[cfg(test)]
        let before_stream_observation = self.before_stream_observation.clone();
        let stream = async_stream::try_stream! {
            let runtime = tokio::runtime::Handle::try_current().map_err(|error| ReactError::Other(format!("Graph execution requires a Tokio runtime: {error}")))?;
            let control = GraphExecutionControl::new(Some(uuid::Uuid::new_v4().to_string()));
            let execution = owner.fresh_execution(state, control.clone());
            let (events, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let stream_guard = control.cancel.clone().drop_guard();
            let task = runtime.spawn(async move { owner.execute_loop(execution, false, Some(events)).await });
            #[cfg(test)]
            if let Some(gate) = before_stream_observation { gate.notified().await; }
            let observed = Self::observe_owned(task, control);
            tokio::pin!(observed);
            enum Poll { Event(Option<WorkflowEvent>), Outcome(Box<Result<RunUntilInterruptResult>>) }
            loop {
                let poll = tokio::select! {
                    biased;
                    event = receiver.recv() => Poll::Event(event),
                    result = &mut observed => Poll::Outcome(Box::new(result)),
                };
                match poll {
                    Poll::Event(Some(event)) => yield event,
                    Poll::Event(None) => { let _ = observed.await?; break; }
                    Poll::Outcome(result) => {
                        while let Ok(event) = receiver.try_recv() { yield event; }
                        let result = *result;
                        let _ = result?;
                        break;
                    }
                }
            }
            let _ = stream_guard.disarm();
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
                return Err(ReactError::Agent(Box::new(
                    AgentError::InitializationFailed(format!(
                        "Node '{}' has no outgoing edges and is not a finish node",
                        current
                    )),
                )));
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
    use crate::workflow::checkpoint_store::FileCheckpointStore;
    use echo_core::agent::AgentEvent;
    use futures::stream;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct StreamingAgent;

    impl Agent for StreamingAgent {
        fn name(&self) -> &str {
            "streaming-agent"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        fn system_prompt(&self) -> &str {
            "test"
        }

        fn execute<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Ok("streamed answer".to_string()) })
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async {
                Ok(Box::pin(stream::iter([
                    Ok(AgentEvent::Token("streamed ".to_string())),
                    Ok(AgentEvent::Token("answer".to_string())),
                    Ok(AgentEvent::FinalAnswer("streamed answer".to_string())),
                ])) as BoxStream<'a, Result<AgentEvent>>)
            })
        }
    }

    #[derive(Clone, Default)]
    struct CancellationProbeAgent {
        started: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
    }

    impl CancellationProbeAgent {
        async fn wait_for(&self, flag: &AtomicBool) -> Result<()> {
            tokio::time::timeout(Duration::from_secs(1), async {
                while !flag.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .map_err(|_| ReactError::Other("cancellation probe timed out".to_string()))
        }

        async fn wait_started(&self) -> Result<()> {
            self.wait_for(&self.started).await
        }

        async fn wait_cancelled(&self) -> Result<()> {
            self.wait_for(&self.cancelled).await
        }
    }

    impl Agent for CancellationProbeAgent {
        fn name(&self) -> &str {
            "cancellation-probe"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        fn system_prompt(&self) -> &str {
            "test"
        }

        fn execute<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(std::future::pending())
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async {
                Err(ReactError::Other(
                    "workflow must use the cancellable Agent stream".to_string(),
                ))
            })
        }

        fn execute_stream_with_cancel<'a>(
            &'a self,
            _task: &'a str,
            cancel: CancellationToken,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            let started = self.started.clone();
            let cancelled = self.cancelled.clone();
            Box::pin(async move {
                started.store(true, Ordering::SeqCst);
                let producer = tokio::spawn(async move {
                    cancel.cancelled().await;
                    cancelled.store(true, Ordering::SeqCst);
                });
                // The original producer actually closes after cancellation;
                // EOF must join it rather than use a never-ending placeholder.
                let events: BoxStream<'a, Result<AgentEvent>> = Box::pin(async_stream::stream! {
                    yield Ok(AgentEvent::Token("ready".to_string()));
                    if let Err(error) = producer.await { yield Err(ReactError::Other(format!("cancellation probe producer failed: {error}"))); }
                });
                Ok(events)
            })
        }
    }

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
    async fn parallel_failure_cancels_and_drains_siblings() {
        let graph = GraphBuilder::new("parallel-failure")
            .add_function_node("start", |_state: &SharedState| Box::pin(async { Ok(()) }))
            .add_function_node("fail", |_state: &SharedState| {
                Box::pin(async { Err(ReactError::Other("branch failed".to_string())) })
            })
            .add_function_node("slow", |_state: &SharedState| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok(())
                })
            })
            .add_function_node("merge", |_state: &SharedState| Box::pin(async { Ok(()) }))
            .set_entry("start")
            .add_parallel_edge("start", vec!["fail".into(), "slow".into()], "merge")
            .set_finish("merge")
            .build()
            .unwrap();

        let result = tokio::time::timeout(Duration::from_secs(1), graph.run(SharedState::new()))
            .await
            .unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn resume_after_parallel_source_executes_and_merges_all_branches() -> Result<()> {
        let graph = GraphBuilder::new("parallel-resume")
            .add_function_node("start", |state: &SharedState| {
                Box::pin(async move {
                    state
                        .set("input", "hello")
                        .map_err(|error| ReactError::Other(error.to_string()))
                })
            })
            .add_function_node("upper", |state: &SharedState| {
                Box::pin(async move {
                    let input = state.get::<String>("input").ok_or_else(|| {
                        ReactError::Other("parallel branch lost input".to_string())
                    })?;
                    state
                        .set("upper", input.to_uppercase())
                        .map_err(|error| ReactError::Other(error.to_string()))
                })
            })
            .add_function_node("length", |state: &SharedState| {
                Box::pin(async move {
                    let input = state.get::<String>("input").ok_or_else(|| {
                        ReactError::Other("parallel branch lost input".to_string())
                    })?;
                    state
                        .set("length", input.chars().count())
                        .map_err(|error| ReactError::Other(error.to_string()))
                })
            })
            .add_function_node("merge", |state: &SharedState| {
                Box::pin(async move {
                    let upper = state
                        .get::<String>("upper")
                        .ok_or_else(|| ReactError::Other("upper branch was skipped".to_string()))?;
                    let length = state.get::<usize>("length").ok_or_else(|| {
                        ReactError::Other("length branch was skipped".to_string())
                    })?;
                    state
                        .set("result", format!("{upper}:{length}"))
                        .map_err(|error| ReactError::Other(error.to_string()))
                })
            })
            .set_entry("start")
            .add_parallel_edge(
                "start",
                vec!["upper".to_string(), "length".to_string()],
                "merge",
            )
            .set_finish("merge")
            .interrupt_after(vec!["start"])
            .build()?;

        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await? {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            RunUntilInterruptResult::Completed(_)
            | RunUntilInterruptResult::Deferred(_)
            | RunUntilInterruptResult::Rejected { .. } => {
                return Err(ReactError::Other(
                    "graph did not interrupt after parallel source".to_string(),
                ));
            }
        };
        assert!(matches!(
            checkpoint.continuation,
            WorkflowContinuation::FanOut { .. }
        ));

        let result = match graph.resume(checkpoint, ApprovalDecision::Approved).await? {
            RunUntilInterruptResult::Completed(result) => result,
            RunUntilInterruptResult::Interrupted(_) => {
                return Err(ReactError::Other("resume interrupted twice".to_string()));
            }
            RunUntilInterruptResult::Deferred(_) | RunUntilInterruptResult::Rejected { .. } => {
                return Err(ReactError::Other("resume did not complete".to_string()));
            }
        };
        assert_eq!(
            result.state.get::<String>("result").as_deref(),
            Some("HELLO:5")
        );
        assert_eq!(result.path, vec!["start", "upper", "length", "merge"]);
        Ok(())
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
    async fn run_stream_forwards_agent_tokens_and_commits_final_answer() -> Result<()> {
        let graph = GraphBuilder::new("stream-agent-token")
            .add_agent_node("agent", StreamingAgent, "task", "result")
            .set_entry("agent")
            .set_finish("agent")
            .build()?;
        let state = SharedState::new();
        state.set("task", "write")?;

        let mut events = graph.run_stream(state.clone()).await?;
        let mut tokens = Vec::new();
        while let Some(event) = events.next().await {
            if let WorkflowEvent::Token { node_name, token } = event? {
                tokens.push((node_name, token));
            }
        }

        assert_eq!(
            tokens,
            vec![
                ("agent".to_string(), "streamed ".to_string()),
                ("agent".to_string(), "answer".to_string()),
            ]
        );
        assert_eq!(
            state.get::<String>("result"),
            Some("streamed answer".to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn dropping_workflow_stream_cancels_agent_producer() -> Result<()> {
        let probe = CancellationProbeAgent::default();
        let graph = GraphBuilder::new("stream-drop-cancel")
            .add_agent_node("agent", probe.clone(), "task", "result")
            .set_entry("agent")
            .set_finish("agent")
            .build()?;
        let mut events = graph.run_stream(SharedState::new()).await?;

        assert!(matches!(
            events.next().await,
            Some(Ok(WorkflowEvent::NodeStart { node_name, .. })) if node_name == "agent"
        ));
        assert!(matches!(
            events.next().await,
            Some(Ok(WorkflowEvent::Token { node_name, token }))
                if node_name == "agent" && token == "ready"
        ));
        drop(events);

        probe.wait_cancelled().await
    }

    #[tokio::test]
    async fn graph_cancel_cancels_non_streaming_agent_producer() -> Result<()> {
        let probe = CancellationProbeAgent::default();
        let graph_cancel = CancellationToken::new();
        let graph = GraphBuilder::new("graph-cancel-producer")
            .add_agent_node("agent", probe.clone(), "task", "result")
            .set_entry("agent")
            .set_finish("agent")
            .build()?
            .with_cancel_token(graph_cancel.clone());
        let cancel_after_start = async {
            probe.wait_started().await?;
            graph_cancel.cancel();
            Result::<()>::Ok(())
        };

        let (run_result, cancel_result) =
            tokio::join!(graph.run(SharedState::new()), cancel_after_start,);
        cancel_result?;
        assert!(run_result.is_err());
        probe.wait_cancelled().await
    }

    #[tokio::test]
    async fn node_timeout_cancels_non_streaming_agent_producer() -> Result<()> {
        let probe = CancellationProbeAgent::default();
        let graph = GraphBuilder::new("node-timeout-producer")
            .add_agent_node("agent", probe.clone(), "task", "result")
            .set_entry("agent")
            .set_finish("agent")
            .node_timeout(Duration::from_millis(10))
            .build()?;

        let result = graph.run(SharedState::new()).await;
        assert!(matches!(result, Err(ReactError::Other(message)) if message.contains("timed out")));
        probe.wait_cancelled().await
    }

    #[tokio::test]
    async fn parallel_failure_cancels_agent_producer() -> Result<()> {
        let probe = CancellationProbeAgent::default();
        let graph = GraphBuilder::new("parallel-agent-cancel")
            .add_function_node("start", |_state: &SharedState| Box::pin(async { Ok(()) }))
            .add_agent_node("slow", probe.clone(), "task", "slow_result")
            .add_function_node("broken", |_state: &SharedState| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    Err(ReactError::Other("branch failed".to_string()))
                })
            })
            .add_function_node("merge", |_state: &SharedState| Box::pin(async { Ok(()) }))
            .set_entry("start")
            .add_parallel_edge("start", vec!["slow".into(), "broken".into()], "merge")
            .set_finish("merge")
            .build()?;

        let result = graph.run(SharedState::new()).await;
        assert!(matches!(result, Err(error) if error.to_string().contains("branch failed")));
        probe.wait_cancelled().await
    }

    #[tokio::test]
    async fn finish_interrupt_resumes_to_the_same_completed_path_as_run() -> Result<()> {
        let build_graph = || {
            GraphBuilder::new("finish-interrupt-parity")
                .add_function_node("start", |_state: &SharedState| Box::pin(async { Ok(()) }))
                .add_function_node("finish", |state: &SharedState| {
                    Box::pin(async move {
                        state
                            .set("result", "done")
                            .map_err(|error| ReactError::Other(error.to_string()))
                    })
                })
                .set_entry("start")
                .add_edge("start", "finish")
                .set_finish("finish")
                .interrupt_after(vec!["finish"])
                .build()
        };

        let direct = build_graph()?.run(SharedState::new()).await?;
        assert_eq!(direct.path, vec!["start", "finish"]);

        let graph = build_graph()?;
        let interrupted = graph.run_until_interrupt(SharedState::new()).await?;
        let checkpoint = match interrupted {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            other => {
                return Err(ReactError::Other(format!(
                    "expected finish-node interrupt, got {other:?}"
                )));
            }
        };
        assert_eq!(checkpoint.path, direct.path);
        assert_eq!(checkpoint.continuation, WorkflowContinuation::End);

        let resumed = graph.resume(checkpoint, ApprovalDecision::Approved).await?;
        let completed = match resumed {
            RunUntilInterruptResult::Completed(result) => result,
            other => {
                return Err(ReactError::Other(format!(
                    "expected completed resume, got {other:?}"
                )));
            }
        };
        assert_eq!(completed.path, direct.path);
        assert_eq!(completed.steps, direct.steps);
        Ok(())
    }

    #[tokio::test]
    async fn run_stream_emits_parallel_node_error_before_failed_terminal_item() -> Result<()> {
        let graph = GraphBuilder::new("parallel-stream-error")
            .add_function_node("start", |_state: &SharedState| Box::pin(async { Ok(()) }))
            .add_function_node("broken", |_state: &SharedState| {
                Box::pin(async { Err(ReactError::Other("branch failed".to_string())) })
            })
            .add_function_node("slow", |_state: &SharedState| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok(())
                })
            })
            .add_function_node("merge", |_state: &SharedState| Box::pin(async { Ok(()) }))
            .set_entry("start")
            .add_parallel_edge("start", vec!["broken".into(), "slow".into()], "merge")
            .set_finish("merge")
            .build()?;

        let mut stream = graph.run_stream(SharedState::new()).await?;
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item);
        }

        assert!(items.iter().any(|item| {
            matches!(
                item,
                Ok(WorkflowEvent::NodeError { node_name, error })
                    if node_name == "broken" && error.contains("branch failed")
            )
        }));
        assert!(
            matches!(items.last(), Some(Err(error)) if error.to_string().contains("branch failed"))
        );
        Ok(())
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
    async fn run_stream_emits_node_error_before_failed_terminal_item() {
        use super::WorkflowEvent;
        use futures::StreamExt;

        let graph = GraphBuilder::new("stream-error")
            .add_function_node("broken", |_state: &SharedState| {
                Box::pin(async { Err(ReactError::Other("broken node".to_string())) })
            })
            .set_entry("broken")
            .set_finish("broken")
            .build()
            .unwrap();

        let mut stream = graph.run_stream(SharedState::new()).await.unwrap();
        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            events.push(item);
        }

        assert!(matches!(
            events.first(),
            Some(Ok(WorkflowEvent::NodeStart { node_name, .. })) if node_name == "broken"
        ));
        assert!(events.iter().any(|item| matches!(
            item,
            Ok(WorkflowEvent::NodeError { node_name, error })
                if node_name == "broken" && error.contains("broken node")
        )));
        assert!(events.iter().any(Result::is_err));
        assert!(
            !events
                .iter()
                .any(|item| matches!(item, Ok(WorkflowEvent::Completed { .. })))
        );
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
            _ => return,
        };

        assert_eq!(graph.list_checkpoints().await.unwrap().len(), 1);

        let mut updates = HashMap::new();
        updates.insert("message".to_string(), Value::String("patched".to_string()));

        let resumed = graph.resume_with_state(checkpoint, updates).await.unwrap();
        let result = match resumed {
            RunUntilInterruptResult::Completed(result) => result,
            _ => return,
        };

        let seen: String = result.state.get("result").unwrap_or_default();
        assert_eq!(seen, "seen=patched");
        assert!(
            graph.list_checkpoints().await.unwrap().is_empty(),
            "checkpoint should be deleted after successful resume"
        );
    }

    fn interrupting_graph(name: &str, revision: &str, store: Arc<MemoryCheckpointStore>) -> Graph {
        GraphBuilder::new(name)
            .add_function_node("start", |state: &SharedState| {
                Box::pin(async move {
                    state
                        .set(
                            "executions",
                            state
                                .get::<u64>("executions")
                                .unwrap_or(0)
                                .saturating_add(1),
                        )
                        .map_err(Into::into)
                })
            })
            .set_entry("start")
            .set_finish("start")
            .interrupt_before(vec!["start"])
            .revision(revision)
            .build()
            .unwrap_or_else(|error| panic!("test graph must build: {error}"))
            .with_checkpoint_store(store)
    }

    #[tokio::test]
    async fn checkpoint_is_claimed_once() {
        let store = Arc::new(MemoryCheckpointStore::new());
        let graph = interrupting_graph("claim-once", "v1", store);
        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await {
            Ok(RunUntilInterruptResult::Interrupted(interrupt)) => interrupt.checkpoint,
            other => panic!("expected interrupt, got {other:?}"),
        };
        let replay = checkpoint.clone();
        assert!(matches!(
            graph.resume(checkpoint, ApprovalDecision::Approved).await,
            Ok(RunUntilInterruptResult::Completed(_))
        ));
        assert!(
            graph
                .resume(replay, ApprovalDecision::Approved)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn cross_graph_and_revision_drift_do_not_consume_checkpoint() {
        let store = Arc::new(MemoryCheckpointStore::new());
        let source = interrupting_graph("source", "v1", store.clone());
        let checkpoint = match source.run_until_interrupt(SharedState::new()).await {
            Ok(RunUntilInterruptResult::Interrupted(interrupt)) => interrupt.checkpoint,
            other => panic!("expected interrupt, got {other:?}"),
        };
        let wrong_graph = interrupting_graph("other", "v1", store.clone());
        assert!(
            wrong_graph
                .resume(checkpoint.clone(), ApprovalDecision::Approved)
                .await
                .is_err()
        );
        let wrong_revision = interrupting_graph("source", "v2", store.clone());
        assert!(
            wrong_revision
                .resume(checkpoint.clone(), ApprovalDecision::Approved)
                .await
                .is_err()
        );
        assert!(matches!(
            source.resume(checkpoint, ApprovalDecision::Approved).await,
            Ok(RunUntilInterruptResult::Completed(_))
        ));
    }

    #[tokio::test]
    async fn deferred_preserves_and_rejected_consumes_checkpoint() {
        let store = Arc::new(MemoryCheckpointStore::new());
        let graph = interrupting_graph("decisions", "v1", store);
        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await {
            Ok(RunUntilInterruptResult::Interrupted(interrupt)) => interrupt.checkpoint,
            other => panic!("expected interrupt, got {other:?}"),
        };
        assert!(matches!(
            graph
                .resume(checkpoint.clone(), ApprovalDecision::Deferred)
                .await,
            Ok(RunUntilInterruptResult::Deferred(_))
        ));
        assert!(matches!(
            graph
                .resume(
                    checkpoint.clone(),
                    ApprovalDecision::Rejected {
                        reason: Some("user rejected".to_string()),
                    },
                )
                .await,
            Ok(RunUntilInterruptResult::Rejected { .. })
        ));
        assert!(
            graph
                .resume(checkpoint, ApprovalDecision::Approved)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn physical_fanout_panic_retains_other_original_branch() -> Result<()> {
        use futures::FutureExt;
        use std::sync::atomic::AtomicUsize;
        use tokio::sync::{Mutex, Notify};
        struct BranchDrop(Arc<AtomicUsize>);
        impl Drop for BranchDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let directory =
            std::env::temp_dir().join(format!("echo_fanout_owner_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let effect = directory.join("effect.txt");
        let release = Arc::new(Notify::new());
        let entered = Arc::new(AtomicBool::new(false));
        let drops = Arc::new(AtomicUsize::new(0));
        let slot = Arc::new(Mutex::new(None));
        let node_release = release.clone();
        let node_entered = entered.clone();
        let node_effect = effect.clone();
        let node_drops = drops.clone();
        let node_slot = slot.clone();
        let panic_entered = entered.clone();
        let store = Arc::new(MemoryCheckpointStore::new());
        let graph = Arc::new(
            GraphBuilder::new("fanout-panic-physical")
                .add_function_node("start", |_| Box::pin(async { Ok(()) }))
                .add_function_node("held", move |_| {
                    let release = node_release.clone();
                    let entered = node_entered.clone();
                    let effect = node_effect.clone();
                    let drops = node_drops.clone();
                    let slot = node_slot.clone();
                    Box::pin(async move {
                        let _scope = BranchDrop(drops);
                        let backend = tokio::spawn(async move {
                            std::fs::write(&effect, "entered")
                                .map_err(|error| error.to_string())?;
                            entered.store(true, Ordering::SeqCst);
                            release.notified().await;
                            std::fs::write(&effect, "closed").map_err(|error| error.to_string())?;
                            Ok::<(), String>(())
                        })
                        .map(|result| {
                            result
                                .map_err(|error| error.to_string())
                                .and_then(|result| result)
                        })
                        .boxed()
                        .shared();
                        *slot.lock().await = Some(backend.clone());
                        backend.await.map_err(ReactError::Other)
                    })
                })
                .add_function_node("panic", move |_| {
                    let entered = panic_entered.clone();
                    Box::pin(async move {
                        while !entered.load(Ordering::SeqCst) {
                            tokio::task::yield_now().await;
                        }
                        std::panic::resume_unwind(Box::new("controlled branch poll loss"))
                    })
                })
                .add_function_node("merge", |_| Box::pin(async { Ok(()) }))
                .set_entry("start")
                .add_parallel_edge("start", vec!["held".into(), "panic".into()], "merge")
                .set_finish("merge")
                .interrupt_after(vec!["start"])
                .revision("fanout-owner-v1")
                .build()?
                .with_checkpoint_store(store.clone()),
        );
        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await? {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            other => {
                return Err(ReactError::Other(format!(
                    "expected fanout interrupt, got {other:?}"
                )));
            }
        };
        let observer_graph = graph.clone();
        let observer_checkpoint = checkpoint.clone();
        let mut observer = tokio::spawn(async move {
            observer_graph
                .resume(observer_checkpoint, ApprovalDecision::Approved)
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("held original branch never started".into()))?;
        let early = tokio::time::timeout(Duration::from_millis(20), &mut observer).await;
        let pending = early.is_err();
        let before_release_drops = drops.load(Ordering::SeqCst);
        let held = std::fs::read_to_string(&effect).ok();
        let original_attempt = store
            .load(&checkpoint.id)
            .await?
            .and_then(|claim| claim.resume_attempt_id);
        release.notify_one();
        let cleanup = match slot.lock().await.take() {
            Some(backend) => backend.await.map_err(ReactError::Other),
            None => Err(ReactError::Other("missing original branch join".into())),
        };
        let result = match early {
            Ok(result) => result,
            Err(_) => observer.await,
        }
        .map_err(|error| ReactError::Other(format!("borrowed resume join: {error}")))?;
        let replacement = store.claim(&checkpoint.id).await?;
        let closed = std::fs::read_to_string(&effect).ok();
        let _ = std::fs::remove_dir_all(&directory);
        cleanup?;
        assert_eq!(held.as_deref(), Some("entered"));
        assert_eq!(closed.as_deref(), Some("closed"));
        assert!(
            pending,
            "one panicked branch destroyed/settled its still-live sibling: {result:?}"
        );
        assert_eq!(before_release_drops, 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(
            matches!(result, Err(ReactError::ExecutionNotSettled { ref run_id, ref execution_id }) if run_id == &checkpoint.workflow_run_id && execution_id == &original_attempt)
        );
        assert!(
            replacement.is_none(),
            "panic is not a physical receipt for the original protected attempt"
        );
        Ok(())
    }

    struct ControlledMemoryStore {
        inner: Arc<MemoryCheckpointStore>,
        protected: bool,
        claims: std::sync::atomic::AtomicUsize,
        saves: std::sync::atomic::AtomicUsize,
        load_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
        first_load: AtomicBool,
        fail_heartbeat: bool,
        heartbeat_observation: bool,
        requeue_observation: bool,
        requeue_error: bool,
        requeues: std::sync::atomic::AtomicUsize,
        heartbeat_failed: Arc<tokio::sync::Notify>,
    }

    impl ControlledMemoryStore {
        fn new(protected: bool) -> Self {
            Self {
                inner: Arc::new(MemoryCheckpointStore::new()),
                protected,
                claims: std::sync::atomic::AtomicUsize::new(0),
                saves: std::sync::atomic::AtomicUsize::new(0),
                load_gate: None,
                first_load: AtomicBool::new(true),
                fail_heartbeat: false,
                heartbeat_observation: false,
                requeue_observation: false,
                requeue_error: false,
                requeues: std::sync::atomic::AtomicUsize::new(0),
                heartbeat_failed: Arc::new(tokio::sync::Notify::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl CheckpointStore for ControlledMemoryStore {
        async fn save(&self, checkpoint: &Checkpoint) -> Result<()> {
            self.saves.fetch_add(1, Ordering::SeqCst);
            self.inner.save(checkpoint).await
        }
        async fn load(&self, id: &str) -> Result<Option<Checkpoint>> {
            let result = self.inner.load(id).await?;
            if let Some((entered, release)) = &self.load_gate
                && self.first_load.swap(false, Ordering::SeqCst)
            {
                entered.notify_one();
                release.notified().await;
            }
            Ok(result)
        }
        async fn claim(&self, id: &str) -> Result<Option<Checkpoint>> {
            self.claims.fetch_add(1, Ordering::SeqCst);
            self.inner.claim(id).await
        }
        async fn ack_claim(&self, id: &str, attempt: &str) -> Result<()> {
            self.inner.ack_claim(id, attempt).await
        }
        async fn requeue_claim(&self, id: &str, attempt: &str) -> Result<()> {
            self.requeues.fetch_add(1, Ordering::SeqCst);
            if self.requeue_error {
                return Err(ReactError::Other(
                    "controlled auxiliary requeue failure".into(),
                ));
            }
            if self.requeue_observation {
                return Err(ReactError::ExecutionNotSettled {
                    run_id: "original-store-run".into(),
                    execution_id: Some("original-store-attempt".into()),
                });
            }
            self.inner.requeue_claim(id, attempt).await
        }
        async fn renew_claim(&self, id: &str, attempt: &str) -> Result<()> {
            if self.fail_heartbeat {
                self.heartbeat_failed.notify_one();
                if self.heartbeat_observation {
                    return Err(ReactError::ExecutionNotSettled {
                        run_id: "original-store-run".into(),
                        execution_id: Some("original-store-attempt".into()),
                    });
                }
                return Err(ReactError::Other("controlled heartbeat failure".into()));
            }
            self.inner.renew_claim(id, attempt).await
        }
        fn claim_heartbeat_interval(&self) -> Option<Duration> {
            self.fail_heartbeat.then_some(Duration::from_millis(5))
        }
        fn supports_claim_settlement(&self) -> bool {
            true
        }
        fn supports_protected_claims(&self) -> bool {
            self.protected
        }
        async fn protect_claim(&self, id: &str, attempt: &str) -> Result<()> {
            self.inner.protect_claim(id, attempt).await
        }
        async fn save_if_generation(
            &self,
            checkpoint: &Checkpoint,
            generation: u64,
        ) -> Result<bool> {
            self.inner.save_if_generation(checkpoint, generation).await
        }
        async fn list(&self) -> Result<Vec<super::super::checkpoint_store::CheckpointInfo>> {
            self.inner.list().await
        }
        async fn delete(&self, id: &str) -> Result<()> {
            self.inner.delete(id).await
        }
        async fn clear(&self) -> Result<()> {
            self.inner.clear().await
        }
    }

    struct InvocationIdentityProbe(
        Arc<std::sync::Mutex<Vec<echo_core::tools::ExternalRunContext>>>,
    );
    impl Agent for InvocationIdentityProbe {
        fn name(&self) -> &str {
            "workflow-invocation-identity"
        }
        fn model_name(&self) -> &str {
            "test"
        }
        fn system_prompt(&self) -> &str {
            "test"
        }
        fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async {
                Err(ReactError::Other(
                    "must use original context-aware entry".into(),
                ))
            })
        }
        fn execute_stream<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async {
                Err(ReactError::Other(
                    "must use original context-aware stream".into(),
                ))
            })
        }
        fn execute_stream_with_invocation_context<'a>(
            &'a self,
            _: &'a str,
            _: CancellationToken,
            invocation: echo_core::agent::AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async move {
                let runtime = invocation.runtime.ok_or_else(|| {
                    ReactError::Other("missing original invocation context".into())
                })?;
                self.0
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(runtime);
                Ok(Box::pin(stream::iter([Ok(AgentEvent::FinalAnswer(
                    "one original invocation".into(),
                ))])) as BoxStream<'a, Result<AgentEvent>>)
            })
        }
    }

    #[tokio::test]
    async fn won_checkpoint_claim_binds_identity_after_concurrent_pending_replacement() -> Result<()>
    {
        let loaded = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let mut controlled = ControlledMemoryStore::new(true);
        controlled.load_gate = Some((loaded.clone(), release.clone()));
        let store = Arc::new(controlled);
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let graph = Arc::new(
            GraphBuilder::new("won-claim-identity")
                .add_agent_node(
                    "effect",
                    InvocationIdentityProbe(seen.clone()),
                    "task",
                    "result",
                )
                .set_entry("effect")
                .set_finish("effect")
                .interrupt_before(vec!["effect"])
                .revision("identity-v1")
                .build()?
                .with_checkpoint_store(store.clone()),
        );
        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await? {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            other => {
                return Err(ReactError::Other(format!(
                    "expected checkpoint, got {other:?}"
                )));
            }
        };
        let observer_graph = graph.clone();
        let reference = checkpoint.clone();
        let observer = tokio::spawn(async move {
            observer_graph
                .resume(reference, ApprovalDecision::Approved)
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), loaded.notified())
            .await
            .map_err(|_| ReactError::Other("original load did not reach barrier".into()))?;
        let mut replacement = checkpoint.clone();
        replacement.workflow_run_id = uuid::Uuid::new_v4().to_string();
        replacement.generation = replacement.generation.saturating_add(1);
        // This is a real legal pending save on the original Memory authority,
        // not a Store inventing another claim response.
        store.inner.save(&replacement).await?;
        release.notify_one();
        let result = observer
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let contexts = seen.lock().unwrap_or_else(|error| error.into_inner());
        assert!(matches!(result, Ok(RunUntilInterruptResult::Completed(_))));
        assert_eq!(contexts.len(), 1);
        let context = contexts
            .first()
            .ok_or_else(|| ReactError::Other("original Agent was not invoked".into()))?;
        assert_eq!(
            context.run_id.as_deref(),
            Some(replacement.workflow_run_id.as_str())
        );
        assert_ne!(
            context.run_id.as_deref(),
            Some(checkpoint.workflow_run_id.as_str())
        );
        assert!(
            context
                .turn_id
                .as_ref()
                .is_some_and(|turn| Some(turn) != context.run_id.as_ref())
        );
        assert!(
            context
                .execution_id
                .as_ref()
                .is_some_and(|execution| Some(execution) != context.run_id.as_ref()
                    && Some(execution) != context.turn_id.as_ref())
        );
        Ok(())
    }

    #[tokio::test]
    async fn unsupported_protection_rejects_executable_resume_and_branch_before_mutation()
    -> Result<()> {
        let store = Arc::new(ControlledMemoryStore::new(false));
        let effects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let node_effects = effects.clone();
        let graph = GraphBuilder::new("unsupported-protection")
            .add_function_node("effect", move |_| {
                let effects = node_effects.clone();
                Box::pin(async move {
                    effects.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
            .set_entry("effect")
            .set_finish("effect")
            .interrupt_before(vec!["effect"])
            .revision("capability-v1")
            .build()?
            .with_checkpoint_store(store.clone());
        assert!(!graph.supports_protected_claims());
        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await? {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            other => {
                return Err(ReactError::Other(format!(
                    "expected fresh interrupt, got {other:?}"
                )));
            }
        };
        let saves = store.saves.load(Ordering::SeqCst);
        assert!(
            graph
                .resume(checkpoint.clone(), ApprovalDecision::Approved)
                .await
                .is_err()
        );
        assert!(
            graph
                .resume_with_state(checkpoint.clone(), HashMap::new())
                .await
                .is_err()
        );
        assert!(
            graph
                .branch_from(&checkpoint.id, HashMap::new(), Some("not-saved".into()))
                .await
                .is_err()
        );
        assert_eq!(store.claims.load(Ordering::SeqCst), 0);
        assert_eq!(store.saves.load(Ordering::SeqCst), saves);
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        assert!(matches!(
            graph
                .resume(checkpoint.clone(), ApprovalDecision::Deferred)
                .await,
            Ok(RunUntilInterruptResult::Deferred(_))
        ));
        assert_eq!(store.claims.load(Ordering::SeqCst), 0);
        assert!(matches!(
            graph
                .resume(
                    checkpoint,
                    ApprovalDecision::Rejected {
                        reason: Some("not executed".into())
                    }
                )
                .await,
            Ok(RunUntilInterruptResult::Rejected { .. })
        ));
        assert_eq!(store.claims.load(Ordering::SeqCst), 1);
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        // Fresh execution still has no protected-Store capability requirement.
        assert!(graph.run(SharedState::new()).await.is_ok());
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn first_buffered_graph_stream_yield_drop_signals_original_owner() -> Result<()> {
        use std::sync::atomic::AtomicUsize;
        use tokio::sync::{Mutex, Notify};
        type Backend = futures::future::Shared<BoxFuture<'static, std::result::Result<(), String>>>;
        struct SourceDrop(Arc<AtomicUsize>);
        impl Drop for SourceDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        struct BufferedSource {
            entered: Arc<Notify>,
            cancelled: Arc<Notify>,
            release: Arc<Notify>,
            join: Arc<Mutex<Option<Backend>>>,
            drops: Arc<AtomicUsize>,
            path: std::path::PathBuf,
        }
        impl Agent for BufferedSource {
            fn name(&self) -> &str {
                "first-yield-source"
            }
            fn model_name(&self) -> &str {
                "test"
            }
            fn system_prompt(&self) -> &str {
                "test"
            }
            fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String>> {
                Box::pin(async {
                    Err(ReactError::Other(
                        "requires original cancellable stream".into(),
                    ))
                })
            }
            fn execute_stream<'a>(
                &'a self,
                _: &'a str,
            ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
                Box::pin(async {
                    Err(ReactError::Other(
                        "requires original cancellation scope".into(),
                    ))
                })
            }
            fn execute_stream_with_cancel<'a>(
                &'a self,
                _: &'a str,
                cancel: CancellationToken,
            ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
                let entered = self.entered.clone();
                let cancelled = self.cancelled.clone();
                let release = self.release.clone();
                let join = self.join.clone();
                let drops = self.drops.clone();
                let path = self.path.clone();
                Box::pin(async move {
                    let source = async_stream::stream! {
                        let _guard = SourceDrop(drops);
                        let backend_entered = Arc::new(Notify::new()); let ready = backend_entered.clone();
                        let backend = tokio::spawn(async move {
                            std::fs::write(&path, "entered").map_err(|error| error.to_string())?; ready.notify_one();
                            release.notified().await;
                            std::fs::write(&path, "closed").map_err(|error| error.to_string())?; Ok::<(), String>(())
                        }).map(|result| result.map_err(|error| error.to_string()).and_then(|result| result)).boxed().shared();
                        *join.lock().await = Some(backend.clone()); backend_entered.notified().await;
                        entered.notify_one();
                        let result = tokio::select! {
                            () = cancel.cancelled() => { cancelled.notify_one(); backend.await }
                            result = backend.clone() => result,
                        };
                        match result { Ok(()) => yield Ok(AgentEvent::FinalAnswer("closed original source".into())), Err(error) => yield Err(ReactError::Other(error)) }
                    };
                    Ok(Box::pin(source) as BoxStream<'a, Result<AgentEvent>>)
                })
            }
        }
        let directory =
            std::env::temp_dir().join(format!("echo_first_yield_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("effect.txt");
        let entered = Arc::new(Notify::new());
        let cancelled = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let join = Arc::new(Mutex::new(None));
        let drops = Arc::new(AtomicUsize::new(0));
        let agent = BufferedSource {
            entered: entered.clone(),
            cancelled: cancelled.clone(),
            release: release.clone(),
            join: join.clone(),
            drops: drops.clone(),
            path: path.clone(),
        };
        let mut graph = GraphBuilder::new("buffered-first-stream-yield")
            .add_agent_node("effect", agent, "task", "result")
            .set_entry("effect")
            .set_finish("effect")
            .build()?;
        // Controlled legal scheduling: original NodeStart is buffered and the
        // real source has entered before the first observer select is polled.
        graph.before_stream_observation = Some(entered);
        let mut source = graph.run_stream(SharedState::new()).await?;
        let first = tokio::time::timeout(Duration::from_secs(2), source.next()).await;
        drop(source);
        let signalled = tokio::time::timeout(Duration::from_millis(20), cancelled.notified())
            .await
            .is_ok();
        let held = std::fs::read_to_string(&path).ok();
        let early_drops = drops.load(Ordering::SeqCst);
        release.notify_one();
        let cleanup = match join.lock().await.take() {
            Some(backend) => backend.await.map_err(ReactError::Other),
            None => Err(ReactError::Other(
                "missing original buffered source backend".into(),
            )),
        };
        let tail = tokio::time::timeout(Duration::from_secs(2), async {
            while drops.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        let closed = std::fs::read_to_string(&path).ok();
        let _ = std::fs::remove_dir_all(&directory);
        cleanup?;
        assert!(matches!(
            first,
            Ok(Some(Ok(WorkflowEvent::NodeStart { .. })))
        ));
        assert_eq!(held.as_deref(), Some("entered"));
        assert_eq!(closed.as_deref(), Some("closed"));
        assert_eq!(early_drops, 0);
        assert!(
            signalled,
            "dropping the first buffered yield never signalled the original still-held source"
        );
        assert!(tail.is_ok());
        Ok(())
    }

    async fn assert_heartbeat_retains_original_source(store_observation: bool) -> Result<()> {
        use std::sync::atomic::AtomicUsize;
        use tokio::sync::{Mutex, Notify, Semaphore};
        struct SourceDrop(Arc<AtomicUsize>);
        impl Drop for SourceDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let directory =
            std::env::temp_dir().join(format!("echo_heartbeat_owner_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("effect.txt");
        let release = Arc::new(Notify::new());
        let entered = Arc::new(Notify::new());
        let drops = Arc::new(AtomicUsize::new(0));
        let capacity = Arc::new(Semaphore::new(1));
        let slot = Arc::new(Mutex::new(None));
        let effects = Arc::new(AtomicUsize::new(0));
        let mut controlled = ControlledMemoryStore::new(true);
        controlled.fail_heartbeat = true;
        controlled.heartbeat_observation = store_observation;
        let store = Arc::new(controlled);
        let node_release = release.clone();
        let node_entered = entered.clone();
        let node_drops = drops.clone();
        let node_capacity = capacity.clone();
        let node_slot = slot.clone();
        let node_path = path.clone();
        let node_effects = effects.clone();
        let graph = Arc::new(
            GraphBuilder::new("heartbeat-physical-owner")
                .add_function_node("effect", move |_| {
                    let release = node_release.clone();
                    let entered = node_entered.clone();
                    let drops = node_drops.clone();
                    let capacity = node_capacity.clone();
                    let slot = node_slot.clone();
                    let path = node_path.clone();
                    let effects = node_effects.clone();
                    Box::pin(async move {
                        let _permit = capacity
                            .acquire_owned()
                            .await
                            .map_err(|error| ReactError::Other(error.to_string()))?;
                        let _scope = SourceDrop(drops);
                        effects.fetch_add(1, Ordering::SeqCst);
                        let backend = tokio::spawn(async move {
                            std::fs::write(&path, "entered").map_err(|error| error.to_string())?;
                            entered.notify_one();
                            release.notified().await;
                            std::fs::write(&path, "closed").map_err(|error| error.to_string())?;
                            Ok::<(), String>(())
                        })
                        .map(|result| {
                            result
                                .map_err(|error| error.to_string())
                                .and_then(|result| result)
                        })
                        .boxed()
                        .shared();
                        *slot.lock().await = Some(backend.clone());
                        backend.await.map_err(ReactError::Other)
                    })
                })
                .set_entry("effect")
                .set_finish("effect")
                .interrupt_before(vec!["effect"])
                .revision("heartbeat-v1")
                .build()?
                .with_checkpoint_store(store.clone()),
        );
        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await? {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            other => {
                return Err(ReactError::Other(format!(
                    "expected checkpoint, got {other:?}"
                )));
            }
        };
        let observer_graph = graph.clone();
        let reference = checkpoint.clone();
        let mut observer = tokio::spawn(async move {
            observer_graph
                .resume(reference, ApprovalDecision::Approved)
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .map_err(|_| ReactError::Other("original heartbeat node never entered".into()))?;
        let original_attempt = store
            .inner
            .load(&checkpoint.id)
            .await?
            .and_then(|claim| claim.resume_attempt_id);
        tokio::time::timeout(Duration::from_secs(2), store.heartbeat_failed.notified())
            .await
            .map_err(|_| ReactError::Other("original renewal never failed".into()))?;
        let early = tokio::time::timeout(Duration::from_millis(20), &mut observer).await;
        let pending = early.is_err();
        let held = std::fs::read_to_string(&path).ok();
        let early_drops = drops.load(Ordering::SeqCst);
        let early_permits = capacity.available_permits();
        let replacement = store.inner.claim(&checkpoint.id).await?;
        release.notify_one();
        let cleanup = match slot.lock().await.take() {
            Some(backend) => backend.await.map_err(ReactError::Other),
            None => Err(ReactError::Other(
                "missing original heartbeat backend join".into(),
            )),
        };
        let result = match early {
            Ok(result) => result,
            Err(_) => observer.await,
        }
        .map_err(|error| ReactError::Other(error.to_string()))?;
        let after = store.inner.load(&checkpoint.id).await?;
        let closed = std::fs::read_to_string(&path).ok();
        let _ = std::fs::remove_dir_all(&directory);
        cleanup?;
        assert_eq!(held.as_deref(), Some("entered"));
        assert_eq!(closed.as_deref(), Some("closed"));
        assert!(
            pending && early_drops == 0 && early_permits == 0 && replacement.is_none(),
            "heartbeat error was treated as physical closure: pending={pending}, drops={early_drops}, permits={early_permits}, replacement={replacement:?}, result={result:?}"
        );
        assert!(original_attempt.is_some());
        if store_observation {
            assert!(
                matches!(result, Err(ReactError::ExecutionNotSettled { ref run_id, execution_id: Some(ref execution_id) }) if run_id=="original-store-run" && execution_id=="original-store-attempt"),
                "Store observation lost: {result:?}"
            );
            assert_eq!(
                after.and_then(|claim| claim.resume_attempt_id),
                original_attempt
            );
        } else {
            assert!(
                matches!(result, Err(ReactError::Other(ref message)) if message.contains("Checkpoint claim heartbeat failed: controlled heartbeat failure"))
            );
            assert!(
                after.is_some_and(|claim| claim.resume_attempt_id.is_none()),
                "known physical error must keep its ordinary requeue policy"
            );
        }
        assert_eq!(capacity.available_permits(), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn heartbeat_failure_retains_original_node_claim_and_permit_until_physical_close()
    -> Result<()> {
        assert_heartbeat_retains_original_source(false).await
    }
    #[tokio::test]
    async fn store_observation_heartbeat_is_not_a_node_closure_or_business_failure() -> Result<()> {
        assert_heartbeat_retains_original_source(true).await
    }

    #[tokio::test]
    async fn store_observation_requeue_preserves_original_typed_identity() -> Result<()> {
        let mut controlled = ControlledMemoryStore::new(true);
        controlled.requeue_observation = true;
        let store = Arc::new(controlled);
        let effects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let node_effects = effects.clone();
        let mut graph = GraphBuilder::new("store-requeue-observation")
            .add_function_node("effect", move |_| {
                let effects = node_effects.clone();
                Box::pin(async move {
                    effects.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
            .set_entry("effect")
            .set_finish("effect")
            .interrupt_before(vec!["effect"])
            .revision("store-error-v1")
            .build()?
            .with_checkpoint_store(store.clone());
        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await? {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            other => {
                return Err(ReactError::Other(format!(
                    "expected checkpoint, got {other:?}"
                )));
            }
        };
        graph.set_max_steps(0);
        let result = graph
            .resume(checkpoint.clone(), ApprovalDecision::Approved)
            .await;
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        assert!(
            store
                .inner
                .load(&checkpoint.id)
                .await?
                .is_some_and(|claim| claim.resume_attempt_id.is_some())
        );
        assert!(
            matches!(result,Err(ReactError::ExecutionNotSettled { ref run_id,execution_id:Some(ref execution_id) }) if run_id=="original-store-run" && execution_id=="original-store-attempt"),
            "Store observation was flattened: {result:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn store_observation_ordinary_requeue_error_keeps_original_cause_and_claim() -> Result<()>
    {
        let mut controlled = ControlledMemoryStore::new(true);
        controlled.requeue_error = true;
        let store = Arc::new(controlled);
        let effects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let original_effects = effects.clone();
        let graph = GraphBuilder::new("ordinary-requeue-diagnostic")
            .add_function_node("effect", move |_| {
                let effects = original_effects.clone();
                Box::pin(async move {
                    effects.fetch_add(1, Ordering::SeqCst);
                    Err(ReactError::Agent(Box::new(AgentError::Cancelled(
                        "original Node cause".into(),
                    ))))
                })
            })
            .set_entry("effect")
            .set_finish("effect")
            .interrupt_before(vec!["effect"])
            .revision("ordinary-requeue-v1")
            .build()?
            .with_checkpoint_store(store.clone());
        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await? {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            other => {
                return Err(ReactError::Other(format!(
                    "expected checkpoint, got {other:?}"
                )));
            }
        };
        let result = graph
            .resume(checkpoint.clone(), ApprovalDecision::Approved)
            .await;
        assert!(
            matches!(result, Err(ReactError::Agent(ref error)) if matches!(error.as_ref(), AgentError::Cancelled(message) if message=="original Node cause")),
            "first business cause changed: {result:?}"
        );
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        assert_eq!(store.requeues.load(Ordering::SeqCst), 1);
        assert!(
            store
                .inner
                .load(&checkpoint.id)
                .await?
                .is_some_and(|claim| claim.resume_attempt_id.is_some())
        );
        assert!(store.inner.claim(&checkpoint.id).await?.is_none());
        Ok(())
    }

    async fn assert_workflow_live_source_settlement(mode: u8) -> Result<()> {
        use std::sync::atomic::AtomicUsize;
        use tokio::sync::{Mutex, Notify};
        type Backend = futures::future::Shared<BoxFuture<'static, std::result::Result<(), String>>>;
        struct SourceDrop(Arc<AtomicUsize>);
        impl Drop for SourceDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        struct LiveAgent {
            backend_release: Arc<Notify>,
            eof_release: Arc<Notify>,
            observed: Arc<Notify>,
            source_tail_ready: Arc<Notify>,
            slot: Arc<Mutex<Option<Backend>>>,
            drops: Arc<AtomicUsize>,
            calls: Arc<AtomicUsize>,
            identity: Arc<std::sync::Mutex<Option<(String, String)>>>,
            path: std::path::PathBuf,
            mode: u8,
        }
        impl Agent for LiveAgent {
            fn name(&self) -> &str {
                "matching-live-workflow-source"
            }
            fn model_name(&self) -> &str {
                "test"
            }
            fn system_prompt(&self) -> &str {
                "test"
            }
            fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String>> {
                Box::pin(async {
                    Err(ReactError::Other("must use original context stream".into()))
                })
            }
            fn execute_stream<'a>(
                &'a self,
                _: &'a str,
            ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
                Box::pin(async {
                    Err(ReactError::Other("must bind original Node identity".into()))
                })
            }
            fn execute_stream_with_invocation_context<'a>(
                &'a self,
                _: &'a str,
                _: CancellationToken,
                context: echo_core::agent::AgentInvocationContext,
            ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
                Box::pin(async move {
                    let runtime = context
                        .runtime
                        .ok_or_else(|| ReactError::Other("missing runtime".into()))?;
                    let run = runtime
                        .run_id
                        .ok_or_else(|| ReactError::Other("missing run".into()))?;
                    let execution = runtime
                        .execution_id
                        .ok_or_else(|| ReactError::Other("missing execution".into()))?;
                    *self
                        .identity
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) =
                        Some((run.clone(), execution.clone()));
                    self.calls.fetch_add(1, Ordering::SeqCst);
                    let backend_release = self.backend_release.clone();
                    let eof_release = self.eof_release.clone();
                    let observed = self.observed.clone();
                    let source_tail_ready = self.source_tail_ready.clone();
                    let slot = self.slot.clone();
                    let drops = self.drops.clone();
                    let path = self.path.clone();
                    let mode = self.mode;
                    let source = async_stream::stream! {
                        let _guard = SourceDrop(drops); let entered = Arc::new(Notify::new()); let ready = entered.clone();
                        let backend = tokio::spawn(async move {
                            std::fs::write(&path, "entered").map_err(|error| error.to_string())?; ready.notify_one(); backend_release.notified().await;
                            std::fs::write(&path, "closed").map_err(|error| error.to_string())?; Ok::<(),String>(())
                        }).map(|result| result.map_err(|error| error.to_string()).and_then(|result|result)).boxed().shared();
                        *slot.lock().await = Some(backend.clone()); entered.notified().await;
                        if mode == 3 {
                            yield Err(ReactError::ExecutionNotSettled {
                                run_id: "foreign-workflow-run".into(),
                                execution_id: Some("foreign-workflow-execution".into()),
                            });
                        }
                        yield Err(ReactError::ExecutionNotSettled { run_id: run.clone(), execution_id: Some(execution.clone()) }); observed.notify_one();
                        if let Err(error) = backend.await { yield Err(ReactError::Other(error)); return; }
                        if mode != 2 { yield Ok(AgentEvent::FinalAnswer("later genuine Node answer".into())); }
                        if mode == 4 {
                            yield Err(ReactError::ExecutionNotSettled { run_id: run, execution_id: Some(execution) });
                        }
                        source_tail_ready.notify_one(); eof_release.notified().await;
                    };
                    Ok(Box::pin(source) as BoxStream<'a, Result<AgentEvent>>)
                })
            }
        }
        let directory =
            std::env::temp_dir().join(format!("echo_workflow_live_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("effect.txt");
        let backend_release = Arc::new(Notify::new());
        let eof_release = Arc::new(Notify::new());
        let observed = Arc::new(Notify::new());
        let source_tail_ready = Arc::new(Notify::new());
        let slot = Arc::new(Mutex::new(None));
        let drops = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let identity = Arc::new(std::sync::Mutex::new(None));
        let agent = LiveAgent {
            backend_release: backend_release.clone(),
            eof_release: eof_release.clone(),
            observed: observed.clone(),
            source_tail_ready: source_tail_ready.clone(),
            slot: slot.clone(),
            drops: drops.clone(),
            calls: calls.clone(),
            identity: identity.clone(),
            path: path.clone(),
            mode,
        };
        let store = Arc::new(MemoryCheckpointStore::new());
        let cancel = CancellationToken::new();
        let state_probe = Arc::new(std::sync::Mutex::new(None::<SharedState>));
        let builder = GraphBuilder::new("live-original-workflow-source")
            .add_agent_node("effect", agent, "task", "result")
            .set_entry("effect")
            .set_finish("effect")
            .interrupt_before(vec!["effect"])
            .revision("live-source-v1");
        let builder = if mode >= 3 {
            let probe = state_probe.clone();
            // Observe the resumed Graph's actual state before the Agent node;
            // the original matching positive controls retain their old path.
            builder
                .add_function_node("capture-state", move |state: &SharedState| {
                    let probe = probe.clone();
                    let state = state.clone();
                    Box::pin(async move {
                        *probe.lock().unwrap_or_else(|error| error.into_inner()) = Some(state);
                        Ok(())
                    })
                })
                .set_entry("capture-state")
                .add_edge("capture-state", "effect")
                .interrupt_before(vec!["capture-state"])
        } else {
            builder
        };
        let graph = Arc::new(
            builder
                .build()?
                .with_checkpoint_store(store.clone())
                .with_cancel_token(cancel.clone()),
        );
        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await? {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            other => {
                return Err(ReactError::Other(format!(
                    "expected checkpoint, got {other:?}"
                )));
            }
        };
        let owner = graph.clone();
        let reference = checkpoint.clone();
        let mut observer =
            tokio::spawn(async move { owner.resume(reference, ApprovalDecision::Approved).await });
        let admitted = tokio::time::timeout(Duration::from_secs(2), observed.notified()).await;
        if mode == 1 {
            cancel.cancel();
        }
        let held = std::fs::read_to_string(&path).ok();
        let original_attempt = store
            .load(&checkpoint.id)
            .await?
            .and_then(|claim| claim.resume_attempt_id);
        let replacement_while_held = if mode >= 3 {
            store.claim(&checkpoint.id).await?
        } else {
            None
        };
        let held_output = if mode >= 3 {
            state_probe
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .map(|state| (state.contains("result"), state.messages().is_empty()))
        } else {
            None
        };
        let before_terminal_drops = drops.load(Ordering::SeqCst);
        backend_release.notify_one();
        let backend_cleanup = match slot.lock().await.take() {
            Some(backend) => backend.await.map_err(ReactError::Other),
            None => Err(ReactError::Other("missing original live backend".into())),
        };
        let tail_ready =
            tokio::time::timeout(Duration::from_secs(2), source_tail_ready.notified()).await;
        let premature = tokio::time::timeout(Duration::from_millis(20), &mut observer).await;
        let waits_for_eof = premature.is_err();
        let claim_before_eof = store
            .load(&checkpoint.id)
            .await?
            .and_then(|claim| claim.resume_attempt_id);
        let before_eof_drops = drops.load(Ordering::SeqCst);
        eof_release.notify_one();
        let result = match premature {
            Ok(result) => result,
            Err(_) => observer.await,
        }
        .map_err(|error| ReactError::Other(error.to_string()))?;
        let claim_after = store.load(&checkpoint.id).await?;
        let replacement = store.claim(&checkpoint.id).await?;
        let closed = std::fs::read_to_string(&path).ok();
        let _ = std::fs::remove_dir_all(&directory);
        backend_cleanup?;
        assert!(admitted.is_ok() && tail_ready.is_ok());
        assert_eq!(held.as_deref(), Some("entered"));
        assert_eq!(closed.as_deref(), Some("closed"));
        assert!(waits_for_eof);
        assert_eq!(before_terminal_drops, 0);
        assert_eq!(before_eof_drops, 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(original_attempt.is_some());
        assert_eq!(claim_before_eof, original_attempt);
        if mode >= 3 {
            assert!(replacement_while_held.is_none());
            assert_eq!(
                held_output,
                Some((false, true)),
                "resumed state must be observable and output-free while held"
            );
        }
        match mode {
            0 => {
                assert!(
                    matches!(result, Ok(RunUntilInterruptResult::Completed(ref result)) if result.state.get::<String>("result").as_deref()==Some("later genuine Node answer"))
                );
                assert!(claim_after.is_none());
                assert!(replacement.is_none());
            }
            1 => {
                assert!(
                    matches!(result, Err(ReactError::Agent(ref error)) if matches!(error.as_ref(), AgentError::Cancelled(_)))
                );
                assert!(claim_after.is_some_and(|claim| claim.resume_attempt_id.is_none()));
            }
            _ => {
                let expected = if mode == 3 {
                    Some((
                        "foreign-workflow-run".into(),
                        "foreign-workflow-execution".into(),
                    ))
                } else {
                    identity
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .clone()
                };
                assert!(
                    matches!(result, Err(ReactError::ExecutionNotSettled { ref run_id, execution_id: Some(ref execution_id) }) if expected.as_ref().is_some_and(|(run,execution)|run==run_id && execution==execution_id))
                );
                assert_eq!(
                    claim_after
                        .as_ref()
                        .and_then(|claim| claim.resume_attempt_id.clone()),
                    original_attempt
                );
                assert!(replacement.is_none());
                if mode >= 3 {
                    let state = state_probe
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .clone()
                        .ok_or_else(|| {
                            ReactError::Other("resumed state probe disappeared".into())
                        })?;
                    assert!(
                        !state.contains("result"),
                        "unknown source must not commit its answer"
                    );
                    assert!(
                        state.messages().is_empty(),
                        "unknown source must not append an assistant message"
                    );
                    let durable = claim_after
                        .as_ref()
                        .ok_or_else(|| {
                            ReactError::Other("protected checkpoint disappeared".into())
                        })?
                        .restore_state()?;
                    assert!(!durable.contains("result"));
                    assert!(durable.messages().is_empty());
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn workflow_live_source_matching_terminal_and_eof_settle_original_claim() -> Result<()> {
        assert_workflow_live_source_settlement(0).await
    }
    #[tokio::test]
    async fn workflow_live_source_late_terminal_keeps_first_cancellation_cause() -> Result<()> {
        assert_workflow_live_source_settlement(1).await
    }
    #[tokio::test]
    async fn workflow_live_source_eof_without_terminal_keeps_protected_claim() -> Result<()> {
        assert_workflow_live_source_settlement(2).await
    }
    #[tokio::test]
    async fn workflow_live_source_foreign_observation_survives_matching_progress_and_local_terminal()
    -> Result<()> {
        assert_workflow_live_source_settlement(3).await
    }
    #[tokio::test]
    async fn workflow_live_source_late_matching_unknown_requires_a_new_terminal() -> Result<()> {
        assert_workflow_live_source_settlement(4).await
    }

    async fn assert_graph_waiter_drop_retains_original_source(streaming: bool) -> Result<()> {
        use futures::FutureExt;
        use std::sync::atomic::AtomicUsize;
        use tokio::sync::{Mutex, Notify};
        struct SourceDrop(Arc<AtomicUsize>);
        impl Drop for SourceDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let directory =
            std::env::temp_dir().join(format!("echo_graph_owner_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let effect = directory.join("effect.txt");
        let release = Arc::new(Notify::new());
        let entered = Arc::new(Notify::new());
        let source_ready = Arc::new(Notify::new());
        let drops = Arc::new(AtomicUsize::new(0));
        let backend_join = Arc::new(Mutex::new(None));
        let node_release = release.clone();
        let node_entered = entered.clone();
        let node_ready = source_ready.clone();
        let node_effect = effect.clone();
        let node_drops = drops.clone();
        let node_join = backend_join.clone();
        let store = Arc::new(MemoryCheckpointStore::new());
        let graph = Arc::new(
            GraphBuilder::new("borrowed-graph-owner")
                .add_function_node("effect", move |_state: &SharedState| {
                    let release = node_release.clone();
                    let entered = node_entered.clone();
                    let source_ready = node_ready.clone();
                    let effect = node_effect.clone();
                    let drops = node_drops.clone();
                    let slot = node_join.clone();
                    Box::pin(async move {
                        let _scope = SourceDrop(drops);
                        let backend = tokio::spawn(async move {
                            std::fs::write(&effect, "entered")
                                .map_err(|error| error.to_string())?;
                            entered.notify_one();
                            release.notified().await;
                            std::fs::write(&effect, "closed").map_err(|error| error.to_string())?;
                            Ok::<(), String>(())
                        })
                        .map(|result| {
                            result
                                .map_err(|error| error.to_string())
                                .and_then(|result| result)
                        })
                        .boxed()
                        .shared();
                        *slot.lock().await = Some(backend.clone());
                        source_ready.notify_one();
                        backend.await.map_err(ReactError::Other)
                    })
                })
                .set_entry("effect")
                .set_finish("effect")
                .interrupt_before(vec!["effect"])
                .revision("owner-v1")
                .build()?
                .with_checkpoint_store(store.clone()),
        );
        let checkpoint = if streaming {
            None
        } else {
            match graph.run_until_interrupt(SharedState::new()).await? {
                RunUntilInterruptResult::Interrupted(interrupt) => Some(interrupt.checkpoint),
                other => {
                    return Err(ReactError::Other(format!(
                        "expected interrupt, got {other:?}"
                    )));
                }
            }
        };
        let observer_graph = graph.clone();
        let observer_checkpoint = checkpoint.clone();
        let observer = tokio::spawn(async move {
            if let Some(checkpoint) = observer_checkpoint {
                observer_graph
                    .resume(checkpoint, ApprovalDecision::Approved)
                    .await
                    .map(|_| ())
            } else {
                let mut stream = observer_graph.run_stream(SharedState::new()).await?;
                while let Some(event) = stream.next().await {
                    event?;
                }
                Ok(())
            }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            entered.notified().await;
            source_ready.notified().await;
        })
        .await
        .map_err(|_| ReactError::Other("original graph producer never entered".into()))?;
        let original_attempt = if let Some(checkpoint) = &checkpoint {
            store
                .load(&checkpoint.id)
                .await?
                .and_then(|checkpoint| checkpoint.resume_attempt_id)
        } else {
            None
        };
        observer.abort();
        let observer_result = observer.await;
        let before_release_drops = drops.load(Ordering::SeqCst);
        let held = std::fs::read_to_string(&effect).ok();
        release.notify_one();
        let backend = backend_join.lock().await.take();
        let cleanup = match backend {
            Some(backend) => backend.await.map_err(ReactError::Other),
            None => Err(ReactError::Other("missing original backend join".into())),
        };
        // Wait for the original owner/destructor tail, never re-execute.
        let tail = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let settled = if let Some(checkpoint) = &checkpoint {
                    store
                        .load(&checkpoint.id)
                        .await?
                        .is_none_or(|checkpoint| checkpoint.resume_attempt_id.is_none())
                } else {
                    true
                };
                if drops.load(Ordering::SeqCst) == 1 && settled {
                    break Ok::<(), ReactError>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        let closed = std::fs::read_to_string(&effect).ok();
        let _ = std::fs::remove_dir_all(&directory);
        cleanup?;
        assert!(observer_result.is_err_and(|error| error.is_cancelled()));
        assert_eq!(held.as_deref(), Some("entered"));
        assert_eq!(closed.as_deref(), Some("closed"));
        if !streaming {
            assert!(original_attempt.is_some());
        }
        assert_eq!(
            before_release_drops, 0,
            "borrowed observer/stream Drop destroyed the original Node future before its live backend closed"
        );
        tail.map_err(|_| {
            ReactError::Other(
                "same original Graph owner did not settle after physical closure".into(),
            )
        })??;
        Ok(())
    }

    #[tokio::test]
    async fn physical_graph_resume_waiter_drop_keeps_original_source() -> Result<()> {
        assert_graph_waiter_drop_retains_original_source(false).await
    }

    #[tokio::test]
    async fn physical_graph_stream_drop_keeps_original_source() -> Result<()> {
        assert_graph_waiter_drop_retains_original_source(true).await
    }

    async fn assert_unknown_resume_keeps_exact_claim(file_store: bool) -> Result<()> {
        use tokio::sync::{Mutex, Notify};
        let directory =
            std::env::temp_dir().join(format!("echo_workflow_physical_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let effect = directory.join("effect.txt");
        let store: Arc<dyn CheckpointStore> = if file_store {
            Arc::new(FileCheckpointStore::new(directory.join("checkpoints")))
        } else {
            Arc::new(MemoryCheckpointStore::new())
        };
        let release = Arc::new(Notify::new());
        let entered = Arc::new(Notify::new());
        let original_join = Arc::new(Mutex::new(None));
        let original_claim: Arc<Mutex<Option<Checkpoint>>> = Arc::new(Mutex::new(None));
        let callback_claim = original_claim.clone();
        let callback_join = original_join.clone();
        let callback_release = release.clone();
        let callback_entered = entered.clone();
        let callback_effect = effect.clone();
        let graph = GraphBuilder::new("protected-resume-observation")
            .add_function_node("effect", move |_state: &SharedState| {
                let claim = callback_claim.clone();
                let join = callback_join.clone();
                let release = callback_release.clone();
                let entered = callback_entered.clone();
                let effect = callback_effect.clone();
                Box::pin(async move {
                    let producer_entered = entered.clone();
                    *join.lock().await = Some(tokio::spawn(async move {
                        std::fs::write(&effect, "entered")?;
                        producer_entered.notify_one();
                        release.notified().await;
                        std::fs::write(&effect, "closed")?;
                        Ok::<(), ReactError>(())
                    }));
                    entered.notified().await;
                    let checkpoint = claim.lock().await.clone().ok_or_else(|| {
                        ReactError::Other("missing original checkpoint identity".into())
                    })?;
                    Err(ReactError::ExecutionNotSettled {
                        run_id: checkpoint.workflow_run_id,
                        execution_id: None,
                    })
                })
            })
            .set_entry("effect")
            .set_finish("effect")
            .interrupt_before(vec!["effect"])
            .revision("physical-v1")
            .build()?
            .with_checkpoint_store(store.clone());
        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await? {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            other => {
                return Err(ReactError::Other(format!(
                    "expected original interrupt, got {other:?}"
                )));
            }
        };
        *original_claim.lock().await = Some(checkpoint.clone());
        let result = graph
            .resume(checkpoint.clone(), ApprovalDecision::Approved)
            .await;
        // A new Store instance over the same directory tests real recovery, not
        // presence in list/load (which also expose active claims).
        let observer: Arc<dyn CheckpointStore> = if file_store {
            Arc::new(FileCheckpointStore::new(directory.join("checkpoints")))
        } else {
            store.clone()
        };
        let replacement = observer.claim(&checkpoint.id).await;
        let held = std::fs::read_to_string(&effect).ok();
        release.notify_one();
        let backend = original_join.lock().await.take();
        let cleanup = match backend {
            Some(backend) => backend
                .await
                .map_err(|error| ReactError::Other(format!("original backend join: {error}")))?,
            None => Err(ReactError::Other(
                "node never started its physical backend".into(),
            )),
        };
        let closed = std::fs::read_to_string(&effect).ok();
        let _ = std::fs::remove_dir_all(&directory);
        cleanup?;
        assert_eq!(held.as_deref(), Some("entered"));
        assert_eq!(closed.as_deref(), Some("closed"));
        assert!(
            matches!(result, Err(ReactError::ExecutionNotSettled { ref run_id, execution_id: None }) if run_id == &checkpoint.workflow_run_id),
            "original observation lost: {result:?}"
        );
        let replacement = replacement?;
        assert!(
            replacement.is_none(),
            "same physical effect is still held but checkpoint was reclaimed: {replacement:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn physical_unknown_resume_keeps_memory_claim() -> Result<()> {
        assert_unknown_resume_keeps_exact_claim(false).await
    }

    #[tokio::test]
    async fn physical_unknown_resume_keeps_file_claim_across_reopen() -> Result<()> {
        assert_unknown_resume_keeps_exact_claim(true).await
    }

    #[tokio::test]
    async fn failed_resume_requeues_the_claimed_checkpoint() {
        let store = Arc::new(MemoryCheckpointStore::new());
        let graph = GraphBuilder::new("resume-requeue")
            .add_function_node("start", |_state: &SharedState| {
                Box::pin(async { Err(ReactError::Other("node failed".to_string())) })
            })
            .set_entry("start")
            .set_finish("start")
            .interrupt_before(vec!["start"])
            .revision("v1")
            .build()
            .unwrap()
            .with_checkpoint_store(store.clone());

        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await.unwrap() {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            _ => return,
        };
        assert!(
            graph
                .resume(checkpoint, ApprovalDecision::Approved)
                .await
                .is_err()
        );

        let pending = store.list().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(store.load(&pending[0].id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn long_resume_renews_file_claim_and_blocks_tag_resurrection() -> Result<()> {
        let temp_path =
            std::env::temp_dir().join(format!("echo_graph_renew_{}", uuid::Uuid::new_v4()));
        let store = Arc::new(
            FileCheckpointStore::new(&temp_path)
                .with_claim_timing(Duration::from_millis(120), Duration::from_millis(30))?,
        );
        let observer = FileCheckpointStore::new(&temp_path)
            .with_claim_timing(Duration::from_millis(120), Duration::from_millis(30))?;
        let graph = Arc::new(
            GraphBuilder::new("renew-active-claim")
                .add_function_node("work", |_state: &SharedState| {
                    Box::pin(async {
                        tokio::time::sleep(Duration::from_millis(260)).await;
                        Ok(())
                    })
                })
                .set_entry("work")
                .set_finish("work")
                .interrupt_before(vec!["work"])
                .revision("v1")
                .build()?
                .with_checkpoint_store(store.clone()),
        );
        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await? {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            other => {
                return Err(ReactError::Other(format!(
                    "expected interrupt before long resume, got {other:?}"
                )));
            }
        };
        let checkpoint_id = checkpoint.id.clone();
        let resume_graph = graph.clone();
        let resume = tokio::spawn(async move {
            resume_graph
                .resume(checkpoint, ApprovalDecision::Approved)
                .await
        });

        tokio::time::sleep(Duration::from_millis(170)).await;
        assert!(observer.claim(&checkpoint_id).await?.is_none());
        assert!(
            observer
                .load(&checkpoint_id)
                .await?
                .is_some_and(|checkpoint| { checkpoint.resume_attempt_id.is_some() })
        );
        assert!(
            graph
                .tag_checkpoint(&checkpoint_id, Some("late"), vec!["stale"])
                .await
                .is_err()
        );

        let result = resume
            .await
            .map_err(|error| ReactError::Other(format!("resume task failed: {error}")))??;
        assert!(matches!(result, RunUntilInterruptResult::Completed(_)));
        assert!(store.load(&checkpoint_id).await?.is_none());
        let _ = std::fs::remove_dir_all(&temp_path);
        Ok(())
    }

    #[tokio::test]
    async fn tagging_claimed_checkpoint_fails_without_resurrection() {
        let store = Arc::new(MemoryCheckpointStore::new());
        let graph = interrupting_graph("tag-race", "v1", store.clone());
        let checkpoint = match graph.run_until_interrupt(SharedState::new()).await.unwrap() {
            RunUntilInterruptResult::Interrupted(interrupt) => interrupt.checkpoint,
            _ => return,
        };
        let claimed = store.claim(&checkpoint.id).await.unwrap().unwrap();
        assert!(
            graph
                .tag_checkpoint(&checkpoint.id, Some("late"), vec!["race"])
                .await
                .is_err()
        );
        assert!(store.load(&checkpoint.id).await.unwrap().is_some());
        let attempt_id = claimed.resume_attempt_id.as_deref().unwrap_or_default();
        store
            .requeue_claim(&checkpoint.id, attempt_id)
            .await
            .unwrap();
        let restored = store.load(&checkpoint.id).await.unwrap().unwrap();
        assert!(restored.label.is_none());
    }
}
