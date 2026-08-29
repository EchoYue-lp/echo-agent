//! 综合示例：智能数据分析助手
//!
//! 展示 echo-agent 在数据分析场景中的完整能力：
//!
//! ## 功能清单
//!
//! | 功能模块 | 实现方式 |
//! |---------|---------|
//! | 语义搜索 | `SqliteStore` + `HttpEmbedder` 语义检索 |
//! | 持久化存储 | `SqliteStore` 保存分析历史 |
//! | 结构化输出 | `extract<T>()` 提取统计数据 |
//! | 文件处理 | `FileSystemSkill` + 数据解析工具 |
//! | Workflow | `GraphBuilder` 数据处理流水线 |
//! | 流式输出 | `execute_stream()` 实时进度 |
//!
//! ## 运行方式
//!
//! ```bash
//! # 基础运行（需要 LLM API Key + Embedding API）
//! QWEN_API_KEY=your_key EMBEDDING_APIKEY=your_key cargo run -p echo-agent-learning --example demo46_data_analyst --features sqlite
//! ```

use echo_agent::memory::store::Store;
use echo_agent::memory::{Embedder, HttpEmbedder, SearchQuery, SqliteStore};
use echo_agent::prelude::*;
use echo_agent::workflow::{GraphBuilder, SharedState};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 结构化输出类型
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SalesAnalysis {
    period: String,
    total_revenue: f64,
    growth_rate: f64,
    top_products: Vec<ProductSale>,
    insights: Vec<String>,
    recommendations: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProductSale {
    name: String,
    quantity: i32,
    revenue: f64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DataQualityReport {
    total_rows: i32,
    null_count: i32,
    duplicate_count: i32,
    quality_score: f64,
    issues: Vec<String>,
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Main
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=info,data_analyst=info".into()),
        )
        .init();

    print_banner();

    println!("📊 正在初始化智能数据分析助手...\n");

    let db_path = data_analyst_db_path();
    cleanup_sqlite_files(&db_path);

    // ── Part 1: 语义搜索存储 ─────────────────────────────────────────────────────
    demo_semantic_storage(&db_path).await?;

    // ── Part 2: 结构化数据分析 ───────────────────────────────────────────────────
    demo_structured_analysis().await?;

    // ── Part 3: 数据质量检查 ─────────────────────────────────────────────────────
    demo_data_quality_check().await?;

    // ── Part 4: 数据处理流水线 ───────────────────────────────────────────────────
    demo_processing_pipeline().await?;

    // ── Part 5: 历史分析检索 ─────────────────────────────────────────────────────
    demo_history_retrieval(&db_path).await?;

    println!("\n═══════════════════════════════════════════════════════");
    println!("              综合示例演示完成！");
    println!("═══════════════════════════════════════════════════════");

    cleanup_sqlite_files(&db_path);

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 1: 语义搜索存储
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_semantic_storage(db_path: &Path) -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 1: 语义搜索存储");
    println!("═══════════════════════════════════════════════════════\n");

    let store = Arc::new(SqliteStore::with_embedder(
        db_path,
        load_verified_embedder_from_config().await?,
    )?);

    let ns = &["data_analyst", "reports"];

    // 存储历史分析报告（中英文混合）
    let reports = vec![
        (
            "q1_2024",
            json!({
                "content": "2024年Q1销售额达到500万，同比增长15%，主要来自新产品线",
                "period": "2024 Q1",
                "revenue": 5000000,
                "tags": ["销售", "季度报告"]
            }),
        ),
        (
            "customer_churn",
            json!({
                "content": "客户流失分析显示，由于客服支持改善，流失率下降了5%",
                "period": "2024 Q1",
                "type": "客户流失分析",
                "tags": ["流失", "客服", "支持"]
            }),
        ),
        (
            "product_performance",
            json!({
                "content": "产品A表现优异，占总销售额40%，产品B需要改进",
                "period": "2024 Q1",
                "tags": ["产品", "绩效"]
            }),
        ),
    ];

    for (key, value) in &reports {
        store.put(ns, key, value.clone()).await?;
    }

    println!(
        "  ✓ 已存储 {} 条分析报告（用于验证语义检索）\n",
        reports.len()
    );

    // 演示语义搜索
    println!("  🔍 语义搜索测试:\n");

    let queries = [
        (
            "销售增长主要来自新产品线",
            "中文语义查询「销售增长主要来自新产品线」",
        ),
        (
            "客服支持改善降低了客户流失",
            "中文语义查询「客服支持改善降低了客户流失」",
        ),
        ("产品B需要改进", "中文语义查询「产品B需要改进」"),
    ];

    for (query, desc) in &queries {
        let keyword_hits = store.search(ns, query, 3).await?;
        let results = store
            .search_with(ns, SearchQuery::semantic(query, 3))
            .await?;
        println!("    查询: \"{}\" ({})", query, desc);
        println!("      关键词命中: {}", keyword_hits.len());

        if results.is_empty() {
            return Err(echo_agent::error::ReactError::Other(format!(
                "语义检索验收失败：查询 `{query}` 没有命中，说明 embedding 索引或语义检索链路不可用"
            )));
        }

        for (i, item) in results.iter().take(2).enumerate() {
            let content = item
                .value
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(50)
                .collect::<String>();
            println!(
                "      [{}] {} (相似度: {:.2}) - {}...",
                i + 1,
                item.key,
                item.score.unwrap_or(0.0),
                content
            );
        }
        println!();
    }

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 2: 结构化数据分析
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_structured_analysis() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 2: 结构化数据分析");
    println!("═══════════════════════════════════════════════════════\n");

    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("sales-analyst")
        .system_prompt("你是销售数据分析专家，擅长从数据中提取洞察并生成结构化报告。")
        .max_iterations(5)
        .build()?;

    println!("  📋 分析任务: Q1 销售数据分析\n");

    let schema = typed_response_format::<SalesAnalysis>("sales_analysis")?;

    let prompt = r#"请分析以下 Q1 销售数据，并返回一个严格匹配 schema 的 JSON 对象。
不要输出 Markdown，不要省略任何字段。

字段要求：
- period: 字符串，例如 "2024 Q1"
- total_revenue: 数字，总销售额
- growth_rate: 数字，使用小数表达增长率，例如 18% 写成 0.18
- top_products: 数组，元素必须包含 name / quantity / revenue
- insights: 字符串数组，至少 3 条
- recommendations: 字符串数组，至少 2 条

产品销售数据：
- 产品A: 1200件，单价¥200，销售额¥240,000
- 产品B: 800件，单价¥350，销售额¥280,000
- 产品C: 500件，单价¥500，销售额¥250,000
- 产品D: 1500件，单价¥80，销售额¥120,000

总销售额: ¥890,000
相比去年同期增长: 18%

请给出：
1. 销售额排名前 3 的产品
2. 至少 3 条关键洞察
3. 至少 2 条改进建议"#;

    let analysis: SalesAnalysis = agent.extract(prompt, schema).await?;
    println!("  ✓ 结构化分析完成:\n");
    println!("    分析周期: {}", analysis.period);
    println!("    总销售额: ¥{:.2}", analysis.total_revenue);
    println!("    增长率: {:.1}%", analysis.growth_rate * 100.0);
    println!("\n    热销产品 TOP 3:");
    for (i, product) in analysis.top_products.iter().take(3).enumerate() {
        println!(
            "      {}. {} - {}件, ¥{:.2}",
            i + 1,
            product.name,
            product.quantity,
            product.revenue
        );
    }
    println!("\n    关键洞察:");
    for insight in &analysis.insights {
        println!("      • {}", insight);
    }
    println!("\n    改进建议:");
    for rec in &analysis.recommendations {
        println!("      • {}", rec);
    }
    println!();

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 3: 数据质量检查
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_data_quality_check() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 3: 数据质量检查");
    println!("═══════════════════════════════════════════════════════\n");

    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("quality-checker")
        .system_prompt("你是数据质量专家，擅长检查数据集的完整性和准确性。")
        .max_iterations(5)
        .build()?;

    println!("  📋 数据集描述:\n");
    println!("    用户数据集 (users.csv):");
    println!("    - 总行数: 10,000");
    println!("    - 空值: email字段有150个空值");
    println!("    - 重复: 检测到25个重复用户");
    println!("    - 格式: phone字段有80个不符合格式\n");

    let schema = typed_response_format::<DataQualityReport>("data_quality")?;

    let prompt = "请评估以下用户数据集的质量，并返回一个严格匹配 schema 的 JSON 对象。
不要输出 Markdown，不要省略任何字段。

字段要求：
- total_rows: 总行数
- null_count: 全部空值数量
- duplicate_count: 重复记录数量
- quality_score: 0 到 100 的数字
- issues: 主要问题的字符串数组

请把 phone 格式错误和 age 异常值归纳进 issues，而不是创建额外字段。

用户数据集 users.csv:
- 总行数: 10,000
- email字段空值: 150个
- phone字段格式错误: 80个
- 重复用户记录: 25个
- age字段异常值(>150): 5个";

    let report: DataQualityReport = agent.extract(prompt, schema).await?;
    println!("  ✓ 质量检查完成:\n");
    println!("    数据行数: {}", report.total_rows);
    println!("    空值数量: {}", report.null_count);
    println!("    重复数量: {}", report.duplicate_count);
    println!("    质量分数: {:.1}/100", report.quality_score);
    println!("\n    发现的问题:");
    for issue in &report.issues {
        println!("      • {}", issue);
    }
    println!();

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 4: 数据处理流水线
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_processing_pipeline() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 4: 数据处理流水线");
    println!("═══════════════════════════════════════════════════════\n");

    // 创建数据处理工作流
    let graph = GraphBuilder::new("etl_pipeline")
        .add_function_node("validate", |state: &SharedState| {
            Box::pin(async move {
                println!("    ▶ 验证数据格式...");
                let _ = state.set("validation_status", "passed");
                let _ = state.set("invalid_rows", 3i64);
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                Ok(())
            })
        })
        .add_function_node("clean", |state: &SharedState| {
            Box::pin(async move {
                println!("    ▶ 清洗数据（去重、填充空值）...");
                let _ = state.set("cleaned_rows", 997i64);
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                Ok(())
            })
        })
        .add_function_node("transform", |state: &SharedState| {
            Box::pin(async move {
                println!("    ▶ 转换数据格式...");
                let _ = state.set("transformed_rows", 997i64);
                tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
                Ok(())
            })
        })
        .add_function_node("aggregate", |state: &SharedState| {
            Box::pin(async move {
                println!("    ▶ 聚合统计...");
                let cleaned: i64 = state.get("cleaned_rows").unwrap_or(0);
                let _ = state.set("total_records", cleaned);
                tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
                Ok(())
            })
        })
        .set_entry("validate")
        .add_edge("validate", "clean")
        .add_edge("clean", "transform")
        .add_edge("transform", "aggregate")
        .set_finish("aggregate")
        .build()?;

    println!("  执行 ETL 流水线:\n");

    let state = SharedState::new();
    let result = graph.run(state).await?;

    println!("\n  ✓ 流水线完成");
    println!("    执行路径: {:?}", result.path);
    println!("    总步骤数: {}", result.steps);
    println!(
        "    处理记录数: {}",
        result.state.get::<i64>("total_records").unwrap_or(0)
    );
    println!();

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 5: 历史分析检索
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_history_retrieval(db_path: &Path) -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 5: 历史分析检索");
    println!("══════════════════════════━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let store = Arc::new(SqliteStore::with_embedder(
        db_path,
        load_verified_embedder_from_config().await?,
    )?);
    let ns = &["data_analyst", "history"];

    // 存储分析历史
    let analyses = vec![
        (
            "monthly_2024_01",
            json!({
                "content": "2024年1月数据分析：销售额增长12%，新客户增加20%",
                "month": "2024-01",
                "metrics": {"revenue": 1200000, "customers": 450}
            }),
        ),
        (
            "monthly_2024_02",
            json!({
                "content": "2024年2月数据分析：销售额增长8%，客户满意度提升",
                "month": "2024-02",
                "metrics": {"revenue": 1296000, "customers": 478}
            }),
        ),
        (
            "monthly_2024_03",
            json!({
                "content": "2024年3月数据分析：季节性促销活动效果显著，销售额增长25%",
                "month": "2024-03",
                "metrics": {"revenue": 1620000, "customers": 550}
            }),
        ),
    ];

    for (key, value) in &analyses {
        store.put(ns, key, value.clone()).await?;
    }

    println!("  ✓ 已存储 {} 条历史分析\n", analyses.len());

    // 检索相关历史
    println!("  🔍 检索相关历史:\n");

    let queries = ["季节性促销活动效果", "新客户增长情况", "客户满意度提升"];

    for query in &queries {
        let keyword_hits = store.search(ns, query, 2).await?;
        let results = store.search_with(ns, SearchQuery::hybrid(query, 2)).await?;
        println!("    查询: \"{}\"", query);
        println!("      关键词命中: {}", keyword_hits.len());
        if results.is_empty() {
            return Err(echo_agent::error::ReactError::Other(format!(
                "历史检索验收失败：查询 `{query}` 没有命中，说明历史检索链路不可用"
            )));
        }
        for (i, item) in results.iter().enumerate() {
            let content = item
                .value
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(60)
                .collect::<String>();
            let month = item
                .value
                .get("month")
                .and_then(Value::as_str)
                .unwrap_or("unknown month");
            println!("      [{}] {}: {}...", i + 1, month, content);
        }
        println!();
    }

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 辅助函数
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

fn print_banner() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          Echo Agent 智能数据分析助手 - 综合示例            ║");
    println!("║                                                                ║");
    println!("║  展示核心能力：                                                 ║");
    println!("║  • 语义搜索 • 持久化存储 • 结构化输出 • 文件处理              ║");
    println!("║  • Workflow 流水线 • 历史检索                                 ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");
}

fn typed_response_format<T: JsonSchema>(name: &str) -> Result<ResponseFormat> {
    let schema = schema_for!(T);
    let schema_value = serde_json::to_value(schema)?;
    Ok(ResponseFormat::json_schema(name, schema_value))
}

fn data_analyst_db_path() -> PathBuf {
    std::env::temp_dir().join(format!("echo_agent_data_analyst_{}.db", std::process::id()))
}

fn cleanup_sqlite_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

fn load_embedder_from_config() -> Result<Arc<dyn Embedder>> {
    Ok(Arc::new(HttpEmbedder::from_env()))
}

async fn load_verified_embedder_from_config() -> Result<Arc<dyn Embedder>> {
    let embedder = load_embedder_from_config()?;
    embedder
        .embed("echo-agent embedding health check")
        .await
        .map_err(|e| {
            echo_agent::error::ReactError::Other(format!(
                "embedding 服务健康检查失败，无法完成综合验收示例: {e}"
            ))
        })?;
    Ok(embedder)
}
