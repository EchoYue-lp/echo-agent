//! demo07_skills.rs —— Skills（技能）系统演示
//!
//! 演示如何通过 Skill 为 Agent 快速装备能力组合，
//! 与逐个 add_tool 相比，Skill 额外提供了：
//! - 工具组的语义封装（"我懂文件操作" vs "我有 read/write 工具"）
//! - 自动注入 System Prompt 指引片段（告诉 LLM 何时怎么用这组工具）
//! - 技能元数据查询（list_skills / has_skill）
//!
//! # 运行
//! ```bash
//! cargo run --example demo07_skills
//! ```

use echo_agent::prelude::*;
use echo_agent::skills::Skill;
use echo_agent::skills::builtin::FileSystemSkill;
use echo_agent::tools::{Tool, ToolParameters, ToolResult};

// ── 自定义 Skill 示例：展示如何实现自己的 Skill ─────────────────────────────

/// 一个演示用的自定义 Skill，将字符串转换为大写/小写
struct TextProcessingSkill;

/// 转大写工具
struct ToUpperTool;
impl Tool for ToUpperTool {
    fn name(&self) -> &str {
        "to_upper"
    }
    fn description(&self) -> &str {
        "将文本转换为全大写"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "要转换的文本" }
            },
            "required": ["text"]
        })
    }
    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> futures::future::BoxFuture<'_, echo_agent::error::Result<ToolResult>> {
        Box::pin(async move {
            let text = parameters
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(ToolResult::success(text.to_uppercase()))
        })
    }
}

/// 转小写工具
struct ToLowerTool;
impl Tool for ToLowerTool {
    fn name(&self) -> &str {
        "to_lower"
    }
    fn description(&self) -> &str {
        "将文本转换为全小写"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "要转换的文本" }
            },
            "required": ["text"]
        })
    }
    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> futures::future::BoxFuture<'_, echo_agent::error::Result<ToolResult>> {
        Box::pin(async move {
            let text = parameters
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(ToolResult::success(text.to_lowercase()))
        })
    }
}

/// 实现 Skill trait
impl Skill for TextProcessingSkill {
    fn name(&self) -> &str {
        "text_processing"
    }
    fn description(&self) -> &str {
        "文本大小写转换能力"
    }
    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![Box::new(ToUpperTool), Box::new(ToLowerTool)]
    }
    fn system_prompt_injection(&self) -> Option<String> {
        Some("\n\n## 文本处理能力\n你可以对文本进行大小写转换：\n- `to_upper(text)`：将文本转为全大写\n- `to_lower(text)`：将文本转为全小写".to_string())
    }
}

// ── 主程序 ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=info,demo07_skills=info".into()),
        )
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("          Echo Agent × Skills 系统演示");
    println!("═══════════════════════════════════════════════════════\n");

    // Part 1: 展示 Skill 基础元数据（不需要 LLM）
    demo_skill_metadata();

    // Part 2: 安装并查询 Skills（不需要 LLM）
    demo_skill_installation();

    // Part 3: 通过 Skill 驱动 Agent 执行真实任务（需要 LLM 配置）
    demo_agent_with_skills().await?;

    Ok(())
}

/// Part 1: 直接查看各 Skill 的元数据
fn demo_skill_metadata() {
    println!("{}", "─".repeat(55));
    println!("Part 1: 查看内置 Skill 元数据\n");

    let skills: Vec<Box<dyn Skill>> = vec![
        Box::new(FileSystemSkill::with_base_dir("/tmp")),
        Box::new(TextProcessingSkill),
    ];

    for skill in &skills {
        println!("  Skill: {}", skill.name());
        println!("    描述: {}", skill.description());
        let tools = skill.tools();
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        println!("    工具: {:?}", tool_names);
        println!(
            "    Prompt注入: {}",
            if skill.system_prompt_injection().is_some() {
                "✓ 有"
            } else {
                "✗ 无"
            }
        );
        println!();
    }
}

/// Part 2: 向 Agent 安装 Skills 并查询状态
fn demo_skill_installation() {
    println!("{}", "─".repeat(55));
    println!("Part 2: 安装 Skills 到 Agent，查询状态\n");

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("demo-agent")
        .system_prompt("你是一个多功能助手。")
        .enable_tools()
        .build()
        .unwrap();

    println!("安装前：");
    println!("  已安装 Skill 数量: {}", agent.skill_count());
    println!("  已注册工具: {:?}\n", agent.list_tools());

    agent.add_skill(Box::new(FileSystemSkill::with_base_dir("/tmp")));
    agent.add_skill(Box::new(TextProcessingSkill));

    println!("\n安装后：");
    println!("  已安装 Skill 数量: {}", agent.skill_count());
    println!("  已注册工具数量: {}", agent.list_tools().len());
    println!(
        "  has_skill('filesystem'): {}",
        agent.has_skill("filesystem")
    );
    println!(
        "  has_skill('nonexistent'): {}",
        agent.has_skill("nonexistent")
    );

    println!("\n  已安装的 Skills：");
    for info in agent.list_skills() {
        println!(
            "    • {} — {} [工具: {}]",
            info.name,
            info.description,
            info.tool_names.join(", ")
        );
    }
    println!();
}

/// Part 3: 真实 Agent 执行（需要 LLM）
async fn demo_agent_with_skills() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 3: Skills + ReAct Agent 执行真实任务\n");

    let system_prompt = "你是一个全能助手，请使用工具完成用户的请求，不要猜测，一定要调用工具。";

    // ── 场景 A: FileSystem Skill ───────────────────────────────────────────
    println!("场景 A: FileSystem Skill —— 文件读写操作\n");
    {
        let mut agent = ReactAgentBuilder::new()
            .model("qwen3-max")
            .name("file-agent")
            .system_prompt(system_prompt)
            .enable_tools()
            .build()?;
        agent.add_skill(Box::new(FileSystemSkill::with_base_dir("/tmp")));

        let task = "在 /tmp/skills_demo.txt 写入内容 'Hello from echo-agent Skills!'，然后读取它并确认内容正确";
        println!("任务: {}", task);
        match agent.execute(task).await {
            Ok(result) => println!("✓ 结果: {}\n", result),
            Err(e) => println!("✗ 失败: {}\n", e),
        }
    }

    // ── 场景 B: 多 Skill 组合 ─────────────────────────────────────────────
    println!("场景 B: 多 Skill 组合 —— 文本处理 + 文件写入\n");
    {
        let mut agent = ReactAgentBuilder::new()
            .model("qwen3-max")
            .name("multi-skill-agent")
            .system_prompt(system_prompt)
            .enable_tools()
            .build()?;
        agent.add_skills(vec![
            Box::new(FileSystemSkill::with_base_dir("/tmp")),
            Box::new(TextProcessingSkill),
        ]);

        let task = "把 'hello world' 转成大写，然后写入 /tmp/uppercase_test.txt";
        println!("任务: {}", task);
        match agent.execute(task).await {
            Ok(result) => println!("✓ 结果: {}\n", result),
            Err(e) => println!("✗ 失败: {}\n", e),
        }
    }

    Ok(())
}
