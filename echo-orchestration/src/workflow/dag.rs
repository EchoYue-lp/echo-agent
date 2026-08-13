//! DAG workflow: agents organized as a directed acyclic graph, executed in topological order with independent nodes running concurrently.

use super::{SharedAgent, StepOutput, Workflow, WorkflowOutput, shared_agent};
use echo_core::agent::Agent;
use echo_core::error::{AgentError, ReactError, Result};
use futures::future::BoxFuture;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;
use tracing::{debug, info};

/// A node in the DAG
pub struct DagNode {
    pub id: String,
    pub agent: SharedAgent,
}

/// Directed edge in the DAG (from -> to means to depends on from's output)
#[derive(Debug, Clone)]
pub struct DagEdge {
    pub from: String,
    pub to: String,
}

/// DAG workflow: nodes execute in topological order; independent nodes run concurrently.
///
/// Each node receives the concatenated outputs of all its predecessors (joined by newlines) as input;
/// root nodes with zero in-degree receive the workflow's original input.
///
/// # Example
///
/// ```rust,no_run
/// use echo_core::agent::{Agent, AgentEvent};
/// use echo_core::error::Result;
/// use echo_orchestration::workflow::{DagWorkflow, Workflow};
/// use futures::future::BoxFuture;
/// use futures::stream::{self, BoxStream};
///
/// # struct DummyAgent {
/// #     name: String,
/// # }
/// #
/// # impl DummyAgent {
/// #     fn new(name: impl Into<String>) -> Self {
/// #         Self { name: name.into() }
/// #     }
/// # }
/// #
/// # impl Agent for DummyAgent {
/// #     fn name(&self) -> &str { &self.name }
/// #     fn model_name(&self) -> &str { "mock-model" }
/// #     fn system_prompt(&self) -> &str { "You are a mock agent" }
/// #     fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
/// #         Box::pin(async move { Ok(format!("{}: {task}", self.name)) })
/// #     }
/// #     fn execute_stream<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
/// #         Box::pin(async move {
/// #             let s: BoxStream<'a, Result<AgentEvent>> = Box::pin(stream::empty());
/// #             Ok(s)
/// #         })
/// #     }
/// # }
///
/// # async fn example() -> Result<()> {
/// let researcher = DummyAgent::new("researcher");
/// let analyst = DummyAgent::new("analyst");
/// let writer = DummyAgent::new("writer");
///
/// let mut wf = DagWorkflow::builder()
///     .node("research", researcher)
///     .node("analyze", analyst)
///     .node("write", writer)
///     .edge("research", "write")
///     .edge("analyze", "write")
///     .build()?;
///
/// let output = wf.run("Analyze the 2025 AI Agent ecosystem").await?;
/// println!("{}", output.result);
/// # Ok(())
/// # }
/// ```
pub struct DagWorkflow {
    nodes: HashMap<String, SharedAgent>,
    edges: Vec<DagEdge>,
    node_order: Vec<String>,
}

