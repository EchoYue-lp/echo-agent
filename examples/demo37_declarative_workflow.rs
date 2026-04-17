//! demo37_declarative_workflow —— Workflow 声明式定义（YAML / JSON）
//!
//! 演示通过 YAML / JSON 文本声明图工作流结构，
//! 降低非 Rust 用户的使用门槛：
//! - `WorkflowDefinition::from_yaml_str()` / `from_json_str()`
//! - `workflow::loader::load_graph_from_yaml_str()` / `load_graph_from_json_str()`
//! - 支持条件分支、并行 fan-out 等高级拓扑
//!
//! ```bash
//! cargo run --example demo37_declarative_workflow
//! ```

use echo_agent::prelude::*;
use echo_agent::workflow::loader::{
    WorkflowDefinition, load_graph_from_json_str, load_graph_from_yaml_str,
};

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    println!("═══ Declarative Workflow Demo ═══\n");

    // ── 1. YAML 定义解析 ─────────────────────────────────────────────────
    println!("[1] YAML 定义解析");
    let yaml = r#"
name: data_pipeline
nodes:
  - name: hub
    type: router
  - name: worker_a
    type: router
  - name: worker_b
    type: router
  - name: merger
    type: router
edges:
  - from: hub
    parallel:
      - worker_a
      - worker_b
    then: merger
entry: hub
finish:
  - merger
max_steps: 50
"#;

    let def = WorkflowDefinition::from_yaml_str(yaml)?;
    println!("    name:      {}", def.name);
    println!("    nodes:     {} 个", def.nodes.len());
    println!("    edges:     {} 条", def.edges.len());
    println!("    entry:     {}", def.entry);
    println!("    finish:    {:?}", def.finish);
    println!("    max_steps: {:?}", def.max_steps);

    // ── 2. 从 YAML 直接构建 Graph ────────────────────────────────────────
    println!("\n[2] Graph::from_yaml_str()");
    let graph = load_graph_from_yaml_str(yaml)?;
    println!("    ✓ Graph '{}' built successfully", graph.name);

    let state = SharedState::new();
    let result = graph.run(state).await?;
    println!(
        "    execution: {} steps, path={:?}",
        result.steps, result.path
    );

    // ── 3. JSON 定义解析 ─────────────────────────────────────────────────
    println!("\n[3] JSON 定义解析");
    let json_str = r#"{
        "name": "approval_flow",
        "nodes": [
            { "name": "intake", "type": "router", "input_key": "input", "output_key": "output" },
            { "name": "review", "type": "router", "input_key": "input", "output_key": "output" },
            { "name": "approved", "type": "router", "input_key": "input", "output_key": "output" },
            { "name": "rejected", "type": "router", "input_key": "input", "output_key": "output" }
        ],
        "edges": [
            { "from": "intake", "to": "review" },
            {
                "from": "review",
                "condition": {
                    "key": "decision",
                    "equals": "yes",
                    "then": "approved",
                    "else": "rejected"
                }
            }
        ],
        "entry": "intake",
        "finish": ["approved", "rejected"]
    }"#;

    let graph = load_graph_from_json_str(json_str)?;
    println!("    ✓ Graph '{}' built from JSON", graph.name);

    // 测试条件分支：decision=yes → approved
    let state = SharedState::new();
    let _ = state.set("decision", "yes");
    let result = graph.run(state).await?;
    println!("    decision='yes' → path={:?}", result.path);
    assert!(result.path.contains(&"approved".to_string()));

    // 测试条件分支：decision=no → rejected
    let state = SharedState::new();
    let _ = state.set("decision", "no");
    let result = graph.run(state).await?;
    println!("    decision='no'  → path={:?}", result.path);
    assert!(result.path.contains(&"rejected".to_string()));

    // ── 4. Agent 节点的 YAML 定义 ────────────────────────────────────────
    println!("\n[4] Agent 节点定义（仅解析，不执行 LLM）");
    let agent_yaml = r#"
name: research_pipeline
nodes:
  - name: researcher
    type: agent
    model: qwen3-max
    system_prompt: "你是一个研究助手，擅长信息检索和总结"
    input_key: task
    output_key: research
  - name: writer
    type: agent
    model: qwen3-max
    system_prompt: "你是一个写作助手，擅长将研究内容转化为文章"
    input_key: research
    output_key: article
edges:
  - from: researcher
    to: writer
entry: researcher
finish:
  - writer
"#;

    let def = WorkflowDefinition::from_yaml_str(agent_yaml)?;
    println!("    工作流: {}", def.name);
    for node in &def.nodes {
        let prompt_preview = node.system_prompt.as_deref().map(|s| {
            let chars: String = s.chars().take(10).collect();
            if s.chars().count() > 10 {
                format!("{chars}...")
            } else {
                chars
            }
        });
        println!(
            "    节点 '{}': type={}, model={:?}, prompt={:?}",
            node.name,
            node.node_type,
            node.model.as_deref().unwrap_or("-"),
            prompt_preview,
        );
    }

    // 构建 Graph（包含 Agent 节点）
    let graph = def.build_graph()?;
    println!("    ✓ Graph '{}' with Agent nodes built", graph.name);

    // ── 5. 错误处理 ──────────────────────────────────────────────────────
    println!("\n[5] 错误处理");

    let bad_yaml = "invalid: [[[yaml: content";
    let err = WorkflowDefinition::from_yaml_str(bad_yaml);
    assert!(err.is_err());
    println!("    无效 YAML → Err ✓");

    let bad_json = r#"{"name": "test", "nodes": "not_array"}"#;
    let err = WorkflowDefinition::from_json_str(bad_json);
    assert!(err.is_err());
    println!("    无效 JSON → Err ✓");

    let missing_entry = r#"
name: bad_graph
nodes:
  - name: a
    type: router
edges: []
entry: nonexistent
finish: [a]
"#;
    let err = load_graph_from_yaml_str(missing_entry);
    assert!(err.is_err());
    println!("    不存在的 entry 节点 → Err ✓");

    println!("\n═══ Demo Complete ═══");
    Ok(())
}
