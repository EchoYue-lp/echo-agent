//! demo08_external_skills.rs —— 外部 Skill 文件系统加载演示
//!
//! 演示如何从 `skills/` 目录自动扫描并加载基于 SKILL.md 定义的外部技能。

use echo_agent::prelude::*;
use echo_agent::skills::external::SkillLoader;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=info,demo08_external_skills=info".into()),
        )
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("       Echo Agent × 外部 Skill 文件系统加载演示");
    println!("═══════════════════════════════════════════════════════\n");

    // Part 1: 直接使用 SkillLoader API
    demo_skill_loader().await?;

    // Part 2: 解析单个 SKILL.md frontmatter
    demo_parse_frontmatter()?;

    // Part 3: Agent 自动加载目录中所有技能
    demo_agent_with_external_skills().await?;

    Ok(())
}

/// Part 1: 直接操作 SkillLoader
async fn demo_skill_loader() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 1: SkillLoader 扫描与懒加载演示\n");

    let mut loader = SkillLoader::new("./skills");
    let skills = loader.scan().await?;

    if skills.is_empty() {
        println!("未找到技能目录，请确认 ./skills/ 目录存在\n");
        return Ok(());
    }

    println!("扫描到 {} 个技能：", skills.len());
    for skill in &skills {
        let meta = &skill.meta;
        let resource_count = meta.resources.as_ref().map_or(0, |r| r.len());
        println!(
            "  • {} v{} — {} [{} 个资源]",
            meta.name,
            meta.version.as_deref().unwrap_or("?"),
            meta.description,
            resource_count
        );
    }

    println!("\n懒加载资源演示：");
    match loader.load_resource("code_review", "checklist").await {
        Ok(content) => {
            let preview: String = content.lines().take(3).collect::<Vec<_>>().join("\n");
            println!(
                "  已加载 code_review/checklist:\n  ---\n  {}\n  ---",
                preview
            );
        }
        Err(e) => println!("  加载失败: {}", e),
    }

    println!();
    Ok(())
}

/// Part 2: 手动解析 SKILL.md frontmatter
fn demo_parse_frontmatter() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 2: 手动解析 SKILL.md Frontmatter\n");

    let skill_md = r#"---
name: example_skill
version: "0.1.0"
description: "演示用技能"
tags: [demo, example]
instructions: |
  这是一个演示性技能，展示 frontmatter 格式。
resources:
  - name: guide
    path: guide.md
    description: "使用指南"
---

# Example Skill
"#;

    match SkillLoader::parse_frontmatter(skill_md) {
        Ok(meta) => {
            println!("解析成功！");
            println!("  name: {}", meta.name);
            println!("  version: {}", meta.version.as_deref().unwrap_or("-"));
            println!("  description: {}", meta.description);
            println!("\n生成的 System Prompt 注入块：");
            println!("{}", "·".repeat(40));
            println!("{}", meta.to_prompt_block());
            println!("{}", "·".repeat(40));
        }
        Err(e) => println!("解析失败: {}", e),
    }

    println!();
    Ok(())
}

/// Part 3: Agent 自动加载目录中所有技能
async fn demo_agent_with_external_skills() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 3: Agent 自动加载外部技能\n");

    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("multi-skill-agent")
        .system_prompt(
            "你是一个多功能专业助手，拥有以下专业技能。在执行任务时，请充分利用你的专业技能指引。",
        )
        .enable_tools()
        .build()?;

    println!("正在从 ./skills 目录加载外部技能...");
    let loaded = agent.load_skills_from_dir("./skills").await?;

    println!("\n已加载 {} 个外部技能: {:?}", loaded.len(), loaded);
    println!("已注册工具: {:?}", agent.list_tools());

    if !has_llm_config() {
        println!("\n跳过 LLM 执行部分：未检测到 API 密钥");
        return Ok(());
    }

    // 执行代码审查任务
    println!("\n--- 场景：代码审查 ---");
    let code_task = r#"
请帮我审查以下 Python 函数，找出安全和质量问题：

```python
def get_user(user_id):
    query = "SELECT * FROM users WHERE id = " + str(user_id)
    result = db.execute(query)
    return result
```
"#;

    match agent.execute(code_task).await {
        Ok(result) => println!("审查结果:\n{}", result),
        Err(e) => println!("执行失败: {}", e),
    }

    Ok(())
}

fn has_llm_config() -> bool {
    std::env::var("OPENAI_API_KEY").is_ok()
        || std::env::var("DEEPSEEK_API_KEY").is_ok()
        || std::env::var("QWEN_API_KEY").is_ok()
}
