//! Declarative workflow loader
//!
//! Supports loading graph workflow definitions from YAML / JSON files, lowering
//! the entry barrier for non-Rust users.
//!
//! # YAML Format Example
//!
//! ```yaml
//! name: my_workflow
//! nodes:
//!   - name: researcher
//!     type: agent
//!     model: qwen3-max
//!     system_prompt: "You are a research assistant"
//!     input_key: task
//!     output_key: research
//!   - name: writer
//!     type: agent
//!     model: qwen3-max
//!     system_prompt: "You are a writing assistant"
//!     input_key: research
//!     output_key: result
//! edges:
//!   - from: researcher
//!     to: writer
//! entry: researcher
//! finish:
//!   - writer
//! max_steps: 50
//! ```
//!
//! # JSON format example
//!
//! ```json
//! {
//!   "name": "my_workflow",
//!   "nodes": [
//!     { "name": "researcher", "type": "agent", "model": "qwen3-max",
//!       "system_prompt": "...", "input_key": "task", "output_key": "research" }
//!   ],
//!   "edges": [
//!     { "from": "researcher", "to": "writer" }
//!   ],
//!   "entry": "researcher",
//!   "finish": ["writer"]
//! }
//! ```

use crate::agent::react::builder::ReactAgentBuilder;
use crate::error::{AgentError, ReactError, Result};
use crate::llm::config::LlmConfig;
use crate::workflow::Graph;
use crate::workflow::GraphBuilder;
use crate::workflow::SharedState;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Workflow declarative definition
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowDefinition {
    /// Workflow name
    pub name: String,
    /// List of node definitions
    pub nodes: Vec<NodeDefinition>,
    /// List of edge definitions
    pub edges: Vec<EdgeDefinition>,
    /// Entry node name
    pub entry: String,
    /// List of finish node names
    #[serde(default)]
    pub finish: Vec<String>,
    /// Maximum execution steps (optional, default 100)
    #[serde(default)]
    pub max_steps: Option<usize>,
}

/// Node definition
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodeDefinition {
    /// Node name
    pub name: String,
    /// Node type: "agent" | "function" | "router"
    #[serde(rename = "type")]
    pub node_type: String,
    /// Agent model name (only for type=agent)
    #[serde(default)]
    pub model: Option<String>,
    /// Agent system prompt (only for type=agent)
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Key to read input from state (only for type=agent, default "input")
    #[serde(default = "default_input_key")]
    pub input_key: String,
    /// Key to write output to state (only for type=agent, default "output")
    #[serde(default = "default_output_key")]
    pub output_key: String,
}

fn default_input_key() -> String {
    "input".to_string()
}
fn default_output_key() -> String {
    "output".to_string()
}

/// Edge definition
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EdgeDefinition {
    /// Source node
    pub from: String,
    /// Target node (fixed edge)
    #[serde(default)]
    pub to: Option<String>,
    /// Conditional expression (conditional edge) — simple state key check
    #[serde(default)]
    pub condition: Option<ConditionDefinition>,
    /// Parallel target node list (fan-out edge)
    #[serde(default)]
    pub parallel: Option<Vec<String>>,
    /// Next node after parallel convergence
    #[serde(default)]
    pub then: Option<String>,
}

/// Condition definition (simplified: value comparison based on state key)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConditionDefinition {
    /// Key in state
    pub key: String,
    /// Expected value
    pub equals: serde_json::Value,
    /// Node to go to on match
    pub then: String,
    /// Node to go to on mismatch
    #[serde(rename = "else")]
    pub else_node: String,
}

