//! Agent topology visualization
//!
//! Records call relationships between Agents at runtime and supports export to
//! DOT (Graphviz) and Mermaid formats.
//!
//! # Core Concepts
//!
//! - [`TopologyTracker`][]: Global call relationship tracker (thread-safe)
//! - [`TopologyNode`][]: Nodes in the graph (Agents)
//! - [`TopologyEdge`][]: Edges in the graph (call relationships)
//! - [`TopologyCallback`]: Agent callback that auto-records call relationships
//!
//! # Example
//!
//! ```rust
//! use echo_agent::topology::{TopologyTracker, TopologyNode, NodeType};
//!
//! let tracker = TopologyTracker::new();
//!
//! // Register nodes
//! tracker.add_node(TopologyNode::new("orchestrator", NodeType::Orchestrator));
//! tracker.add_node(TopologyNode::new("math_agent", NodeType::Subagent));
//!
//! // Record call
//! tracker.record_call("orchestrator", "math_agent", "compute 1+1");
//!
//! // Export Mermaid diagram
//! println!("{}", tracker.to_mermaid());
//!
//! // Export DOT diagram
//! println!("{}", tracker.to_dot());
//! ```

use crate::agent::AgentCallback;
use crate::error::ReactError;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// ── Node and Edge Types ───────────────────────────────────────────────────────

/// Node type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeType {
    /// Orchestrator Agent
    Orchestrator,
    /// Subagent Agent
    Subagent,
    /// Planner
    Planner,
    /// External service (MCP, A2A, etc.)
    External,
    /// Tool
    Tool,
}

/// Topology graph node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyNode {
    /// Node ID (unique identifier)
    pub id: String,
    /// Display name
    pub label: String,
    /// Node type
    pub node_type: NodeType,
    /// Node metadata
    pub metadata: HashMap<String, String>,
}

impl TopologyNode {
    /// Create a new topology node.
    ///
    /// # Arguments
    /// - `id`: Unique node identifier
    /// - `node_type`: Node type (Orchestrator, Subagent, Tool, etc.)
    ///
    /// # Example
    /// ```
    /// use echo_agent::topology::{TopologyNode, NodeType};
    ///
    /// let node = TopologyNode::new("my_agent", NodeType::Subagent);
    /// assert_eq!(node.id, "my_agent");
    /// assert_eq!(node.node_type, NodeType::Subagent);
    /// ```
    pub fn new(id: impl Into<String>, node_type: NodeType) -> Self {
        let id_str: String = id.into();
        Self {
            label: id_str.clone(),
            id: id_str,
            node_type,
            metadata: HashMap::new(),
        }
    }

    /// Set the node's display label (different from ID).
    ///
    /// # Arguments
    /// - `label`: Display label, used in visualizations
    ///
    /// # Example
    /// ```
    /// use echo_agent::topology::{TopologyNode, NodeType};
    ///
    /// let node = TopologyNode::new("agent_123", NodeType::Subagent)
    ///     .with_label("Compute Agent");
    /// assert_eq!(node.label, "Compute Agent");
    /// ```
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Add a metadata key-value pair to the node.
    ///
    /// Metadata can be used to store extra information such as URL, version,
    /// configuration, etc.
    ///
    /// # Arguments
    /// - `key`: Metadata key
    /// - `value`: Metadata value
    ///
    /// # Example
    /// ```
    /// use echo_agent::topology::{TopologyNode, NodeType};
    ///
    /// let node = TopologyNode::new("web_search", NodeType::Tool)
    ///     .with_metadata("provider", "DuckDuckGo")
    ///     .with_metadata("rate_limit", "10/min");
    /// assert_eq!(node.metadata.get("provider").unwrap(), "DuckDuckGo");
    /// ```
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

/// Topology graph edge (call relationship)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyEdge {
    /// Source node ID
    pub from: String,
    /// Target node ID
    pub to: String,
    /// Call label (describes call content)
    pub label: Option<String>,
    /// Call count
    pub call_count: u64,
    /// Most recent call time
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub last_called: DateTime<Utc>,
    /// Cumulative execution time (milliseconds)
    pub total_duration_ms: u64,
}

