//! demo07_skills.rs —— Skills（技能）系统演示
//!
//! 演示如何通过 Skill 为 Agent 快速装备能力组合，
//! 与逐个 add_tool 相比，Skill 额外提供了：
//! - 工具组的语义封装（"我懂数学" vs "我有 add/subtract 工具"）
//! - 自动注入 System Prompt 指引片段（告诉 LLM 何时怎么用这组工具）
//! - 技能元数据查询（list_skills / has_skill）
//!
//! # Skills 架构
//!
//! ```text
//! agent.add_skill(Box::new(CalculatorSkill))
//!          │
//!          ├── 注册工具: add / subtract / multiply / divide
//!          └── 注入提示: "使用 add/subtract/... 做精确计算"
//!
//! agent.add_skill(Box::new(FileSystemSkill::with_base_dir("/tmp")))
//!          │
//!          ├── 注册工具: read_file / write_file / append_file / list_dir
//!          └── 注入提示: "操作限制在 /tmp 目录下..."
//! ```
//!
//! # 运行
//! ```bash
//! cargo run --example demo07_skills
//! ```

use echo_agent::agent::Agent;
use echo_agent::agent::react_agent::{AgentConfig, ReactAgent};
use echo_agent::skills::Skill;
use echo_agent::skills::builtin::{CalculatorSkill, FileSystemSkill, WeatherSkill};
use echo_agent::tools::{Tool, ToolParameters, ToolResult};

// ── 自定义 Skill 示例：展示如何实现自己的 Skill ─────────────────────────────

/// 一个演示用的自定义 Skill，将字符串转换为大写/小写
struct TextProcessingSkill;

/// 转大写工具
struct ToUpperTool;
#[async_trait::async_trait]
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
    async fn execute(&self, parameters: ToolParameters) -> echo_agent::error::Result<ToolResult> {
        let text = parameters
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(ToolResult::success(text.to_uppercase()))
    }
}

/// 转小写工具
struct ToLowerTool;
#[async_trait::async_trait]
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
    async fn execute(&self, parameters: ToolParameters) -> echo_agent::error::Result<ToolResult> {
        let text = parameters
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(ToolResult::success(text.to_lowercase()))
    }
}

/// 实现 Skill trait —— 这就是自定义 Skill 的全部工作
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
        Some(
            "\n\n## 文本处理能力（TextProcessing Skill）\n\
             你可以对文本进行大小写转换：\n\
             - `to_upper(text)`：将文本转为全大写\n\
             - `to_lower(text)`：将文本转为全小写"
                .to_string(),
        )
    }
}

// ── 主程序 ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
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

    // ── Part 1: 展示 Skill 基础元数据（不需要 LLM）────────────────────────
    demo_skill_metadata();

    // ── Part 2: 安装并查询 Skills（不需要 LLM）──────────────────────────────
    demo_skill_installation();

    // ── Part 3: 通过 Skill 驱动 Agent 执行真实任务（需要 LLM 配置）──────────
    demo_agent_with_skills().await?;

    Ok(())
}

/// Part 1: 直接查看各 Skill 的元数据
fn demo_skill_metadata() {
    println!("{}", "─".repeat(55));
    println!("Part 1: 查看内置 Skill 元数据\n");

    let skills: Vec<Box<dyn Skill>> = vec![
        Box::new(CalculatorSkill),
        Box::new(FileSystemSkill::with_base_dir("/tmp")),
        Box::new(WeatherSkill),
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
        if let Some(injection) = skill.system_prompt_injection() {
            // 只显示第一行
            let first_line = injection.trim().lines().next().unwrap_or("");
            println!("    注入预览: \"{}...\"", first_line);
        }
        println!();
    }
}