impl WorkflowDefinition {
    /// Load from a YAML file
    pub fn from_yaml(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref()).map_err(|e| {
            ReactError::Agent(Box::new(AgentError::InitializationFailed(format!(
                "Failed to read workflow YAML file: {e}"
            ))))
        })?;
        Self::from_yaml_str(&content)
    }

    /// Parse from a YAML string
    pub fn from_yaml_str(yaml: &str) -> Result<Self> {
        serde_yaml_ng::from_str(yaml).map_err(|e| {
            ReactError::Agent(Box::new(AgentError::InitializationFailed(format!(
                "Failed to parse workflow YAML: {e}"
            ))))
        })
    }

    /// Load from a JSON file
    pub fn from_json(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref()).map_err(|e| {
            ReactError::Agent(Box::new(AgentError::InitializationFailed(format!(
                "Failed to read workflow JSON file: {e}"
            ))))
        })?;
        Self::from_json_str(&content)
    }

    /// Parse from a JSON string
    pub fn from_json_str(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| {
            ReactError::Agent(Box::new(AgentError::InitializationFailed(format!(
                "Failed to parse workflow JSON: {e}"
            ))))
        })
    }

    /// Build Graph (without LLM config, Agent uses environment variable configuration)
    pub fn build_graph(self) -> Result<Graph> {
        self.build_graph_with_llm_config(None)
    }

    /// Build Graph (optional LLM configuration injection)
    pub fn build_graph_with_llm_config(self, llm_config: Option<&LlmConfig>) -> Result<Graph> {
        let mut builder = GraphBuilder::new(&self.name);

        for node_def in &self.nodes {
            match node_def.node_type.as_str() {
                "agent" => {
                    let model = node_def.model.as_deref().unwrap_or("qwen3-max");
                    let prompt = node_def
                        .system_prompt
                        .as_deref()
                        .unwrap_or("You are a helpful assistant");

                    let mut agent_builder = ReactAgentBuilder::new()
                        .name(&node_def.name)
                        .model(model)
                        .system_prompt(prompt);

                    if let Some(config) = llm_config {
                        agent_builder = agent_builder.llm_config(config.clone());
                    }

                    let agent = agent_builder.build()?;
                    builder = builder.add_agent_node(
                        &node_def.name,
                        agent,
                        &node_def.input_key,
                        &node_def.output_key,
                    );
                }
                "router" => {
                    builder = builder.add_router_node(&node_def.name);
                }
                "function" => {
                    return Err(ReactError::Agent(Box::new(
                        AgentError::InitializationFailed(format!(
                            "Node type 'function' is not yet supported for node '{}'. \
                             Use 'agent' or 'router' instead, or register a function node manually.",
                            node_def.name
                        )),
                    )));
                }
                other => {
                    return Err(ReactError::Agent(Box::new(
                        AgentError::InitializationFailed(format!(
                            "Unknown node type '{}' for node '{}'",
                            other, node_def.name
                        )),
                    )));
                }
            }
        }

        for edge_def in &self.edges {
            if let Some(ref to) = edge_def.to {
                builder = builder.add_edge(&edge_def.from, to);
            } else if let Some(ref cond) = edge_def.condition {
                let key = cond.key.clone();
                let expected = cond.equals.clone();
                let then = cond.then.clone();
                let else_node = cond.else_node.clone();

                builder =
                    builder.add_conditional_edge(&edge_def.from, move |state: &SharedState| {
                        let key = key.clone();
                        let expected = expected.clone();
                        let then = then.clone();
                        let else_node = else_node.clone();
                        Box::pin(async move {
                            let actual = state.get_raw(&key);
                            if actual.as_ref() == Some(&expected) {
                                then
                            } else {
                                else_node
                            }
                        })
                    });
            } else if let Some(ref targets) = edge_def.parallel {
                let then = edge_def
                    .then
                    .clone()
                    .unwrap_or_else(|| "__end__".to_string());
                builder = builder.add_parallel_edge(&edge_def.from, targets.clone(), then);
            }
        }

        builder = builder.set_entry(&self.entry);
        for finish in &self.finish {
            builder = builder.set_finish(finish);
        }

        let mut graph = builder.build()?;
        if let Some(max) = self.max_steps {
            graph.set_max_steps(max);
        }

        Ok(graph)
    }
}