// ── TopologyTracker ──────────────────────────────────────────────────────────

/// Global topology tracker (thread-safe)
///
/// Records the call relationship graph at Agent runtime. Can be automatically
/// integrated into an Agent via `TopologyCallback`.
pub struct TopologyTracker {
    nodes: RwLock<HashMap<String, TopologyNode>>,
    edges: RwLock<HashMap<(String, String), TopologyEdge>>,
}

impl TopologyTracker {
    /// Create a new topology tracker instance.
    ///
    /// # Example
    /// ```
    /// use echo_agent::topology::TopologyTracker;
    ///
    /// let tracker = TopologyTracker::new();
    /// assert_eq!(tracker.stats().node_count, 0);
    /// ```
    pub fn new() -> Self {
        Self {
            nodes: RwLock::new(HashMap::new()),
            edges: RwLock::new(HashMap::new()),
        }
    }

    /// Add a node
    pub fn add_node(&self, node: TopologyNode) {
        if let Ok(mut nodes) = self.nodes.write() {
            nodes.insert(node.id.clone(), node);
        }
    }

    /// Record a call relationship
    pub fn record_call(&self, from: &str, to: &str, label: &str) {
        self.record_call_with_duration(from, to, label, 0);
    }

    /// Record a call relationship (with duration)
    pub fn record_call_with_duration(&self, from: &str, to: &str, label: &str, duration_ms: u64) {
        // Ensure the node exists
        self.ensure_node(from, NodeType::Subagent);
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
            // Update the label to the most recent call
            edge.label = Some(label.to_string());
        }
    }

    /// Ensure node exists (does not overwrite existing node)
    fn ensure_node(&self, id: &str, default_type: NodeType) {
        if let Ok(mut nodes) = self.nodes.write() {
            nodes
                .entry(id.to_string())
                .or_insert_with(|| TopologyNode::new(id, default_type));
        }
    }

    /// Get all nodes
    pub fn nodes(&self) -> Vec<TopologyNode> {
        self.nodes
            .read()
            .map(|n| n.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Get all edges
    pub fn edges(&self) -> Vec<TopologyEdge> {
        self.edges
            .read()
            .map(|e| e.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Get statistics
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

    /// Clear topology data
    pub fn clear(&self) {
        if let Ok(mut nodes) = self.nodes.write() {
            nodes.clear();
        }
        if let Ok(mut edges) = self.edges.write() {
            edges.clear();
        }
    }

    // ── Export Formats ─────────────────────────────────────────────────────────

    /// Export as Mermaid flowchart format
    ///
    /// ```text
    /// graph TD
    ///     orchestrator[🎯 orchestrator]
    ///     math_agent[⚙️ math_agent]
    ///     orchestrator -->|"compute 1+1 (x3)"| math_agent
    /// ```
    pub fn to_mermaid(&self) -> String {
        let mut lines = vec!["graph TD".to_string()];

        // Node definitions
        if let Ok(nodes) = self.nodes.read() {
            for node in nodes.values() {
                let icon = match node.node_type {
                    NodeType::Orchestrator => "🎯",
                    NodeType::Subagent => "⚙️",
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

        // Edge definitions
        if let Ok(edges) = self.edges.read() {
            for edge in edges.values() {
                let label = if let Some(l) = &edge.label {
                    let truncated = if l.chars().count() > 30 {
                        let t: String = l.chars().take(30).collect();
                        format!("{t}...")
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

    /// Export as DOT (Graphviz) format
    pub fn to_dot(&self) -> String {
        let mut lines = vec![
            "digraph AgentTopology {".to_string(),
            "    rankdir=TB;".to_string(),
            "    node [shape=box, style=rounded];".to_string(),
            String::new(),
        ];

        // Node definitions
        if let Ok(nodes) = self.nodes.read() {
            for node in nodes.values() {
                let (shape, color) = match node.node_type {
                    NodeType::Orchestrator => ("diamond", "#FFB74D"),
                    NodeType::Subagent => ("box", "#64B5F6"),
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

        // Edge definitions
        if let Ok(edges) = self.edges.read() {
            for edge in edges.values() {
                let label = if let Some(l) = &edge.label {
                    let truncated = if l.chars().count() > 30 {
                        let t: String = l.chars().take(30).collect();
                        format!("{t}...")
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

    /// Export as JSON format (convenient for frontend rendering)
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

/// Topology data (for JSON serialization)
#[derive(Debug, Serialize, Deserialize)]
pub struct TopologyData {
    /// All nodes in the graph (Agents, tools, services, etc.)
    pub nodes: Vec<TopologyNode>,
    /// All edges in the graph (call relationships)
    pub edges: Vec<TopologyEdge>,
    /// Topology statistics
    pub stats: TopologyStats,
}

/// Topology statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyStats {
    /// Total number of nodes in the graph
    pub node_count: usize,
    /// Total number of edges in the graph (distinct call relationships)
    pub edge_count: usize,
    /// Sum of call counts across all edges
    pub total_calls: u64,
}

// ── TopologyCallback ─────────────────────────────────────────────────────────

/// Agent callback — Automatically records tool calls to the topology tracker
///
/// After adding this callback to an Agent, all tool calls will be automatically
/// recorded in the topology graph.
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
/// // After agent execution, the topology graph can be viewed
/// println!("{}", tracker.to_mermaid());
/// ```
pub struct TopologyCallback {
    tracker: Arc<TopologyTracker>,
}

impl TopologyCallback {
    /// Create a new topology callback instance.
    ///
    /// # Arguments
    /// - `tracker`: Topology tracker for recording Agent call relationships
    ///
    /// # Example
    /// ```
    /// use echo_agent::topology::{TopologyTracker, TopologyCallback};
    /// use std::sync::Arc;
    ///
    /// let tracker = Arc::new(TopologyTracker::new());
    /// let callback = TopologyCallback::new(tracker.clone());
    /// ```
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
                .add_node(TopologyNode::new(agent, NodeType::Subagent));
            self.tracker
                .add_node(TopologyNode::new(tool, NodeType::Tool));
            self.tracker.record_call(agent, tool, "call");
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
            self.tracker.record_call(agent, tool, "call failed");
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
        tracker.add_node(TopologyNode::new("agent_b", NodeType::Subagent));
        tracker.record_call("agent_a", "agent_b", "dispatch task");

        let stats = tracker.stats();
        assert_eq!(stats.node_count, 2);
        assert_eq!(stats.edge_count, 1);
        assert_eq!(stats.total_calls, 1);
    }

    #[test]
    fn test_topology_multiple_calls() {
        let tracker = TopologyTracker::new();

        tracker.add_node(TopologyNode::new("agent", NodeType::Subagent));
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
        tracker.add_node(TopologyNode::new("subagent", NodeType::Subagent));
        tracker.record_call("orchestrator", "subagent", "execute");

        let mermaid = tracker.to_mermaid();
        assert!(mermaid.starts_with("graph TD"));
        assert!(mermaid.contains("orchestrator"));
        assert!(mermaid.contains("subagent"));
    }

    #[test]
    fn test_topology_to_dot() {
        let tracker = TopologyTracker::new();
        tracker.add_node(TopologyNode::new("agent", NodeType::Subagent));
        tracker.record_call("agent", "tool1", "use");

        let dot = tracker.to_dot();
        assert!(dot.starts_with("digraph"));
        assert!(dot.contains("agent"));
        assert!(dot.contains("tool1"));
    }

    #[test]
    fn test_topology_to_json() {
        let tracker = TopologyTracker::new();
        tracker.add_node(TopologyNode::new("a", NodeType::Subagent));
        tracker.record_call("a", "b", "call");

        let json = tracker.to_json().unwrap();
        let data: TopologyData = serde_json::from_str(&json).unwrap();
        assert!(!data.nodes.is_empty());
        assert!(!data.edges.is_empty());
    }

    #[test]
    fn test_topology_clear() {
        let tracker = TopologyTracker::new();
        tracker.add_node(TopologyNode::new("a", NodeType::Subagent));
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
