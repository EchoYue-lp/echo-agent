//! Agent 拓扑可视化
//!
//! 运行时记录 Agent 间的调用关系，并支持导出为 DOT (Graphviz) 和 Mermaid 格式。
//!
//! # 核心概念
//!
//! - [`TopologyTracker`]: 全局调用关系追踪器（线程安全）
//! - [`TopologyNode`]: 图中的节点（Agent）
//! - [`TopologyEdge`]: 图中的边（调用关系）
//! - [`TopologyCallback`]: Agent 回调，自动记录调用关系
//!
//! # 示例
//!
//! ```rust
//! use echo_agent::topology::{TopologyTracker, TopologyNode, NodeType};
//!
//! let tracker = TopologyTracker::new();
//!
//! // 注册节点
//! tracker.add_node(TopologyNode::new("orchestrator", NodeType::Orchestrator));
//! tracker.add_node(TopologyNode::new("math_agent", NodeType::Worker));
//!
//! // 记录调用
//! tracker.record_call("orchestrator", "math_agent", "计算 1+1");
//!
//! // 导出 Mermaid 图
//! println!("{}", tracker.to_mermaid());
//!
//! // 导出 DOT 图
//! println!("{}", tracker.to_dot());
//! ```

use crate::agent::AgentCallback;
use crate::error::ReactError;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// ── 节点与边的类型 ───────────────────────────────────────────────────────────

/// 节点类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeType {
    /// 编排者 Agent
    Orchestrator,
    /// 工作者 Agent
    Worker,
    /// 规划器
    Planner,
    /// 外部服务（MCP、A2A 等）
    External,
    /// 工具
    Tool,
}

/// 拓扑图节点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyNode {
    /// 节点 ID（唯一标识）
    pub id: String,
    /// 显示名称
    pub label: String,
    /// 节点类型
    pub node_type: NodeType,
    /// 节点元数据
    pub metadata: HashMap<String, String>,
}