impl DagWorkflow {
    pub fn builder() -> DagWorkflowBuilder {
        DagWorkflowBuilder {
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }
}

impl Workflow for DagWorkflow {
    fn run<'a>(&'a mut self, input: &'a str) -> BoxFuture<'a, Result<WorkflowOutput>> {
        Box::pin(async move {
            let total_start = Instant::now();
            let mut step_outputs: Vec<StepOutput> = Vec::new();
            let mut node_results: HashMap<String, String> = HashMap::new();

            let predecessors = build_predecessors(&self.edges);
            let successors = build_successors(&self.edges);
            let in_degree = compute_in_degree(&self.node_order, &self.edges);

            let mut remaining_in_degree = in_degree.clone();
            let mut ready: VecDeque<String> = VecDeque::new();

            for node_id in &self.node_order {
                if remaining_in_degree
                    .get(node_id.as_str())
                    .copied()
                    .unwrap_or_default()
                    == 0
                {
                    ready.push_back(node_id.clone());
                }
            }

            info!(
                workflow = "dag",
                nodes = self.node_order.len(),
                edges = self.edges.len(),
                roots = ready.len(),
                "🔀 DAG workflow started"
            );

            while !ready.is_empty() {
                let batch: Vec<String> = ready.drain(..).collect();

                debug!(
                    workflow = "dag",
                    batch = ?batch,
                    "⚡ Executing {} nodes concurrently",
                    batch.len()
                );

                let mut handles = Vec::with_capacity(batch.len());

                for node_id in &batch {
                    let agent_handle = self.nodes.get(node_id).cloned().ok_or_else(|| {
                        ReactError::Other(format!("DAG node {node_id:?} has no registered agent"))
                    })?;
                    let preds = predecessors
                        .get(node_id.as_str())
                        .cloned()
                        .unwrap_or_default();

                    let node_input = if preds.is_empty() {
                        input.to_string()
                    } else {
                        preds
                            .iter()
                            .filter_map(|p| node_results.get(p.as_str()))
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n\n")
                    };

                    let nid = node_id.clone();
                    handles.push(tokio::spawn(async move {
                        let step_start = Instant::now();
                        let agent = agent_handle.as_ref();
                        let agent_name = agent.name().to_string();
                        let result = agent.execute(&node_input).await;
                        let elapsed = step_start.elapsed();
                        (nid, agent_name, node_input, result, elapsed)
                    }));
                }

                let mut first_error = None;
                for handle in &mut handles {
                    let (node_id, agent_name, node_input, result, elapsed) = match handle.await {
                        Ok(value) => value,
                        Err(error) => {
                            first_error.get_or_insert_with(|| {
                                ReactError::Other(format!("task join error: {error}"))
                            });
                            continue;
                        }
                    };
                    let output = match result {
                        Ok(output) => output,
                        Err(error) => {
                            first_error.get_or_insert(error);
                            continue;
                        }
                    };

                    info!(
                        workflow = "dag",
                        node = %node_id,
                        agent = %agent_name,
                        elapsed_ms = elapsed.as_millis(),
                        "✓ Node completed"
                    );

                    step_outputs.push(StepOutput {
                        agent_name,
                        input: node_input,
                        output: output.clone(),
                        elapsed,
                    });

                    node_results.insert(node_id.clone(), output);

                    if let Some(succs) = successors.get(node_id.as_str()) {
                        for succ in succs {
                            if let Some(deg) = remaining_in_degree.get_mut(succ.as_str()) {
                                *deg = deg.saturating_sub(1);
                                if *deg == 0 {
                                    ready.push_back(succ.clone());
                                }
                            }
                        }
                    }
                }
                if let Some(error) = first_error {
                    for handle in &handles {
                        if !handle.is_finished() {
                            handle.abort();
                        }
                    }
                    return Err(error);
                }
            }

            // Final result: collect output from all leaf nodes (out-degree = 0)
            let leaf_nodes: Vec<&str> = self
                .node_order
                .iter()
                .filter(|id| successors.get(id.as_str()).is_none_or(|s| s.is_empty()))
                .map(|s| s.as_str())
                .collect();

            let final_result = leaf_nodes
                .iter()
                .filter_map(|id| node_results.get(*id))
                .cloned()
                .collect::<Vec<_>>()
                .join("\n\n");

            Ok(WorkflowOutput {
                result: final_result,
                steps: step_outputs,
                elapsed: total_start.elapsed(),
            })
        })
    }
}

/// [`DagWorkflow`] builder
pub struct DagWorkflowBuilder {
    nodes: Vec<(String, SharedAgent)>,
    edges: Vec<DagEdge>,
}

impl DagWorkflowBuilder {
    /// Register a named node
    pub fn node(mut self, id: impl Into<String>, agent: impl Agent + 'static) -> Self {
        self.nodes.push((id.into(), shared_agent(agent)));
        self
    }

    /// Register a named node (using an already-wrapped SharedAgent)
    pub fn node_shared(mut self, id: impl Into<String>, agent: SharedAgent) -> Self {
        self.nodes.push((id.into(), agent));
        self
    }

    /// Add a directed edge: `from`'s output will flow into `to`'s input
    pub fn edge(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.edges.push(DagEdge {
            from: from.into(),
            to: to.into(),
        });
        self
    }

    /// Build the DAG workflow, validate acyclicity, and compute topological order
    pub fn build(self) -> Result<DagWorkflow> {
        let node_ids: HashSet<&str> = self.nodes.iter().map(|(id, _)| id.as_str()).collect();
        if node_ids.len() != self.nodes.len() {
            return Err(ReactError::Agent(Box::new(
                AgentError::InitializationFailed("DAG contains duplicate node IDs".to_string()),
            )));
        }

        for edge in &self.edges {
            if !node_ids.contains(edge.from.as_str()) {
                return Err(ReactError::Agent(Box::new(
                    AgentError::InitializationFailed(format!(
                        "DAG edge references unknown node: '{}'",
                        edge.from
                    )),
                )));
            }
            if !node_ids.contains(edge.to.as_str()) {
                return Err(ReactError::Agent(Box::new(
                    AgentError::InitializationFailed(format!(
                        "DAG edge references unknown node: '{}'",
                        edge.to
                    )),
                )));
            }
        }

        let node_list: Vec<String> = self.nodes.iter().map(|(id, _)| id.clone()).collect();
        if let Some(cycle) = detect_cycle(&node_list, &self.edges) {
            return Err(ReactError::Agent(Box::new(
                AgentError::InitializationFailed(format!(
                    "DAG contains cycle: {}",
                    cycle.join(" → ")
                )),
            )));
        }

        let topo_order = topological_sort(&node_list, &self.edges)?;

        let nodes: HashMap<String, SharedAgent> = self.nodes.into_iter().collect();

        Ok(DagWorkflow {
            nodes,
            edges: self.edges,
            node_order: topo_order,
        })
    }
}

// ── DAG Algorithms ──────────────────────────────────────────────────────────────────

fn build_predecessors(edges: &[DagEdge]) -> HashMap<&str, Vec<String>> {
    let mut preds: HashMap<&str, Vec<String>> = HashMap::new();
    for edge in edges {
        preds
            .entry(edge.to.as_str())
            .or_default()
            .push(edge.from.clone());
    }
    preds
}

fn build_successors(edges: &[DagEdge]) -> HashMap<&str, Vec<String>> {
    let mut succs: HashMap<&str, Vec<String>> = HashMap::new();
    for edge in edges {
        succs
            .entry(edge.from.as_str())
            .or_default()
            .push(edge.to.clone());
    }
    succs
}

fn compute_in_degree<'a>(nodes: &'a [String], edges: &[DagEdge]) -> HashMap<&'a str, usize> {
    let mut deg: HashMap<&str, usize> = nodes.iter().map(|id| (id.as_str(), 0)).collect();
    for edge in edges {
        if let Some(d) = deg.get_mut(edge.to.as_str()) {
            *d = d.saturating_add(1);
        }
    }
    deg
}

