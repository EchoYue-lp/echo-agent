//! demo43: 数据处理工具演示（Excel / CSV / Word / Text）
//!
//! 展示内置的数据处理能力：
//! - Excel 文件读取和导出
//! - Word 文档读取
//! - 文本文件处理
//! - CSV/JSON/Parquet 数据处理
//!
//! ```bash
//! cargo run --example demo43_data_tools --features full
//! ```

use echo_agent::error::Result;
use echo_agent::prelude::*;
use echo_agent::testing::MockLlmClient;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::fs;
use std::sync::{Arc, Mutex};

#[derive(Default, Clone)]
struct ToolRecorder {
    calls: Arc<Mutex<Vec<String>>>,
}

impl ToolRecorder {
    fn names(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl AgentCallback for ToolRecorder {
    fn on_tool_start<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        _args: &'a Value,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(tool.to_string());
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("    Echo Agent 数据处理工具演示");
    println!("═══════════════════════════════════════════════════════\n");

    // 创建测试文件
    create_test_files()?;

    // ── Part 1: Excel 文件处理 ───────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 1: Excel 文件处理");
    println!("───────────────────────────────────────────────────────\n");

    let excel_path = "/tmp/echo_test_excel.xlsx";
    let task = format!(
        r#"请处理这个 Excel 文件：{}

步骤：
1. 使用 excel_info 获取工作表信息
2. 使用 read_excel 读取数据预览
3. 分析数据内容并总结"#,
        excel_path
    );

    println!("任务: {}\n", task);

    let result = run_scripted_task(
        "excel-agent",
        &task,
        vec![
            ("excel_info", json!({ "file_path": excel_path })),
            (
                "read_excel",
                json!({ "file_path": excel_path, "preview_rows": 5 }),
            ),
        ],
        "Excel 文件包含 1 个工作表，内容可被正常读取，适合继续做结构化分析。",
    )
    .await?;
    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo43 验收失败：Excel 处理返回空结果".to_string(),
        ));
    }
    println!("✓ 结果:\n{}\n", result);

    // ── Part 2: 文本文件处理 ─────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 2: 文本文件处理");
    println!("───────────────────────────────────────────────────────\n");

    let text_path = "/tmp/echo_test_text.txt";
    let task = format!(
        r#"请处理这个文本文件：{}

步骤：
1. 使用 text_stats 获取文本统计
2. 使用 search_text 搜索关键词 "数据"
3. 总结文件内容"#,
        text_path
    );

    println!("任务: {}\n", task);

    let result = run_scripted_task(
        "text-agent",
        &task,
        vec![
            ("text_stats", json!({ "file_path": text_path })),
            (
                "search_text",
                json!({ "file_path": text_path, "pattern": "数据" }),
            ),
        ],
        "文本文件统计和关键词搜索均已完成，文档主题集中在 Echo Agent 的本地数据处理能力。",
    )
    .await?;
    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo43 验收失败：文本处理返回空结果".to_string(),
        ));
    }
    println!("✓ 结果:\n{}\n", result);

    // ── Part 3: CSV 数据处理 ─────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 3: CSV 数据处理");
    println!("───────────────────────────────────────────────────────\n");

    let csv_path = "/tmp/echo_test_data.csv";
    let task = format!(
        r#"请处理这个 CSV 文件：{}

步骤：
1. 使用 data_read 读取 CSV 数据
2. 使用 data_stats 获取数据统计
3. 分析数据并给出洞察"#,
        csv_path
    );

    println!("任务: {}\n", task);

    let result = run_scripted_task(
        "csv-agent",
        &task,
        vec![
            (
                "read_data",
                json!({ "file_path": csv_path, "format": "csv", "preview_rows": 5 }),
            ),
            ("data_stats", json!({ "file_path": csv_path })),
        ],
        "CSV 文件已成功读取并完成统计，可继续做分组分析或可视化处理。",
    )
    .await?;
    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo43 验收失败：CSV 处理返回空结果".to_string(),
        ));
    }
    println!("✓ 结果:\n{}\n", result);

    // ── Part 4: Word 文档处理 ────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 4: Word 文档处理");
    println!("───────────────────────────────────────────────────────\n");

    // 创建一个简单的 Word 文档（模拟）
    println!("注意: Word 文档需要实际 .docx 文件进行测试");
    println!("可以使用 read_word 和 word_info 工具读取 .docx 文件\n");

    // ── Part 5: 综合数据处理任务 ─────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 5: 综合数据处理任务");
    println!("───────────────────────────────────────────────────────\n");

    let task = r#"我有一个数据处理的综合需求：

1. 读取 /tmp/echo_test_data.csv 文件
2. 分析其中的数值列统计信息
3. 给出数据处理建议

