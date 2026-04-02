//! 声明式工作流加载器
//!
//! 支持从 YAML / JSON 文件加载图工作流定义，降低非 Rust 用户的使用门槛。
//!
//! # YAML 格式示例
//!
//! ```yaml
//! name: my_workflow
//! nodes:
//!   - name: researcher
//!     type: agent
//!     model: qwen3-max
//!     system_prompt: "你是一个研究助手"
//!     input_key: task
//!     output_key: research
//!   - name: writer
//!     type: agent
//!     model: qwen3-max
//!     system_prompt: "你是一个写作助手"
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
//! # JSON 格式示例
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

use crate::agent::react_agent::builder::ReactAgentBuilder;
use crate::error::{AgentError, ReactError, Result};
use crate::llm::config::LlmConfig;
use crate::workflow::graph::{Graph, GraphBuilder};
use crate::workflow::state::SharedState;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 工作流声明式定义
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowDefinition {
    /// 工作流名称
    pub name: String,
    /// 节点定义列表
    pub nodes: Vec<NodeDefinition>,
    /// 边定义列表
    pub edges: Vec<EdgeDefinition>,
    /// 入口节点名
    pub entry: String,
    /// 结束节点名列表
    #[serde(default)]
    pub finish: Vec<String>,
    /// 最大执行步数（可选，默认 100）
    #[serde(default)]
    pub max_steps: Option<usize>,
}

/// 节点定义
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodeDefinition {
    /// 节点名称
    pub name: String,
    /// 节点类型："agent" | "function" | "router"
    #[serde(rename = "type")]
    pub node_type: String,
    /// Agent 模型名称（仅 type=agent）
    #[serde(default)]
    pub model: Option<String>,
    /// Agent 系统提示词（仅 type=agent）
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// 从 state 读取输入的 key（仅 type=agent，默认 "input"）
    #[serde(default = "default_input_key")]
    pub input_key: String,
    /// 输出写入 state 的 key（仅 type=agent，默认 "output"）
    #[serde(default = "default_output_key")]
    pub output_key: String,
}

fn default_input_key() -> String {
    "input".to_string()
}
fn default_output_key() -> String {
    "output".to_string()
}

/// 边定义
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EdgeDefinition {
    /// 起始节点
    pub from: String,
    /// 目标节点（固定边）
    #[serde(default)]
    pub to: Option<String>,
    /// 条件表达式（条件边）——简单的 state key 检查
    #[serde(default)]
    pub condition: Option<ConditionDefinition>,
    /// 并行目标节点列表（fan-out 边）
    #[serde(default)]
    pub parallel: Option<Vec<String>>,
    /// 并行汇聚后的下一个节点
    #[serde(default)]
    pub then: Option<String>,
}

/// 条件定义（简化版：基于 state key 的值比较）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConditionDefinition {
    /// state 中的 key
    pub key: String,
    /// 期望值
    pub equals: serde_json::Value,
    /// 匹配时进入的节点
    pub then: String,
    /// 不匹配时进入的节点
    #[serde(rename = "else")]
    pub else_node: String,
}

impl WorkflowDefinition {
    /// 从 YAML 文件加载
    pub fn from_yaml(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref()).map_err(|e| {
            ReactError::Agent(AgentError::InitializationFailed(format!(
                "Failed to read workflow YAML file: {e}"
            )))
        })?;
        Self::from_yaml_str(&content)
    }

    /// 从 YAML 字符串解析
    pub fn from_yaml_str(yaml: &str) -> Result<Self> {
        serde_yaml::from_str(yaml).map_err(|e| {
            ReactError::Agent(AgentError::InitializationFailed(format!(
                "Failed to parse workflow YAML: {e}"
            )))
        })
    }

    /// 从 JSON 文件加载
    pub fn from_json(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref()).map_err(|e| {
            ReactError::Agent(AgentError::InitializationFailed(format!(
                "Failed to read workflow JSON file: {e}"
            )))
        })?;
        Self::from_json_str(&content)
    }

    /// 从 JSON 字符串解析
    pub fn from_json_str(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| {
            ReactError::Agent(AgentError::InitializationFailed(format!(
                "Failed to parse workflow JSON: {e}"
            )))
        })
    }

    /// 构建 Graph（不带 LLM 配置，Agent 使用环境变量配置）
    pub fn build_graph(self) -> Result<Graph> {
        self.build_graph_with_llm_config(None)
    }

    /// 构建 Graph（可选注入 LLM 配置）
    pub fn build_graph_with_llm_config(self, llm_config: Option<&LlmConfig>) -> Result<Graph> {
        let mut builder = GraphBuilder::new(&self.name);

        for node_def in &self.nodes {
            match node_def.node_type.as_str() {
                "agent" => {
                    let model = node_def.model.as_deref().unwrap_or("qwen3-max");
                    let prompt = node_def
                        .system_prompt
                        .as_deref()
                        .unwrap_or("你是一个有帮助的助手");

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
                    builder = builder.add_router_node(&node_def.name);
                }
                other => {
                    return Err(ReactError::Agent(AgentError::InitializationFailed(
                        format!("Unknown node type '{}' for node '{}'", other, node_def.name),
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

impl Graph {
    /// 从 YAML 文件加载图工作流
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::workflow::Graph;
    ///
    /// # fn main() -> echo_agent::error::Result<()> {
    /// let graph = Graph::from_yaml("workflow.yaml")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_yaml(path: impl AsRef<Path>) -> Result<Self> {
        WorkflowDefinition::from_yaml(path)?.build_graph()
    }

    /// 从 JSON 文件加载图工作流
    pub fn from_json(path: impl AsRef<Path>) -> Result<Self> {
        WorkflowDefinition::from_json(path)?.build_graph()
    }

    /// 从 YAML 字符串加载图工作流
    pub fn from_yaml_str(yaml: &str) -> Result<Self> {
        WorkflowDefinition::from_yaml_str(yaml)?.build_graph()
    }

    /// 从 JSON 字符串加载图工作流
    pub fn from_json_str(json: &str) -> Result<Self> {
        WorkflowDefinition::from_json_str(json)?.build_graph()
    }
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
    system_prompt: "你是助手"
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
        let graph = Graph::from_yaml_str(yaml).unwrap();
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
        let graph = Graph::from_yaml_str(yaml).unwrap();
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
        let graph = Graph::from_yaml_str(yaml).unwrap();
        assert_eq!(graph.name, "parallel_flow");
    }
}