/// Kahn's algorithm for topological sort
fn topological_sort(nodes: &[String], edges: &[DagEdge]) -> Result<Vec<String>> {
    let mut in_deg = compute_in_degree(nodes, edges);
    let succs = build_successors(edges);

    let mut queue: VecDeque<String> = nodes
        .iter()
        .filter(|id| in_deg.get(id.as_str()).copied().unwrap_or_default() == 0)
        .cloned()
        .collect();

    let mut order = Vec::with_capacity(nodes.len());

    while let Some(node) = queue.pop_front() {
        order.push(node.clone());
        if let Some(neighbors) = succs.get(node.as_str()) {
            for neighbor in neighbors {
                if let Some(d) = in_deg.get_mut(neighbor.as_str()) {
                    *d -= 1;
                    if *d == 0 {
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }
    }

    if order.len() != nodes.len() {
        return Err(ReactError::Agent(Box::new(
            AgentError::InitializationFailed(
                "DAG contains a cycle (topological sort incomplete)".to_string(),
            ),
        )));
    }

    Ok(order)
}

/// DFS-based cycle detection; returns the cycle path if found
fn detect_cycle(nodes: &[String], edges: &[DagEdge]) -> Option<Vec<String>> {
    let succs: HashMap<String, Vec<String>> = {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for edge in edges {
            map.entry(edge.from.clone())
                .or_default()
                .push(edge.to.clone());
        }
        map
    };

    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }

    let mut color: HashMap<String, Color> =
        nodes.iter().map(|id| (id.clone(), Color::White)).collect();
    let mut path: Vec<String> = Vec::new();

    fn dfs(
        node: &str,
        succs: &HashMap<String, Vec<String>>,
        color: &mut HashMap<String, Color>,
        path: &mut Vec<String>,
    ) -> bool {
        color.insert(node.to_string(), Color::Gray);
        path.push(node.to_string());

        if let Some(neighbors) = succs.get(node) {
            for neighbor in neighbors {
                match color.get(neighbor.as_str()).copied() {
                    Some(Color::Gray) => {
                        path.push(neighbor.clone());
                        return true;
                    }
                    Some(Color::White) | None if dfs(neighbor, succs, color, path) => {
                        return true;
                    }
                    Some(Color::White) | None => {}
                    _ => {}
                }
            }
        }

        path.pop();
        color.insert(node.to_string(), Color::Black);
        false
    }

    for node in nodes {
        if color[node.as_str()] == Color::White && dfs(node, &succs, &mut color, &mut path) {
            return Some(path);
        }
    }

    None
}

// ── Unit Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topological_sort_simple() {
        let nodes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let edges = vec![
            DagEdge {
                from: "a".into(),
                to: "b".into(),
            },
            DagEdge {
                from: "b".into(),
                to: "c".into(),
            },
        ];
        let order = topological_sort(&nodes, &edges).unwrap();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_topological_sort_diamond() {
        let nodes = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let edges = vec![
            DagEdge {
                from: "a".into(),
                to: "b".into(),
            },
            DagEdge {
                from: "a".into(),
                to: "c".into(),
            },
            DagEdge {
                from: "b".into(),
                to: "d".into(),
            },
            DagEdge {
                from: "c".into(),
                to: "d".into(),
            },
        ];
        let order = topological_sort(&nodes, &edges).unwrap();
        assert_eq!(order[0], "a");
        assert_eq!(order[3], "d");
        assert!(order.contains(&"b".to_string()));
        assert!(order.contains(&"c".to_string()));
    }

    #[test]
    fn test_cycle_detection() {
        let nodes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let edges = vec![
            DagEdge {
                from: "a".into(),
                to: "b".into(),
            },
            DagEdge {
                from: "b".into(),
                to: "c".into(),
            },
            DagEdge {
                from: "c".into(),
                to: "a".into(),
            },
        ];
        assert!(detect_cycle(&nodes, &edges).is_some());
    }

    #[test]
    fn test_no_cycle() {
        let nodes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let edges = vec![
            DagEdge {
                from: "a".into(),
                to: "b".into(),
            },
            DagEdge {
                from: "a".into(),
                to: "c".into(),
            },
        ];
        assert!(detect_cycle(&nodes, &edges).is_none());
    }
}
