//! demo60: Data Quality & Statistics — 数据质量评估与统计分析
//!
//! 演示数据质量和统计工具：
//! 1. MissingValueAnalysisTool — 缺失值分析与插补建议
//! 2. OutlierDetectionTool — IQR / Z-score 异常值检测
//! 3. ConsistencyCheckTool — 数据一致性校验
//! 4. ExploratoryStatisticsTool — 描述性分布摘要
//!
//! 需要 `data` 和 `statistics` features。
//!
//! Contract test: `contract_demo60_data_quality` (requires `data` and `statistics`).

use echo_agent::error::Result;
use echo_agent::tools::Tool;
use serde_json::json;
use std::collections::HashMap;
use std::fs;

/// Helper: build ToolParameters and execute a tool, returning the text output.
async fn run_tool(
    tool: &dyn Tool,
    params: serde_json::Value,
) -> std::result::Result<String, String> {
    let map: HashMap<String, serde_json::Value> = match params {
        serde_json::Value::Object(m) => m.into_iter().collect(),
        _ => HashMap::new(),
    };
    tool.execute(map)
        .await
        .map(|r| r.output.clone())
        .map_err(|e| format!("{e}"))
}

/// Pretty-print JSON text output (indented for readability).
fn print_json_preview(text: &str, max_lines: usize) {
    // Try to parse as JSON for pretty-printing
    let display = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| text.to_string()),
        Err(_) => text.to_string(),
    };
    let lines: Vec<&str> = display.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if i >= max_lines {
            println!("    … ({} more lines)", lines.len() - max_lines);
            break;
        }
        println!("    {line}");
    }
    println!();
}

#[tokio::test]
async fn contract_demo60_data_quality() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    println!("═══════════════════════════════════════════════════════");
    println!("    demo60: Data Quality & Statistics");
    println!("═══════════════════════════════════════════════════════\n");

    // ── 创建测试数据 ──────────────────────────────────────────────────────
    let csv_path = "/tmp/echo_demo60_data.csv";
    let csv_content = "\
name,age,salary,department,rating
张三,28,15000,技术部,4.2
李四,32,18000,销售部,3.8
王五,25,12000,技术部,4.5
赵六,35,22000,管理部,3.5
孙七,29,16000,市场部,4.0
周八,31,17000,技术部,
吴九,27,14000,销售部,3.2
郑十,33,,管理部,4.8
钱十一,22,95000,技术部,4.1
陈十二,-5,16500,市场部,3.9";

    fs::write(csv_path, csv_content)?;
    println!("  测试数据已创建: {csv_path}");
    println!("  注意数据中的问题：");
    println!("    - 郑十的 salary 缺失");
    println!("    - 周八的 rating 缺失");
    println!("    - 陈十二的 age 为负数 (-5)");
    println!("    - 钱十一的 salary 异常高 (95000)\n");

    // ── Part 1：缺失值分析 ────────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 1：MissingValueAnalysisTool — 缺失值分析");
    println!("───────────────────────────────────────────────────────\n");

    let missing_tool = echo_agent::tools::data_quality::MissingValueAnalysisTool;
    let output = run_tool(&missing_tool, json!({ "data_path": csv_path }))
        .await
        .unwrap_or_else(|e| format!("Error: {e}"));

    println!("  分析每列的缺失值数量、百分比和模式：\n");
    print_json_preview(&output, 40);

    // ── Part 2：异常值检测（IQR 方法） ────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 2：OutlierDetectionTool — IQR 异常值检测");
    println!("───────────────────────────────────────────────────────\n");

    let outlier_tool = echo_agent::tools::data_quality::OutlierDetectionTool;
    let output = run_tool(
        &outlier_tool,
        json!({
            "data_path": csv_path,
            "method": "iqr",
            "threshold": 1.5
        }),
    )
    .await
    .unwrap_or_else(|e| format!("Error: {e}"));

    println!("  使用 IQR 方法（k=1.5）检测异常值：\n");
    print_json_preview(&output, 30);

    // ── Part 3：异常值检测（Z-score 方法） ─────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 3：OutlierDetectionTool — Z-score 异常值检测");
    println!("───────────────────────────────────────────────────────\n");

    let output = run_tool(
        &outlier_tool,
        json!({
            "data_path": csv_path,
            "method": "zscore",
            "threshold": 2.0
        }),
    )
    .await
    .unwrap_or_else(|e| format!("Error: {e}"));

    println!("  使用 Z-score 方法（threshold=2.0）检测异常值：\n");
    print_json_preview(&output, 30);

    // ── Part 4：一致性检查 ────────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 4：ConsistencyCheckTool — 数据一致性校验");
    println!("───────────────────────────────────────────────────────\n");

    let consistency_tool = echo_agent::tools::data_quality::ConsistencyCheckTool;
    let output = run_tool(
        &consistency_tool,
        json!({
            "data_path": csv_path,
            "rules": json!([
                {"column": "age", "type": "range", "min": 0, "max": 100},
                {"column": "salary", "type": "range", "min": 3000, "max": 50000}
            ])
            .to_string()
        }),
    )
    .await
    .unwrap_or_else(|e| format!("Error: {e}"));

    println!("  自定义规则：age ∈ [0, 100]，salary ∈ [3000, 50000]\n");
    print_json_preview(&output, 30);

    // ── Part 5：高级描述统计 ──────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 5：ExploratoryStatisticsTool — 描述性分布摘要");
    println!("───────────────────────────────────────────────────────\n");

    let stats_tool = echo_agent::tools::statistics::ExploratoryStatisticsTool::default();
    let output = run_tool(
        &stats_tool,
        json!({
            "data_path": csv_path
        }),
    )
    .await
    .unwrap_or_else(|e| format!("Error: {e}"));

    println!("  计算分位数、偏度(skewness)和超额峰度(kurtosis)，不做统计推断：\n");
    print_json_preview(&output, 40);

    // ── Cleanup ───────────────────────────────────────────────────────────
    fs::remove_file(csv_path).ok();

    // ── Summary ───────────────────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("工具一览");
    println!("───────────────────────────────────────────────────────\n");
    println!("  data_quality 模块（需要 'data' feature）：");
    println!("    - MissingValueAnalysisTool : 缺失值模式分类 + 插补建议");
    println!("    - OutlierDetectionTool     : IQR / Z-score 异常值检测");
    println!("    - ConsistencyCheckTool     : 类型不匹配 + 范围校验 + 自定义规则");
    println!();
    println!("  statistics 模块（需要 'statistics' feature）：");
    println!("    - ExploratoryStatisticsTool: 描述性摘要，不输出 p 值或显著性结论");
    println!(
        "    - 正式推断                    : 生成 SciPy/statsmodels/R 脚本并通过 run_code 执行"
    );

    println!("\n═══════════════════════════════════════════════════════");
    println!("    demo60 完成");
    println!("═══════════════════════════════════════════════════════");

    Ok(())
}