请使用 data_read、data_stats 等工具完成。"#;

    println!("任务: {}\n", task);

    let result = run_scripted_task(
        "data-summary-agent",
        task,
        vec![
            (
                "read_data",
                json!({ "file_path": "/tmp/echo_test_data.csv", "format": "csv", "preview_rows": 5 }),
            ),
            ("data_stats", json!({ "file_path": "/tmp/echo_test_data.csv" })),
        ],
        "数值列已经完成统计分析，建议下一步重点关注 salary 与 department 的分布差异。",
    )
    .await?;
    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo43 验收失败：综合数据处理返回空结果".to_string(),
        ));
    }
    println!("✓ 结果:\n{}\n", result);

    println!("═══════════════════════════════════════════════════════");
    println!("    Demo 完成");
    println!("═══════════════════════════════════════════════════════");

    // 清理测试文件
    cleanup_test_files();

    Ok(())
}

async fn run_scripted_task(
    agent_name: &str,
    task: &str,
    steps: Vec<(&str, Value)>,
    final_answer: &str,
) -> Result<String> {
    let recorder = ToolRecorder::default();
    let expected_tools: Vec<String> = steps.iter().map(|(name, _)| (*name).to_string()).collect();

    let mut mock = MockLlmClient::new().with_model_name(format!("{agent_name}-mock"));
    for (idx, (tool_name, args)) in steps.iter().enumerate() {
        mock = mock.then_tool_call(
            format!("call_{idx}_{tool_name}"),
            *tool_name,
            serde_json::to_string(args).unwrap(),
        );
    }
    mock = mock.then_tool_call(
        "call_final_answer",
        "final_answer",
        serde_json::to_string(&json!({ "answer": final_answer })).unwrap(),
    );

    let mut agent = ReactAgentBuilder::new()
        .name(agent_name)
        .llm_client(Arc::new(mock))
        .system_prompt("你是一个数据处理助手，请严格按既定步骤调用工具并总结结果。")
        .enable_tools()
        .max_iterations(10)
        .callback(Arc::new(recorder.clone()))
        .build()?;

    let result = agent.execute(task).await?;
    let actual_tools = recorder.names();
    for expected_tool in &expected_tools {
        if !actual_tools.iter().any(|name| name == expected_tool) {
            return Err(echo_agent::error::ReactError::Other(format!(
                "demo43 验收失败：脚本化任务未调用预期工具 `{expected_tool}`"
            )));
        }
    }
    Ok(result)
}

/// 创建测试文件
fn create_test_files() -> Result<()> {
    // 创建测试文本文件
    let text_content = r#"Echo Agent 数据处理工具演示文档

本测试文档用于演示 Echo Agent 的文本处理能力。

一、数据概述
本系统支持多种数据格式的处理，包括：
- Excel 文件（.xlsx, .xls, .xlsb, .ods）
- Word 文档（.docx）
- 文本文件（支持多种编码）
- CSV/JSON/Parquet 数据文件

二、主要功能
1. 文件读取：支持读取各种格式的数据文件
2. 数据分析：提供统计分析、数据过滤等功能
3. 数据转换：支持格式转换和数据导出

三、数据安全
所有数据处理均在本地进行，确保数据安全。

四、总结
Echo Agent 提供了强大的数据处理能力，适合各种数据分析场景。
"#;

    fs::write("/tmp/echo_test_text.txt", text_content)?;

    // 创建测试 CSV 文件
    let csv_content = "name,age,salary,department
张三,28,15000,技术部
李四,32,18000,销售部
王五,25,12000,技术部
赵六,35,22000,管理部
孙七,29,16000,市场部
周八,31,17000,技术部
吴九,27,14000,销售部
郑十,33,20000,管理部";

    fs::write("/tmp/echo_test_data.csv", csv_content)?;

    // 创建真正的 Excel 文件（xlsx 格式）
    create_test_xlsx()?;

    println!("测试文件已创建:");
    println!("  - /tmp/echo_test_text.txt");
    println!("  - /tmp/echo_test_data.csv");
    println!("  - /tmp/echo_test_excel.xlsx");
    println!();
    Ok(())
}

/// 清理测试文件
fn cleanup_test_files() {
    fs::remove_file("/tmp/echo_test_text.txt").ok();
    fs::remove_file("/tmp/echo_test_data.csv").ok();
    fs::remove_file("/tmp/echo_test_excel.xlsx").ok();
    fs::remove_file("/tmp/echo_test_data.xlsx.csv").ok();
    fs::remove_file("/tmp/echo_test_doc.pdf").ok();
}

/// 创建真正的 xlsx 测试文件（通过 base64 解码嵌入的最小有效 xlsx）
fn create_test_xlsx() -> Result<()> {
    use base64::Engine;
    let xlsx_base64 = "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";

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(xlsx_base64)
        .expect("Invalid base64 xlsx data");

    fs::write("/tmp/echo_test_excel.xlsx", bytes)?;
    Ok(())
}