impl TopologyNode {
    pub fn new(id: impl Into<String>, node_type: NodeType) -> Self {
        let id_str: String = id.into();
        Self {
            label: id_str.clone(),
            id: id_str,
            node_type,
            metadata: HashMap::new(),
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

/// 拓扑图边（调用关系）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyEdge {
    /// 源节点 ID
    pub from: String,
    /// 目标节点 ID
    pub to: String,
    /// 调用标签（描述调用内容）
    pub label: Option<String>,
    /// 调用次数
    pub call_count: u64,
    /// 最近一次调用时间
    pub last_called: DateTime<Utc>,
    /// 累计执行时间（毫秒）
    pub total_duration_ms: u64,
}

// ── TopologyTracker ──────────────────────────────────────────────────────────

/// 全局拓扑追踪器（线程安全）
///
/// 用于记录 Agent 运行时的调用关系图。可通过 `TopologyCallback` 自动集成到 Agent 中。
pub struct TopologyTracker {
    nodes: RwLock<HashMap<String, TopologyNode>>,
    edges: RwLock<HashMap<(String, String), TopologyEdge>>,
}

impl TopologyTracker {
    pub fn new() -> Self {
        Self {
            nodes: RwLock::new(HashMap::new()),
            edges: RwLock::new(HashMap::new()),
        }
    }

    /// 添加节点
    pub fn add_node(&self, node: TopologyNode) {
        if let Ok(mut nodes) = self.nodes.write() {
            nodes.insert(node.id.clone(), node);
        }
    }

    /// 记录一次调用关系
    pub fn record_call(&self, from: &str, to: &str, label: &str) {
        self.record_call_with_duration(from, to, label, 0);
    }

    /// 记录一次调用关系（含耗时）
    pub fn record_call_with_duration(&self, from: &str, to: &str, label: &str, duration_ms: u64) {
        // 确保节点存在
        self.ensure_node(from, NodeType::Worker);
        self.ensure_node(to, NodeType::Tool);

        if let Ok(mut edges) = self.edges.write() {
            let key = (from.to_string(), to.to_string());
            let edge = edges.entry(key).or_insert_with(|| TopologyEdge {
                from: from.to_string(),
                to: to.to_string(),
                label: Some(label.to_string()),
                call_count: 0,
                last_called: Utc::now(),
                total_duration_ms: 0,
            });
            edge.call_count += 1;
            edge.last_called = Utc::now();
            edge.total_duration_ms += duration_ms;
            // 更新标签为最新调用
            edge.label = Some(label.to_string());
        }
    }

    /// 确保节点存在（不覆盖已有节点）
    fn ensure_node(&self, id: &str, default_type: NodeType) {
        if let Ok(mut nodes) = self.nodes.write() {
            nodes
                .entry(id.to_string())
                .or_insert_with(|| TopologyNode::new(id, default_type));
        }
    }

    /// 获取所有节点
    pub fn nodes(&self) -> Vec<TopologyNode> {
        self.nodes
            .read()
            .map(|n| n.values().cloned().collect())
            .unwrap_or_default()
    }

    /// 获取所有边
    pub fn edges(&self) -> Vec<TopologyEdge> {
        self.edges
            .read()
            .map(|e| e.values().cloned().collect())
            .unwrap_or_default()
    }

    /// 获取统计信息
    pub fn stats(&self) -> TopologyStats {
        let nodes = self.nodes.read().map(|n| n.len()).unwrap_or(0);
        let edges = self.edges.read().map(|e| e.len()).unwrap_or(0);
        let total_calls = self
            .edges
            .read()
            .map(|e| e.values().map(|edge| edge.call_count).sum())
            .unwrap_or(0);

        TopologyStats {
            node_count: nodes,
            edge_count: edges,
            total_calls,
        }
    }

    /// 清空拓扑数据
    pub fn clear(&self) {
        if let Ok(mut nodes) = self.nodes.write() {
            nodes.clear();
        }
        if let Ok(mut edges) = self.edges.write() {
            edges.clear();
        }
    }

    // ── 导出格式 ─────────────────────────────────────────────────────────────

    /// 导出为 Mermaid 流程图格式
    ///
    /// ```text
    /// graph TD
    ///     orchestrator[🎯 orchestrator]
    ///     math_agent[⚙️ math_agent]
    ///     orchestrator -->|"计算 1+1 (x3)"| math_agent
    /// ```
    pub fn to_mermaid(&self) -> String {
        let mut lines = vec!["graph TD".to_string()];

        // 节点定义
        if let Ok(nodes) = self.nodes.read() {
            for node in nodes.values() {
                let icon = match node.node_type {
                    NodeType::Orchestrator => "🎯",
                    NodeType::Worker => "⚙️",
                    NodeType::Planner => "📐",
                    NodeType::External => "🌐",
                    NodeType::Tool => "🔧",
                };
                let shape = match node.node_type {
                    NodeType::Orchestrator => {
                        format!("{}{{{{\"{}  {}\"}}}}", node.id, icon, node.label)
                    }
                    NodeType::Tool => format!("{}[/\"{}  {}\"/]", node.id, icon, node.label),
                    NodeType::External => format!("{}((\"{}  {}\"))", node.id, icon, node.label),
                    _ => format!("{}[\"{}  {}\"]", node.id, icon, node.label),
                };
                lines.push(format!("    {}", shape));
            }
        }

        // 边定义
        if let Ok(edges) = self.edges.read() {
            for edge in edges.values() {
                let label = if let Some(l) = &edge.label {
                    let truncated = if l.len() > 30 {
                        format!("{}...", &l[..30])
                    } else {
                        l.clone()
                    };
                    if edge.call_count > 1 {
                        format!("{} (x{})", truncated, edge.call_count)
                    } else {
                        truncated
                    }
                } else {
                    format!("x{}", edge.call_count)
                };
                lines.push(format!("    {} -->|\"{}\"| {}", edge.from, label, edge.to));
            }
        }

        lines.join("\n")
    }

    /// 导出为 DOT (Graphviz) 格式
    pub fn to_dot(&self) -> String {
        let mut lines = vec![
            "digraph AgentTopology {".to_string(),
            "    rankdir=TB;".to_string(),
            "    node [shape=box, style=rounded];".to_string(),
            String::new(),
        ];

        // 节点定义
        if let Ok(nodes) = self.nodes.read() {
            for node in nodes.values() {
                let (shape, color) = match node.node_type {
                    NodeType::Orchestrator => ("diamond", "#FFB74D"),
                    NodeType::Worker => ("box", "#64B5F6"),
                    NodeType::Planner => ("hexagon", "#81C784"),
                    NodeType::External => ("ellipse", "#CE93D8"),
                    NodeType::Tool => ("component", "#90A4AE"),
                };
                lines.push(format!(
                    "    \"{}\" [label=\"{}\", shape={}, style=\"filled,rounded\", fillcolor=\"{}\"];",
                    node.id, node.label, shape, color
                ));
            }
        }

        lines.push(String::new());

        // 边定义
        if let Ok(edges) = self.edges.read() {
            for edge in edges.values() {
                let label = if let Some(l) = &edge.label {
                    let truncated = if l.len() > 30 {
                        format!("{}...", &l[..30])
                    } else {
                        l.clone()
                    };
                    format!("{} (x{})", truncated, edge.call_count)
                } else {
                    format!("x{}", edge.call_count)
                };

                let penwidth = if edge.call_count > 5 {
                    "3.0"
                } else if edge.call_count > 1 {
                    "2.0"
                } else {
                    "1.0"
                };

                lines.push(format!(
                    "    \"{}\" -> \"{}\" [label=\"{}\", penwidth={}];",
                    edge.from, edge.to, label, penwidth
                ));
            }
        }

        lines.push("}".to_string());
        lines.join("\n")
    }

    /// 导出为 JSON 格式（便于前端渲染）
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let data = TopologyData {
            nodes: self.nodes(),
            edges: self.edges(),
            stats: self.stats(),
        };
        serde_json::to_string_pretty(&data)
    }
}

impl Default for TopologyTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// 拓扑数据（用于 JSON 序列化）
#[derive(Debug, Serialize, Deserialize)]
pub struct TopologyData {
    pub nodes: Vec<TopologyNode>,
    pub edges: Vec<TopologyEdge>,
    pub stats: TopologyStats,
}

/// 拓扑统计信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyStats {
    pub node_count: usize,
    pub edge_count: usize,
    pub total_calls: u64,
}