/// Load a graph workflow from a YAML file
///
/// # Example
///
/// ```rust,no_run
/// use echo_agent::workflow::loader::load_graph_from_yaml;
///
/// # fn main() -> echo_agent::error::Result<()> {
/// let graph = load_graph_from_yaml("workflow.yaml")?;
/// # Ok(())
/// # }
/// ```
pub fn load_graph_from_yaml(path: impl AsRef<Path>) -> Result<crate::workflow::Graph> {
    WorkflowDefinition::from_yaml(path)?.build_graph()
}

/// Load a graph workflow from a JSON file
pub fn load_graph_from_json(path: impl AsRef<Path>) -> Result<crate::workflow::Graph> {
    WorkflowDefinition::from_json(path)?.build_graph()
}

/// Load a graph workflow from a YAML string
pub fn load_graph_from_yaml_str(yaml: &str) -> Result<crate::workflow::Graph> {
    WorkflowDefinition::from_yaml_str(yaml)?.build_graph()
}

/// Load a graph workflow from a JSON string
pub fn load_graph_from_json_str(json: &str) -> Result<crate::workflow::Graph> {
    WorkflowDefinition::from_json_str(json)?.build_graph()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_yaml_definition() {
        let yaml = r#"
name: test_workflow
nodes:
  - name: start
    type: router
  - name: worker
    type: agent
    model: qwen3-max
    system_prompt: "You are an assistant"
    input_key: task
    output_key: result
edges:
  - from: start
    to: worker
  - from: worker
    to: __end__
entry: start
finish: []
max_steps: 50
"#;

        let def = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        assert_eq!(def.name, "test_workflow");
        assert_eq!(def.nodes.len(), 2);
        assert_eq!(def.edges.len(), 2);
        assert_eq!(def.entry, "start");
        assert_eq!(def.max_steps, Some(50));
    }

    #[test]
    fn test_parse_json_definition() {
        let json = r#"{
            "name": "json_flow",
            "nodes": [
                { "name": "n1", "type": "router", "input_key": "input", "output_key": "output" },
                { "name": "n2", "type": "router", "input_key": "input", "output_key": "output" }
            ],
            "edges": [
                { "from": "n1", "to": "n2" }
            ],
            "entry": "n1",
            "finish": ["n2"]
        }"#;

        let def = WorkflowDefinition::from_json_str(json).unwrap();
        assert_eq!(def.name, "json_flow");
        assert_eq!(def.nodes.len(), 2);
    }

    #[test]
    fn test_build_graph_from_yaml() {
        let yaml = r#"
name: simple_graph
nodes:
  - name: hub
    type: router
  - name: end_node
    type: router
edges:
  - from: hub
    to: end_node
entry: hub
finish:
  - end_node
"#;
        let graph = load_graph_from_yaml_str(yaml).unwrap();
        assert_eq!(graph.name, "simple_graph");
    }

    #[test]
    fn test_conditional_edge_definition() {
        let yaml = r#"
name: cond_flow
nodes:
  - name: check
    type: router
  - name: yes_path
    type: router
  - name: no_path
    type: router
edges:
  - from: check
    condition:
      key: approved
      equals: true
      then: yes_path
      else: no_path
entry: check
finish:
  - yes_path
  - no_path
"#;
        let graph = load_graph_from_yaml_str(yaml).unwrap();
        assert_eq!(graph.name, "cond_flow");
    }

    #[test]
    fn test_parallel_edge_definition() {
        let yaml = r#"
name: parallel_flow
nodes:
  - name: start
    type: router
  - name: branch_a
    type: router
  - name: branch_b
    type: router
  - name: merge
    type: router
edges:
  - from: start
    parallel:
      - branch_a
      - branch_b
    then: merge
entry: start
finish:
  - merge
"#;
        let graph = load_graph_from_yaml_str(yaml).unwrap();
        assert_eq!(graph.name, "parallel_flow");
    }
}
