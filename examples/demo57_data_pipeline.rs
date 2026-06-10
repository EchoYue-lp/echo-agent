//! demo57: Data Pipeline — 端到端数据分析流水线
//!
//! 演示 `DataPipelineConfig` 和 `run_data_pipeline` 的用法：
//! 1. 构建 DataPipelineConfig（dataset_path / objective / max_charts）
//! 2. 使用 MockAgent 运行 5 阶段流水线：
//!    init → load_data → profile → analyze → visualize → summarize
//! 3. 从 SharedState 中提取每个阶段的中间结果
//!
//! ```bash
//! cargo run --example demo57_data_pipeline --features testing
//! ```

use echo_agent::error::Result;
use echo_agent::testing::MockAgent;
use echo_agent::workflow::pipelines::data_pipeline::{DataPipelineConfig, run_data_pipeline};
use echo_agent::workflow::shared_agent;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,echo_orchestration=debug")
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("    demo57: Data Pipeline 端到端数据分析流水线");
    println!("═══════════════════════════════════════════════════════\n");

    // ── Part 1：DataPipelineConfig 结构展示 ────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 1：DataPipelineConfig 配置结构");
    println!("───────────────────────────────────────────────────────\n");

    let config = DataPipelineConfig::new("/data/sales_2024.csv")
        .with_objective("Identify revenue trends and seasonal patterns")
        .with_max_charts(5);

    println!("  dataset_path : {}", config.dataset_path);
    println!("  objective    : {:?}", config.objective);
    println!("  max_charts   : {}", config.max_charts);
    println!();

    // Default config
    let default_config = DataPipelineConfig::default();
    println!("  [默认配置]");
    println!("  dataset_path : '{}'", default_config.dataset_path);
    println!("  objective    : {:?}", default_config.objective);
    println!("  max_charts   : {}", default_config.max_charts);
    println!();

    // ── Part 2：运行 5 阶段分析流水线 ──────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 2：运行数据分析流水线（MockAgent）");
    println!("───────────────────────────────────────────────────────\n");

    // Create a MockAgent with 5 staged responses (one per agent node):
    //   load_data → profile → analyze → visualize → summarize
    let mock = MockAgent::new("data_analyst")
        .with_response(
            "Dataset loaded: 10,000 rows × 8 columns. \
             Columns: date, product, region, quantity, price, revenue, cost, profit. \
             Date range: 2024-01-01 to 2024-12-31.",
        )
        .with_response(
            "Profile complete. Numeric columns: quantity (mean=45.2, std=12.8), \
             price (mean=99.5, std=35.1), revenue (mean=4500, std=1800). \
             Missing values: 0.2% in region column.",
        )
        .with_response(
            "Analysis findings: (1) Revenue peaks in Q4 (+32% vs Q1). \
             (2) Product category A drives 60% of revenue. \
             (3) Strong positive correlation (r=0.87) between quantity and revenue.",
        )
        .with_response(
            "Visualization plan: (1) Line chart — monthly revenue trend. \
             (2) Bar chart — revenue by product category. \
             (3) Scatter plot — quantity vs revenue with regression line.",
        )
        .with_response(
            "Executive Summary: Revenue grew 18% YoY with strong Q4 seasonality. \
             Product A is the primary revenue driver. \
             Recommendation: increase Q4 inventory for Product A by 25%.",
        );

    let agent = shared_agent(mock);
    let pipeline_config = DataPipelineConfig::new("/data/sales_2024.csv")
        .with_objective("Identify revenue trends and seasonal patterns")
        .with_max_charts(3);

    println!("  启动流水线…\n");
    let result = run_data_pipeline(&agent, pipeline_config).await?;

    println!("  流水线完成！共 {} 步", result.steps);
    println!("  执行路径: {:?}", result.path);
    println!();

    // ── Part 3：提取各阶段中间结果 ─────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 3：各阶段中间结果（SharedState）");
    println!("───────────────────────────────────────────────────────\n");

    let keys = [
        ("loaded_data", "Stage 1 — 数据加载"),
        ("data_profile", "Stage 2 — 数据画像"),
        ("analysis", "Stage 3 — 统计分析"),
        ("visualizations", "Stage 4 — 可视化方案"),
        ("summary", "Stage 5 — 执行摘要"),
    ];

    for (key, label) in &keys {
        let value: String = result.state.get(key).unwrap_or_default();
        let preview = if value.chars().count() > 120 {
            let truncated: String = value.chars().take(120).collect();
            format!("{truncated}…")
        } else {
            value.clone()
        };
        println!("  {label}");
        println!("    key   : {key}");
        println!("    value : {preview}");
        println!();
    }

    // ── Part 4：流水线架构说明 ─────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 4：流水线架构");
    println!("───────────────────────────────────────────────────────\n");

    println!("  数据流水线由 GraphBuilder 构建，包含以下节点：\n");
    println!("  init ──→ load_prompt ──→ load_data");
    println!("       ──→ profile_prompt ──→ profile");
    println!("       ──→ analyze_prompt ──→ analyze");
    println!("       ──→ visualize_prompt ──→ visualize");
    println!("       ──→ summarize_prompt ──→ summarize (finish)");
    println!();
    println!("  每个 stage 由两部分组成：");
    println!("    1. function_node：从 SharedState 构造 prompt");
    println!("    2. shared_agent_node：用 SharedAgent 执行 prompt");
    println!();
    println!("  所有中间结果写入 SharedState，下游节点可自由引用上游输出。");

    println!("\n═══════════════════════════════════════════════════════");
    println!("    demo57 完成");
    println!("═══════════════════════════════════════════════════════");

    Ok(())
}
