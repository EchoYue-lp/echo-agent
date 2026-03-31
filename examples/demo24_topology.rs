//! Agent 拓扑可视化示例
//!
//! 演示如何追踪 Agent 调用关系并导出为 Mermaid / DOT 格式。
//!
//! ```bash
//! cargo run --example demo24_topology
//! ```

use echo_agent::advanced::*;
use echo_agent::prelude::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    println!("=== Agent 拓扑可视化示例 ===\n");

    // ── 1. 手动构建拓扑图 ────────────────────────────────────
    println!("--- 1. 手动构建拓扑图 ---\n");

    let tracker = TopologyTracker::new();

    // 添加节点
    tracker.add_node(TopologyNode::new("orchestrator", NodeType::Orchestrator));
    tracker.add_node(
        TopologyNode::new("math_agent", NodeType::Worker)
            .with_label("数学计算 Agent")
            .with_metadata("model", "qwen3-max"),
    );
    tracker.add_node(TopologyNode::new("translator", NodeType::Worker).with_label("翻译 Agent"));
    tracker.add_node(TopologyNode::new("calculator", NodeType::Tool));
    tracker
        .add_node(TopologyNode::new("a2a_remote", NodeType::External).with_label("远程 A2A Agent"));

    // 记录调用关系
    tracker.record_call("orchestrator", "math_agent", "计算 100 的阶乘");
    tracker.record_call("orchestrator", "translator", "翻译结果");
    tracker.record_call("math_agent", "calculator", "factorial(100)");
    tracker.record_call("math_agent", "calculator", "pow(2, 10)");
    tracker.record_call("math_agent", "calculator", "sqrt(144)");
    tracker.record_call("orchestrator", "a2a_remote", "跨框架协作");

    // 打印统计
    let stats = tracker.stats();
    println!("拓扑统计:");
    println!("  节点数: {}", stats.node_count);
    println!("  边数: {}", stats.edge_count);
    println!("  总调用次数: {}", stats.total_calls);

    // ── 2. 导出 Mermaid 格式 ─────────────────────────────────
    println!("\n--- 2. Mermaid 流程图 ---\n");
    println!("{}", tracker.to_mermaid());

    // ── 3. 导出 DOT 格式 ────────────────────────────────────
    println!("\n--- 3. DOT (Graphviz) 格式 ---\n");
    println!("{}", tracker.to_dot());

    // ── 4. 导出 JSON 格式 ───────────────────────────────────
    println!("\n--- 4. JSON 格式 ---\n");
    println!("{}", tracker.to_json().unwrap());

    // ── 5. 通过 Callback 自动追踪 ────────────────────────────
    println!("\n--- 5. TopologyCallback 自动追踪 ---\n");

    let auto_tracker = Arc::new(TopologyTracker::new());
    let callback = Arc::new(TopologyCallback::new(auto_tracker.clone()));

    // 将 TopologyCallback 注册到 Agent
    let _agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("my_agent")
        .system_prompt("助手")
        .enable_tools()
        .callback(callback)
        .build()?;

    println!("TopologyCallback 已注册到 Agent。");
    println!("Agent 执行工具调用时，调用关系将自动记录到拓扑图中。");
    println!("执行完毕后，可通过以下方法查看拓扑:");
    println!("  tracker.to_mermaid()  — Mermaid 流程图");
    println!("  tracker.to_dot()     — Graphviz DOT");
    println!("  tracker.to_json()    — JSON 数据");

    println!("\n=== Agent 拓扑可视化示例完成 ===");
    Ok(())
}
