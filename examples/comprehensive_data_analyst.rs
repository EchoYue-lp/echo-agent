//! 综合示例：智能数据分析助手
//!
//! 展示 echo-agent 在数据分析场景中的完整能力：
//!
//! ## 功能清单
//!
//! | 功能模块 | 实现方式 |
//! |---------|---------|
//! | 语义搜索 | `EmbeddingStore` + `HttpEmbedder` 跨语言检索 |
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
//! QWEN_API_KEY=your_key EMBEDDING_APIKEY=your_key cargo run --example comprehensive_data_analyst --features "sqlite web"
//! ```

use echo_agent::memory::{EmbeddingStore, HttpEmbedder, SqliteStore};
use echo_agent::prelude::*;
use echo_agent::workflow::{GraphBuilder, SharedState};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 结构化输出类型
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[derive(Debug, Serialize, Deserialize)]
struct SalesAnalysis {
    period: String,
    total_revenue: f64,
    growth_rate: f64,
    top_products: Vec<ProductSale>,
    insights: Vec<String>,
    recommendations: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProductSale {
    name: String,
    quantity: i32,
    revenue: f64,
}

#[derive(Debug, Serialize, Deserialize)]
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
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=info,data_analyst=info".into()),
        )
        .init();

    print_banner();

    if !has_llm_config() {
        println!("⚠️  未检测到 LLM API 密钥\n");
        println!("请设置环境变量：");
        println!("  - QWEN_API_KEY");
        println!("  - EMBEDDING_APIKEY (可选，用于语义搜索)\n");
        return Ok(());
    }

    println!("📊 正在初始化智能数据分析助手...\n");

    // ── Part 1: 语义搜索存储 ─────────────────────────────────────────────────────
    demo_semantic_storage().await?;

    // ── Part 2: 结构化数据分析 ───────────────────────────────────────────────────
    demo_structured_analysis().await?;

    // ── Part 3: 数据质量检查 ─────────────────────────────────────────────────────
    demo_data_quality_check().await?;

    // ── Part 4: 数据处理流水线 ───────────────────────────────────────────────────
    demo_processing_pipeline().await?;

    // ── Part 5: 历史分析检索 ─────────────────────────────────────────────────────
    demo_history_retrieval().await?;

    println!("\n═══════════════════════════════════════════════════════");
    println!("              综合示例演示完成！");
    println!("═══════════════════════════════════════════════════════");

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 1: 语义搜索存储
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_semantic_storage() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 1: 语义搜索存储");
    println!("═══════════════════════════════════════════════════════\n");

    // 检查是否设置了 Embedding API key
    let has_embedding_key =
        std::env::var("EMBEDDING_APIKEY").is_ok() || std::env::var("OPENAI_API_KEY").is_ok();

    if !has_embedding_key {
        println!("  [跳过] 未设置 EMBEDDING_APIKEY 或 OPENAI_API_KEY，跳过语义搜索演示\n");
        return Ok(());
    }

    // 初始化 Embedder
    let embedder: Arc<dyn echo_agent::memory::Embedder> = Arc::new(HttpEmbedder::from_env());

    let db_path = std::env::temp_dir().join("echo_agent_data_analyst.db");
    let sqlite_store = Arc::new(SqliteStore::with_embedder(&db_path, embedder.clone())?);
    let embedding_store = Arc::new(EmbeddingStore::new(sqlite_store.clone(), embedder));

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
                "content": "Customer churn analysis shows 5% decrease due to improved support",
                "period": "2024 Q1",
                "type": "客户流失分析",
                "tags": ["churn", "support"]
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
        embedding_store.put(ns, key, value.clone()).await?;
    }

    println!(
        "  ✓ 已存储 {} 条分析报告（支持跨语言检索）\n",
        reports.len()
    );

    // 演示语义搜索
    println!("  🔍 语义搜索测试:\n");

    let queries = [
        ("销售数据", "中文查询「销售数据」"),
        ("revenue growth", "英文查询「收入增长」"),
        ("产品表现", "中文查询「产品表现」"),
    ];

    for (query, desc) in &queries {
        let results = embedding_store.semantic_search(ns, query, 3).await?;
        println!("    查询: \"{}\" ({})", query, desc);

        if results.is_empty() {
            println!("      → 无结果");
        } else {
            for (i, item) in results.iter().take(2).enumerate() {
                let content = item.value["content"]
                    .as_str()
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

    let schema = ResponseFormat::json_schema(
        "sales_analysis",
        json!({
            "type": "object",
            "properties": {
                "period": {"type": "string"},
                "total_revenue": {"type": "number"},
                "growth_rate": {"type": "number"},
                "top_products": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "quantity": {"type": "integer"},
                            "revenue": {"type": "number"}
                        }
                    }
                },
                "insights": {"type": "array", "items": {"type": "string"}},
                "recommendations": {"type": "array", "items": {"type": "string"}}
            }
        }),
    );

    let prompt = r#"请分析以下Q1销售数据，并生成结构化报告：

产品销售数据：
- 产品A: 1200件，单价¥200，销售额¥240,000
- 产品B: 800件，单价¥350，销售额¥280,000
- 产品C: 500件，单价¥500，销售额¥250,000
- 产品D: 1500件，单价¥80，销售额¥120,000

总销售额: ¥890,000
相比去年同期增长: 18%

请给出：
1. 销售额排名前3的产品
2. 至少3条关键洞察
3. 至少2条改进建议"#;

    match agent.extract::<SalesAnalysis>(prompt, schema).await {
        Ok(analysis) => {
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
        }
        Err(e) => {
            println!("  ✗ 分析失败: {}\n", e);
        }
    }

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

    let schema = ResponseFormat::json_schema(
        "data_quality",
        json!({
            "type": "object",
            "properties": {
                "total_rows": {"type": "integer"},
                "null_count": {"type": "integer"},
                "duplicate_count": {"type": "integer"},
                "quality_score": {"type": "number"},
                "issues": {
                    "type": "array",
                    "items": {"type": "string"}
                }
            }
        }),
    );

    let prompt = "请评估以下用户数据集的质量：

用户数据集 users.csv:
- 总行数: 10,000
- email字段空值: 150个
- phone字段格式错误: 80个
- 重复用户记录: 25个
- age字段异常值(>150): 5个

请计算质量分数(0-100)，并列出主要问题。";

    match agent.extract::<DataQualityReport>(prompt, schema).await {
        Ok(report) => {
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
        }
        Err(e) => {
            println!("  ✗ 检查失败: {}\n", e);
        }
    }

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

async fn demo_history_retrieval() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 5: 历史分析检索");
    println!("══════════════════════════━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let db_path = std::env::temp_dir().join("echo_agent_data_analyst.db");
    let store = Arc::new(SqliteStore::new(&db_path)?);
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

    let queries = ["促销活动", "customer growth", "销售增长"];

    for query in &queries {
        let results: Vec<_> = store.search(ns, query, 2).await?;
        println!("    查询: \"{}\"", query);
        for (i, item) in results.iter().enumerate() {
            let content = item.value["content"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(60)
                .collect::<String>();
            println!("      [{}] {}: {}...", i + 1, item.value["month"], content);
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

fn has_llm_config() -> bool {
    std::env::var("QWEN_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
        || std::env::var("DEEPSEEK_API_KEY").is_ok()
}