// ── TopologyCallback ─────────────────────────────────────────────────────────

/// Agent 回调 — 自动将工具调用记录到拓扑追踪器
///
/// 将此回调添加到 Agent 后，所有工具调用会自动记录到拓扑图中。
///
/// ```rust,no_run
/// use echo_agent::topology::{TopologyTracker, TopologyCallback};
/// use echo_agent::prelude::*;
/// use std::sync::Arc;
///
/// let tracker = Arc::new(TopologyTracker::new());
/// let callback = Arc::new(TopologyCallback::new(tracker.clone()));
///
/// let agent = ReactAgentBuilder::new()
///     .model("qwen3-max")
///     .name("my_agent")
///     .enable_tools()
///     .callback(callback)
///     .build()
///     .unwrap();
///
/// // agent 执行后，可以查看拓扑图
/// println!("{}", tracker.to_mermaid());
/// ```
pub struct TopologyCallback {
    tracker: Arc<TopologyTracker>,
}

impl TopologyCallback {
    pub fn new(tracker: Arc<TopologyTracker>) -> Self {
        Self { tracker }
    }
}

impl AgentCallback for TopologyCallback {
    fn on_tool_start<'a>(
        &'a self,
        agent: &'a str,
        tool: &'a str,
        _args: &'a serde_json::Value,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.tracker
                .add_node(TopologyNode::new(agent, NodeType::Worker));
            self.tracker
                .add_node(TopologyNode::new(tool, NodeType::Tool));
            self.tracker.record_call(agent, tool, "调用");
        })
    }

    fn on_tool_end<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _result: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_tool_error<'a>(
        &'a self,
        agent: &'a str,
        tool: &'a str,
        _err: &'a ReactError,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.tracker.record_call(agent, tool, "调用失败");
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topology_tracker_basic() {
        let tracker = TopologyTracker::new();

        tracker.add_node(TopologyNode::new("agent_a", NodeType::Orchestrator));
        tracker.add_node(TopologyNode::new("agent_b", NodeType::Worker));
        tracker.record_call("agent_a", "agent_b", "分派任务");

        let stats = tracker.stats();
        assert_eq!(stats.node_count, 2);
        assert_eq!(stats.edge_count, 1);
        assert_eq!(stats.total_calls, 1);
    }

    #[test]
    fn test_topology_multiple_calls() {
        let tracker = TopologyTracker::new();

        tracker.add_node(TopologyNode::new("agent", NodeType::Worker));
        tracker.record_call("agent", "calc", "1+1");
        tracker.record_call("agent", "calc", "2+2");
        tracker.record_call("agent", "calc", "3+3");

        let edges = tracker.edges();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].call_count, 3);
    }

    #[test]
    fn test_topology_to_mermaid() {
        let tracker = TopologyTracker::new();
        tracker.add_node(TopologyNode::new("orchestrator", NodeType::Orchestrator));
        tracker.add_node(TopologyNode::new("worker", NodeType::Worker));
        tracker.record_call("orchestrator", "worker", "执行");

        let mermaid = tracker.to_mermaid();
        assert!(mermaid.starts_with("graph TD"));
        assert!(mermaid.contains("orchestrator"));
        assert!(mermaid.contains("worker"));
    }

    #[test]
    fn test_topology_to_dot() {
        let tracker = TopologyTracker::new();
        tracker.add_node(TopologyNode::new("agent", NodeType::Worker));
        tracker.record_call("agent", "tool1", "使用");

        let dot = tracker.to_dot();
        assert!(dot.starts_with("digraph"));
        assert!(dot.contains("agent"));
        assert!(dot.contains("tool1"));
    }

    #[test]
    fn test_topology_to_json() {
        let tracker = TopologyTracker::new();
        tracker.add_node(TopologyNode::new("a", NodeType::Worker));
        tracker.record_call("a", "b", "call");

        let json = tracker.to_json().unwrap();
        let data: TopologyData = serde_json::from_str(&json).unwrap();
        assert!(!data.nodes.is_empty());
        assert!(!data.edges.is_empty());
    }

    #[test]
    fn test_topology_clear() {
        let tracker = TopologyTracker::new();
        tracker.add_node(TopologyNode::new("a", NodeType::Worker));
        tracker.record_call("a", "b", "call");
        assert!(tracker.stats().node_count > 0);

        tracker.clear();
        assert_eq!(tracker.stats().node_count, 0);
        assert_eq!(tracker.stats().edge_count, 0);
    }

    #[test]
    fn test_topology_node_builder() {
        let node = TopologyNode::new("test", NodeType::External)
            .with_label("Test Service")
            .with_metadata("url", "http://localhost");

        assert_eq!(node.id, "test");
        assert_eq!(node.label, "Test Service");
        assert_eq!(node.metadata.get("url").unwrap(), "http://localhost");
    }
}
