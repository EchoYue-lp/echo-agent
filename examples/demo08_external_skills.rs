//! demo08_external_skills.rs —— 外部 Skill 文件系统加载演示
//!
//! 演示如何从 `skills/` 目录自动扫描并加载基于 SKILL.md 定义的外部技能。
//!
//! # 技能目录结构
//!
//! ```text
//! skills/
//! ├── code_review/
//! │   ├── SKILL.md           ← YAML frontmatter + 说明文档
//! │   ├── checklist.md       ← 按需加载的审查清单
//! │   └── style_guide.md     ← 按需加载的风格规范
//! ├── data_analyst/
//! │   ├── SKILL.md
//! │   ├── report_template.md
//! │   └── statistical_methods.md
//! └── web_researcher/
//!     ├── SKILL.md
//!     ├── research_template.md
//!     └── source_evaluation.md
//! ```
//!
//! # SKILL.md Frontmatter 格式
//!
//! ```yaml
//! ---
//! name: code_review          # 唯一标识
//! version: "1.0.0"
//! description: "代码审查技能"
//! tags: [code, review]
//! instructions: |            # 注入 system prompt 的指引（自动加载）
//!   ## 代码审查能力
//!   审查代码时先调用 load_skill_resource("code_review", "checklist")...
//! resources:                 # 懒加载资源（LLM 按需调用工具获取）
//!   - name: checklist
//!     path: checklist.md
//!     description: "完整审查清单"
//! ---
//! ```
//!
//! # 运行
//! ```bash
//! cargo run --example demo08_external_skills
//! ```

use echo_agent::agent::Agent;
use echo_agent::agent::react_agent::{AgentConfig, ReactAgent};
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

    // ── Part 1: 直接使用 SkillLoader API（不依赖 LLM）────────────────────────
    demo_skill_loader().await?;

    // ── Part 2: 解析单个 SKILL.md frontmatter ─────────────────────────────
    demo_parse_frontmatter()?;

    // ── Part 3: Agent 自动加载目录中所有技能（需要 LLM 配置）────────────────
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
        if let Some(tags) = &meta.tags {
            println!("    标签: {}", tags.join(", "));
        }
        if let Some(resources) = &meta.resources {
            for res in resources {
                let cached = if loader.is_cached(&meta.name, &res.name) {
                    "已预加载"
                } else {
                    "懒加载"
                };
                println!(
                    "    - {}: {} [{}]",
                    res.name,
                    res.description.as_deref().unwrap_or(""),
                    cached
                );
            }
        }
    }

    // 演示懒加载
    println!("\n懒加载资源演示：");
    match loader.load_resource("code_review", "checklist").await {
        Ok(content) => {
            let preview: String = content.lines().take(5).collect::<Vec<_>>().join("\n");
            println!(
                "  已加载 code_review/checklist ({} 字节):\n  ---\n  {}\n  ---",
                content.len(),
                preview
            );
            println!(
                "  再次加载（命中缓存）: {}",
                if loader.is_cached("code_review", "checklist") {
                    "✓"
                } else {
                    "✗"
                }
            );
        }
        Err(e) => println!("  加载失败: {}", e),
    }

    // 资源目录
    println!("\n完整资源目录：");
    for (skill_name, res_ref) in loader.resource_catalog() {
        println!(
            "  {}/{} — {}",
            skill_name,
            res_ref.name,
            res_ref.description.as_deref().unwrap_or("")
        );
    }

    println!();
    Ok(())
}

/// Part 2: 手动解析 SKILL.md frontmatter
fn demo_parse_frontmatter() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 2: 手动解析 SKILL.md Frontmatter\n");

    // 模拟一个 SKILL.md 内容
    let skill_md = r#"---
name: example_skill
version: "0.1.0"
description: "演示用技能"
author: "demo"
tags: [demo, example]
instructions: |
  ## 示例技能
  这是一个演示性技能，展示 frontmatter 格式。
resources:
  - name: guide
    path: guide.md
    description: "使用指南"
  - name: config
    path: config.json
    description: "配置文件"
    load_on_startup: true
---

# Example Skill

（此部分是 Markdown 正文，不会被自动加载到 system prompt）
"#;

    match SkillLoader::parse_frontmatter(skill_md) {
        Ok(meta) => {
            println!("解析成功！");
            println!("  name: {}", meta.name);
            println!("  version: {}", meta.version.as_deref().unwrap_or("-"));
            println!("  description: {}", meta.description);
            println!("  tags: {:?}", meta.tags.as_deref().unwrap_or(&[]));
            println!(
                "  instructions: {} 字符",
                meta.instructions.as_ref().map_or(0, |s| s.len())
            );
            println!(
                "  resources: {} 个",
                meta.resources.as_ref().map_or(0, |r| r.len())
            );
            println!("\n生成的 System Prompt 注入块：");
            println!("{}", "·".repeat(40));
            println!("{}", meta.to_prompt_block());
            println!("{}", "·".repeat(40));

            // 获取启动时预加载的资源
            let startup = meta.startup_resources();
            if !startup.is_empty() {
                println!(
                    "\n需要预加载的资源: {:?}",
                    startup.iter().map(|r| &r.name).collect::<Vec<_>>()
                );
            }
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

    let system_prompt = "你是一个多功能专业助手，拥有以下专业技能。\
                         在执行任务时，请充分利用你的专业技能指引。";

    let config = AgentConfig::new("qwen3-max", "multi-skill-agent", system_prompt)
        .enable_tool(true)
        .enable_task(false)
        .enable_human_in_loop(false);

    let mut agent = ReactAgent::new(config);

    println!("正在从 ./skills 目录加载外部技能...");
    let loaded = agent.load_skills_from_dir("./skills").await?;

    println!("\n已加载 {} 个外部技能: {:?}", loaded.len(), loaded);
    println!("已注册工具: {:?}", agent.list_tools());
    println!("\n已安装的所有技能：");
    for info in agent.list_skills() {
        println!("  • {} — {}", info.name, info.description);
    }

    if !has_llm_config() {
        println!("\n跳过 LLM 执行部分：未检测到 API 密钥");
        println!("（设置 OPENAI_API_KEY / DEEPSEEK_API_KEY / QWEN_API_KEY 后可启用）");
        println!("\n[当前 System Prompt 片段（前 500 字符）]");
        let prompt_preview = agent.system_prompt().chars().take(500).collect::<String>();
        println!("{}", prompt_preview);
        if agent.system_prompt().len() > 500 {
            println!("... [共 {} 字符]", agent.system_prompt().len());
        }
        return Ok(());
    }

    // ── 场景：代码审查（会触发懒加载 checklist 资源）─────────────────────
    println!("\n--- 场景：代码审查 ---");
    let code_task = r#"
请帮我审查以下 Python 函数，找出安全和质量问题：

```python
def get_user(user_id):
    query = "SELECT * FROM users WHERE id = " + str(user_id)
    result = db.execute(query)
    password = result['password']
    print(f"User login: {result['email']}, password: {password}")
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