/// Part 2: 向 Agent 安装 Skills 并查询状态
fn demo_skill_installation() {
    println!("{}", "─".repeat(55));
    println!("Part 2: 安装 Skills 到 Agent，查询状态\n");

    let config =
        AgentConfig::new("qwen3-max", "demo-agent", "你是一个多功能助手。").enable_tool(true);

    let mut agent = ReactAgent::new(config);

    println!("安装前：");
    println!("  已安装 Skill 数量: {}", agent.skill_count());
    println!("  已注册工具: {:?}\n", agent.list_tools());

    // 安装内置 Skills
    agent.add_skill(Box::new(CalculatorSkill));
    agent.add_skill(Box::new(FileSystemSkill::with_base_dir("/tmp")));
    agent.add_skill(Box::new(WeatherSkill));

    // 安装自定义 Skill
    agent.add_skill(Box::new(TextProcessingSkill));

    // 重复安装同名 Skill 会被自动跳过
    agent.add_skill(Box::new(CalculatorSkill));

    println!("\n安装后：");
    println!("  已安装 Skill 数量: {}", agent.skill_count());
    println!("  已注册工具数量: {}", agent.list_tools().len());
    println!(
        "  has_skill('calculator'): {}",
        agent.has_skill("calculator")
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

    println!("\n  已注册工具: {:?}", agent.list_tools());
    println!();
}

/// Part 3: 真实 Agent 执行（需要 LLM）
async fn demo_agent_with_skills() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 3: Skills + ReAct Agent 执行真实任务\n");

    if !has_llm_config() {
        println!("跳过 Part 3：未检测到 LLM API 密钥");
        println!("（设置 OPENAI_API_KEY / DEEPSEEK_API_KEY / QWEN_API_KEY 后可启用）");
        return Ok(());
    }

    let system_prompt = "你是一个全能助手，请使用工具完成用户的请求，不要猜测，一定要调用工具。";

    // ── 场景 A: Calculator Skill ───────────────────────────────────────────
    println!("场景 A: Calculator Skill —— 多步骤精确计算\n");
    {
        let config = AgentConfig::new("qwen3-max", "calc-agent", system_prompt)
            .enable_tool(true)
            .enable_task(false);
        let mut agent = ReactAgent::new(config);
        agent.add_skill(Box::new(CalculatorSkill));

        let task = "计算: (15 * 8 + 36 / 4) - (100 / 5 * 3)，分步给出每一步的结果";
        println!("任务: {}", task);

        match agent.execute(task).await {
            Ok(result) => println!("✓ 结果: {}\n", result),
            Err(e) => println!("✗ 失败: {}\n", e),
        }
    }

    // ── 场景 B: FileSystem Skill ───────────────────────────────────────────
    println!("场景 B: FileSystem Skill —— 文件读写操作\n");
    {
        let config = AgentConfig::new("qwen3-max", "file-agent", system_prompt)
            .enable_tool(true)
            .enable_task(false);
        let mut agent = ReactAgent::new(config);
        agent.add_skill(Box::new(FileSystemSkill::with_base_dir("/tmp")));

        let task = "在 /tmp/skills_demo.txt 写入内容 'Hello from echo-agent Skills!'，\
                    然后读取它并确认内容正确";
        println!("任务: {}", task);

        match agent.execute(task).await {
            Ok(result) => println!("✓ 结果: {}\n", result),
            Err(e) => println!("✗ 失败: {}\n", e),
        }
    }

    // ── 场景 C: 多 Skill 组合 ─────────────────────────────────────────────
    println!("场景 C: 多 Skill 组合 —— 计算结果写入文件\n");
    {
        let config = AgentConfig::new("qwen3-max", "multi-skill-agent", system_prompt)
            .enable_tool(true)
            .enable_task(false);
        let mut agent = ReactAgent::new(config);
        agent.add_skills(vec![
            Box::new(CalculatorSkill),
            Box::new(FileSystemSkill::with_base_dir("/tmp")),
        ]);

        let task = "计算 123 * 456 的结果，然后把算式和结果写入 /tmp/calc_result.txt";
        println!("任务: {}", task);

        match agent.execute(task).await {
            Ok(result) => println!("✓ 结果: {}\n", result),
            Err(e) => println!("✗ 失败: {}\n", e),
        }
    }

    Ok(())
}

fn has_llm_config() -> bool {
    std::env::var("OPENAI_API_KEY").is_ok()
        || std::env::var("DEEPSEEK_API_KEY").is_ok()
        || std::env::var("QWEN_API_KEY").is_ok()
}
