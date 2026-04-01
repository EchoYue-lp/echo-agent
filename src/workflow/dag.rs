//! DAG 工作流：Agent 以有向无环图组织，按拓扑序执行，独立节点自动并发。

use super::{SharedAgent, StepOutput, Workflow, WorkflowOutput, shared_agent};
use crate::agent::Agent;
use crate::error::{AgentError, ReactError, Result};
use futures::future::BoxFuture;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;
use tracing::{debug, info};

/// DAG 中的节点
pub struct DagNode {
    pub id: String,
    pub agent: SharedAgent,
}

/// DAG 中的有向边（from → to 表示 to 依赖 from 的输出）
#[derive(Debug, Clone)]
pub struct DagEdge {
    pub from: String,
    pub to: String,
}

/// DAG 工作流：节点按拓扑序执行，无依赖关系的节点自动并发。
///
/// 每个节点接收其所有前驱节点的输出（以换行拼接）作为输入；
/// 入度为零的根节点接收工作流的原始输入。
///
/// # 示例
///
/// ```rust,no_run
/// use echo_agent::prelude::*;
/// use echo_agent::workflow::{DagWorkflow, Workflow};
///
/// # async fn example() -> echo_agent::error::Result<()> {
/// let researcher = ReactAgentBuilder::simple("qwen3-max", "你是调研员")?;
/// let analyst = ReactAgentBuilder::simple("qwen3-max", "你是分析师")?;
/// let writer = ReactAgentBuilder::simple("qwen3-max", "你是报告撰写者")?;
///
/// let mut wf = DagWorkflow::builder()
///     .node("research", researcher)
///     .node("analyze", analyst)
///     .node("write", writer)
///     .edge("research", "write")
///     .edge("analyze", "write")
///     .build()?;
///
/// let output = wf.run("分析 2025 年 AI Agent 生态").await?;
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
                if remaining_in_degree[node_id.as_str()] == 0 {
                    ready.push_back(node_id.clone());
                }
            }

            info!(
                workflow = "dag",
                nodes = self.node_order.len(),
                edges = self.edges.len(),
                roots = ready.len(),
                "🔀 DAG 工作流开始执行"
            );

            while !ready.is_empty() {
                let batch: Vec<String> = ready.drain(..).collect();

                debug!(
                    workflow = "dag",
                    batch = ?batch,
                    "⚡ 并发执行 {} 个节点",
                    batch.len()
                );

                let mut handles = Vec::with_capacity(batch.len());

                for node_id in &batch {
                    let agent_handle = self.nodes[node_id].clone();
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
                        let mut agent = agent_handle.lock().await;
                        let agent_name = agent.name().to_string();
                        let result = agent.execute(&node_input).await;
                        let elapsed = step_start.elapsed();
                        (nid, agent_name, node_input, result, elapsed)
                    }));
                }

                for handle in handles {
                    let (node_id, agent_name, node_input, result, elapsed) = handle
                        .await
                        .map_err(|e| ReactError::Other(format!("task join error: {e}")))?;

                    let output = result?;

                    info!(
                        workflow = "dag",
                        node = %node_id,
                        agent = %agent_name,
                        elapsed_ms = elapsed.as_millis(),
                        "✓ 节点完成"
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
                                *deg -= 1;
                                if *deg == 0 {
                                    ready.push_back(succ.clone());
                                }
                            }
                        }
                    }
                }
            }

            // 最终结果取所有出度为零的叶子节点的输出
            let leaf_nodes: Vec<&str> = self
                .node_order
                .iter()
                .filter(|id| successors.get(id.as_str()).map_or(true, |s| s.is_empty()))
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

/// [`DagWorkflow`] 构建器
pub struct DagWorkflowBuilder {
    nodes: Vec<(String, SharedAgent)>,
    edges: Vec<DagEdge>,
}

impl DagWorkflowBuilder {
    /// 注册一个命名节点
    pub fn node(mut self, id: impl Into<String>, agent: impl Agent + 'static) -> Self {
        self.nodes.push((id.into(), shared_agent(agent)));
        self
    }

    /// 注册一个命名节点（使用已包装的 SharedAgent）
    pub fn node_shared(mut self, id: impl Into<String>, agent: SharedAgent) -> Self {
        self.nodes.push((id.into(), agent));
        self
    }

    /// 添加有向边：`from` 的输出将流入 `to` 的输入
    pub fn edge(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.edges.push(DagEdge {
            from: from.into(),
            to: to.into(),
        });
        self
    }

    /// 构建 DAG 工作流，验证无环并计算拓扑序
    pub fn build(self) -> Result<DagWorkflow> {
        let node_ids: HashSet<&str> = self.nodes.iter().map(|(id, _)| id.as_str()).collect();

        for edge in &self.edges {
            if !node_ids.contains(edge.from.as_str()) {
                return Err(ReactError::Agent(AgentError::InitializationFailed(
                    format!("DAG edge references unknown node: '{}'", edge.from),
                )));
            }
            if !node_ids.contains(edge.to.as_str()) {
                return Err(ReactError::Agent(AgentError::InitializationFailed(
                    format!("DAG edge references unknown node: '{}'", edge.to),
                )));
            }
        }

        let node_list: Vec<String> = self.nodes.iter().map(|(id, _)| id.clone()).collect();
        if let Some(cycle) = detect_cycle(&node_list, &self.edges) {
            return Err(ReactError::Agent(AgentError::InitializationFailed(
                format!("DAG contains cycle: {}", cycle.join(" → ")),
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

// ── DAG 算法 ──────────────────────────────────────────────────────────────────

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
            *d += 1;
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
        .filter(|id| in_deg[id.as_str()] == 0)
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
        return Err(ReactError::Agent(AgentError::InitializationFailed(
            "DAG contains a cycle (topological sort incomplete)".to_string(),
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
                match color.get(neighbor.as_str()) {
                    Some(Color::Gray) => {
                        path.push(neighbor.clone());
                        return true;
                    }
                    Some(Color::White) | None => {
                        if dfs(neighbor, succs, color, path) {
                            return true;
                        }
                    }
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

// ── 单元测试 ──────────────────────────────────────────────────────────────────

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

    #[test]
    fn test_builder_unknown_node() {
        use crate::testing::MockAgent;
        let result = DagWorkflow::builder()
            .node("a", MockAgent::new("a"))
            .edge("a", "nonexistent")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_builder_cycle_rejected() {
        use crate::testing::MockAgent;
        let result = DagWorkflow::builder()
            .node("a", MockAgent::new("a"))
            .node("b", MockAgent::new("b"))
            .edge("a", "b")
            .edge("b", "a")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_builder_valid_dag() {
        use crate::testing::MockAgent;
        let result = DagWorkflow::builder()
            .node("research", MockAgent::new("r"))
            .node("analyze", MockAgent::new("a"))
            .node("write", MockAgent::new("w"))
            .edge("research", "write")
            .edge("analyze", "write")
            .build();
        assert!(result.is_ok());
    }
}
