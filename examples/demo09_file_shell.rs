//! demo09_file_shell.rs —— 文件系统 + Shell 综合演示

use echo_agent::agent::Agent;
use echo_agent::prelude::*;
use echo_agent::skills::Skill;
use echo_agent::skills::builtin::{FileSystemSkill, ShellSkill};
use echo_agent::tools::shell::{CommandSafety, ShellTool};

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=warn,demo09_file_shell=info".into()),
        )
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("      Echo Agent × 文件系统 + Shell 综合演示");
    println!("═══════════════════════════════════════════════════════\n");

    // Part 1: 静态能力展示
    demo_skill_overview();

    // Part 2: Shell 安全策略验证
    demo_shell_safety();

    // Part 3: Agent 真实执行
    demo_agent_tasks().await?;

    Ok(())
}

// ── Part 1: Skill 元数据展示 ─────────────────────────────────────────────────

fn demo_skill_overview() {
    println!("{}", "─".repeat(55));
    println!("Part 1: Skill 能力概览\n");

    let skills: Vec<(&str, Box<dyn Skill>)> = vec![
        (
            "FileSystemSkill（限制在 /tmp）",
            Box::new(FileSystemSkill::with_base_dir("/tmp")),
        ),
        ("ShellSkill（严格模式）", Box::new(ShellSkill::new())),
        ("ShellSkill（宽松模式）", Box::new(ShellSkill::permissive())),
    ];

    for (label, skill) in &skills {
        let tools = skill.tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        println!("  [{label}]");
        println!("    名称: {}", skill.name());
        println!("    描述: {}", skill.description());
        println!("    工具: {:?}", names);
        println!();
    }
}

// ── Part 2: Shell 安全策略验证 ───────────────────────────────────────────────

fn demo_shell_safety() {
    println!("{}", "─".repeat(55));
    println!("Part 2: Shell 三级安全策略验证\n");

    let tool = ShellTool::new();
    let cases: &[(&str, &str)] = &[
        ("ls -la /tmp", "文件查看"),
        ("git status", "git 只读子命令"),
        ("rm -rf /tmp/test", "文件删除"),
        ("sudo rm -rf /", "极危 - 需要 sudo"),
    ];

    for (cmd, desc) in cases {
        let safety = tool.check_command_safety(cmd);
        let (icon, label) = match &safety {
            CommandSafety::Safe => ("✅", "Safe"),
            CommandSafety::RequiresApproval(_) => ("⚠️ ", "NeedApproval"),
            CommandSafety::Dangerous(_) => ("🚫", "Dangerous"),
        };
        println!("  {icon} [{label}] {desc}: `{cmd}`");
    }
    println!();
}

// ── Part 3: Agent 真实任务执行 ───────────────────────────────────────────────

async fn demo_agent_tasks() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 3: Agent 执行真实任务\n");

    if !has_llm_config() {
        println!("跳过 Part 3：未检测到 LLM API 密钥。\n");
        return Ok(());
    }

    let work_dir = "/tmp/echo_agent_demo";

    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("file-agent")
        .system_prompt(format!(
            "你是一个文件管理助手，所有文件操作都在 `{work_dir}` 目录下进行。"
        ))
        .enable_tools()
        .max_iterations(15)
        .build()?;

    agent.add_skill(Box::new(FileSystemSkill::with_base_dir(work_dir)));

    let task = format!(
        "在 {work_dir}/notes.md 写入内容 '# 项目笔记\n- 完成了文件工具的实现'，然后读取确认"
    );
    println!("任务: {task}\n");

    match agent.execute(&task).await {
        Ok(result) => println!("✓ 结果:\n{result}\n"),
        Err(e) => println!("✗ 失败: {e}\n"),
    }

    Ok(())
}

fn has_llm_config() -> bool {
    std::env::var("QWEN_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
        || std::env::var("DEEPSEEK_API_KEY").is_ok()
}
