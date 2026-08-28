//! demo08_external_skills.rs — File-based Skills: Full Feature Demo
//!
//! Demonstrates the complete agentskills.io skill system:
//!
//! | Feature | Description |
//! |---------|------------|
//! | Progressive disclosure | 3-tier: catalog → activate → resources/scripts |
//! | Inline shell execution | `!`cmd`` and ` ```! cmd ``` ` in SKILL.md |
//! | Variable substitution | `${SKILL_DIR}`, `${SESSION_ID}`, `${ARGUMENTS}` |
//! | Hooks | PreToolUse/PostToolUse interception |
//! | Conditional activation | `paths` frontmatter for file-triggered skills |
//! | `allowed-tools` | Declare allowed tools per skill; runtime-enforced for built-in skill tools |
//!
//! # Run
//! ```bash
//! cargo run --example demo08_external_skills
//! ```

use echo_agent::prelude::*;
use echo_agent::skills::external::loader::{DiscoveryScope, SkillLoader};
use echo_agent::skills::external::resource_tool::ReadSkillResourceTool;
use echo_agent::skills::external::run_script_tool::RunSkillScriptTool;
use echo_agent::skills::external::types::SkillDescriptor;
use echo_agent::skills::registry::SkillRegistry;
use echo_agent::skills::registry::shared_registry;
use echo_agent::tools::Tool;
use std::collections::HashMap;
use std::sync::Arc;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "echo_agent=info,demo08=info".into()),
        )
        .init();

    println!("═══════════════════════════════════════════════════════════");
    println!("  Echo Agent × File-Based Skills (agentskills.io v2)");
    println!("═══════════════════════════════════════════════════════════\n");

    demo_1_discovery().await?;
    demo_2_catalog().await?;
    demo_3_activation().await?;
    demo_4_script_execution().await?;
    demo_5_agent_integration().await?;
    demo_6_new_features().await?;
    demo_7_xiaohongshu_image_generator().await?;

    println!("\n{}", "═".repeat(59));
    println!("All demos completed.");

    Ok(())
}

// ── Part 1: Discovery ────────────────────────────────────────────────────────

/// Tier 1: Parse SKILL.md frontmatter only (name + description).
/// This is the cheapest operation — no file bodies are read.
async fn demo_1_discovery() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(59));
    println!("Part 1: Skill Discovery (Tier 1 — frontmatter only)\n");

    let skills_dir = std::path::Path::new("examples/demo_skills");
    if !skills_dir.exists() {
        return Err(echo_agent::error::ReactError::Other(
            "demo08 验收失败：缺少 ./examples/demo_skills/ 目录".to_string(),
        ));
    }

    let mut loader = SkillLoader::new();
    let descriptors = loader.discover_from_dir(skills_dir).await?;
    if descriptors.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo08 验收失败：未发现任何外部技能".to_string(),
        ));
    }
    if !descriptors.iter().any(|d| d.name == "project-stats")
        || !descriptors
            .iter()
            .any(|d| d.name == "xiaohongshu-image-generator")
    {
        return Err(echo_agent::error::ReactError::Other(
            "demo08 验收失败：缺少关键技能 project-stats 或 xiaohongshu-image-generator"
                .to_string(),
        ));
    }

    println!("  Discovered {} skills:\n", descriptors.len());

    for desc in &descriptors {
        if !desc.validate_name().is_empty() {
            return Err(echo_agent::error::ReactError::Other(format!(
                "demo08 验收失败：技能 `{}` 名称校验未通过: {:?}",
                desc.name,
                desc.validate_name()
            )));
        }
        println!("  ● {}", desc.name);
        println!("    description: {}", desc.description);
        println!(
            "    name valid:  {}",
            if desc.validate_name().is_empty() {
                "✓"
            } else {
                "⚠ (see warnings)"
            }
        );
        if let Some(license) = &desc.license {
            println!("    license:     {}", license);
        }
        if let Some(shell) = &desc.shell {
            println!("    shell:       {}", shell);
        }
        if !desc.paths.is_empty() {
            println!("    paths:       {:?}", desc.paths);
        }
        if !desc.allowed_tools.is_empty() {
            println!("    allowed:     {:?}", desc.allowed_tools);
        }
        if desc.hooks.is_some() {
            println!("    hooks:       defined");
        }
        if !desc.metadata.is_empty() {
            let pairs: Vec<String> = desc
                .metadata
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect();
            println!("    metadata:    {}", pairs.join(", "));
        }
        println!();
    }

    Ok(())
}

// ── Part 2: Catalog ──────────────────────────────────────────────────────────

/// System prompt injection: only name + description per skill.
/// This is what the LLM "sees" at the start of every conversation.
async fn demo_2_catalog() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(59));
    println!("Part 2: Skill Catalog (injected into system prompt)\n");

    let mut loader = SkillLoader::new();
    let descriptors = loader.discover_from_dir("examples/demo_skills").await?;

    let mut registry = SkillRegistry::new();
    for desc in descriptors {
        registry.register_descriptor(desc);
    }

    let catalog = registry.catalog_prompt().ok_or_else(|| {
        echo_agent::error::ReactError::Other("demo08 验收失败：catalog_prompt 为空".to_string())
    })?;
    if !catalog.contains("activate_skill") || !catalog.contains("project-stats") {
        return Err(echo_agent::error::ReactError::Other(
            "demo08 验收失败：catalog 缺少关键技能说明或激活指引".to_string(),
        ));
    }
    println!(
        "  Catalog ({} skills, {} chars):\n",
        registry.descriptor_count(),
        catalog.chars().count()
    );
    for line in catalog.lines() {
        println!("  │ {}", line);
    }
    println!("\n  ↑ This is the ONLY text injected at startup.");
    println!("    Full instructions are loaded on-demand via activate_skill.");

    println!();
    Ok(())
}

// ── Part 3: Activation ───────────────────────────────────────────────────────

/// Tier 2: Load full SKILL.md body + resource listing.
/// This only happens when the LLM calls activate_skill.
async fn demo_3_activation() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(59));
    println!("Part 3: Skill Activation (Tier 2 — full instructions)\n");

    let mut loader = SkillLoader::new();
    let descriptors = loader.discover_from_dir("examples/demo_skills").await?;

    if descriptors.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo08 验收失败：没有可激活的技能".to_string(),
        ));
    }

    let mut registry = SkillRegistry::new();
    for desc in descriptors {
        registry.register_descriptor(desc);
    }

    // Activate "project-stats" (has scripts), or fall back to the first available
    let names = registry.available_names();
    let skill_name = if registry.get_descriptor("project-stats").is_some() {
        "project-stats".to_string()
    } else {
        names
            .first()
            .cloned()
            .unwrap_or_else(|| "code-review".to_string())
    };

    println!("  Activating skill '{}'...\n", skill_name);
    assert!(!registry.is_activated(&skill_name));

    let content = registry.activate(&skill_name).await?;
    if content.instructions.trim().is_empty() || content.resources.is_empty() {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo08 验收失败：技能 `{skill_name}` 激活后 instructions/resources 不完整"
        )));
    }

    println!(
        "  Instructions ({} chars):",
        content.instructions.chars().count()
    );
    for line in content.instructions.lines().take(12) {
        println!("  │ {}", line);
    }
    let total_lines = content.instructions.lines().count();
    if total_lines > 12 {
        println!("  │ ... ({} more lines)", total_lines - 12);
    }

    println!("\n  Bundled resources ({}):", content.resources.len());
    for res in &content.resources {
        let icon = match res.kind {
            echo_agent::skills::external::SkillResourceKind::Script => "⚡",
            echo_agent::skills::external::SkillResourceKind::Reference => "📄",
            _ => "📦",
        };
        println!("    {} [{}] {}", icon, res.kind, res.relative_path);
    }

    assert!(registry.is_activated(&skill_name));
    let dedup = registry.mark_activated(&skill_name);
    if dedup {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo08 验收失败：技能 `{skill_name}` 重复激活去重失效"
        )));
    }
    println!("\n  Dedup: activate again → already activated ✓ (skipped, no wasted tokens)");

    println!();
    Ok(())
}

// ── Part 4: Script Execution ─────────────────────────────────────────────────

/// Tier 3b: Execute scripts from the skill's scripts/ directory.
/// Validates the actual `run_skill_script` tool path instead of bypassing it.
async fn demo_4_script_execution() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(59));
    println!("Part 4: Script Execution (Tier 3b — run_skill_script)\n");

    let skill_dir = std::path::Path::new("skills/project-stats");
    let scripts_dir = skill_dir.join("scripts");
    if !scripts_dir.exists() {
        return Err(echo_agent::error::ReactError::Other(
            "demo08 验收失败：缺少 project-stats/scripts".to_string(),
        ));
    }

    let project_dir = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_string());

    let mut loader = SkillLoader::new();
    let descriptors = loader.discover_from_dir("examples/demo_skills").await?;
    let mut registry = SkillRegistry::new();
    for desc in descriptors {
        registry.register_descriptor(desc);
    }
    if registry.get_descriptor("project-stats").is_none() {
        return Err(echo_agent::error::ReactError::Other(
            "demo08 验收失败：未发现 project-stats 技能".to_string(),
        ));
    }
    registry.activate("project-stats").await?;
    let shared = shared_registry(registry);
    let tool = RunSkillScriptTool::new(shared)
        .with_sandbox_manager(Arc::new(echo_agent::sandbox::SandboxManager::local_only()));

    // 4a: Python script (count lines)
    println!("  4a. Python: count_lines.py");
    println!("  ─────────────────────────");
    {
        let json = run_skill_script_json(
            &tool,
            "project-stats",
            "scripts/count_lines.py",
            &project_dir,
        )
        .await?;
        let summary = required_json_field(&json, "summary", "count_lines.py")?;
        let total_files = required_json_u64(summary, "total_files", "count_lines.py summary")?;
        if total_files == 0 {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：count_lines.py 未返回有效文件统计".to_string(),
            ));
        }
        let total_lines = required_json_field(summary, "total_lines", "count_lines.py summary")?;
        let code_lines = required_json_field(summary, "code_lines", "count_lines.py summary")?;
        let languages_detected =
            required_json_field(summary, "languages_detected", "count_lines.py summary")?;
        println!(
            "    Files: {}, Lines: {}, Code: {}, Languages: {}",
            total_files, total_lines, code_lines, languages_detected
        );
        if let Some(langs) = json.get("languages").and_then(serde_json::Value::as_array) {
            println!("    Top languages:");
            for lang in langs.iter().take(5) {
                let percentage = required_json_f64(lang, "percentage", "count_lines.py language")?;
                let language = required_json_str(lang, "language", "count_lines.py language")?;
                let files = required_json_field(lang, "files", "count_lines.py language")?;
                let code_lines =
                    required_json_field(lang, "code_lines", "count_lines.py language")?;
                println!(
                    "      {:>5.1}%  {} ({} files, {} lines)",
                    percentage, language, files, code_lines
                );
            }
        }
    }

    println!();

    // 4b: Shell script (find TODOs)
    println!("  4b. Bash: find_todos.sh");
    println!("  ───────────────────────");
    {
        let json = run_skill_script_json(
            &tool,
            "project-stats",
            "scripts/find_todos.sh",
            &project_dir,
        )
        .await?;
        let summary = required_json_field(&json, "summary", "find_todos.sh")?;
        let total = required_json_u64(summary, "total", "find_todos.sh summary")?;
        let todo = required_json_field(summary, "TODO", "find_todos.sh summary")?;
        let fixme = required_json_field(summary, "FIXME", "find_todos.sh summary")?;
        let hack = required_json_field(summary, "HACK", "find_todos.sh summary")?;
        let xxx = required_json_field(summary, "XXX", "find_todos.sh summary")?;
        println!(
            "    Total markers: {} (TODO={}, FIXME={}, HACK={}, XXX={})",
            total, todo, fixme, hack, xxx
        );
    }

    println!();

    // 4c: TypeScript script (dependency summary)
    println!("  4c. TypeScript: dep_summary.ts");
    println!("  ──────────────────────────────");
    {
        let json = run_skill_script_json(
            &tool,
            "project-stats",
            "scripts/dep_summary.ts",
            &project_dir,
        )
        .await?;
        let total = required_json_u64(&json, "total_dependencies", "dep_summary.ts")?;
        if total == 0 {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：dep_summary.ts 未返回依赖统计".to_string(),
            ));
        }
        println!("    Total dependencies: {}", total);
        if let Some(managers) = json.get("managers").and_then(serde_json::Value::as_array) {
            for mgr in managers {
                let manager = required_json_str(mgr, "manager", "dep_summary.ts manager")?;
                let file = required_json_str(mgr, "file", "dep_summary.ts manager")?;
                let direct_count =
                    required_json_field(mgr, "direct_count", "dep_summary.ts manager")?;
                let dev_count = required_json_field(mgr, "dev_count", "dep_summary.ts manager")?;
                println!(
                    "    {} ({}): {} direct, {} dev",
                    manager, file, direct_count, dev_count
                );
            }
        }
    }

    println!();
    Ok(())
}

async fn run_skill_script_json(
    tool: &RunSkillScriptTool,
    skill_name: &str,
    script: &str,
    arg: &str,
) -> echo_agent::error::Result<serde_json::Value> {
    let params = HashMap::from([
        ("skill_name".to_string(), serde_json::json!(skill_name)),
        ("script".to_string(), serde_json::json!(script)),
        ("args".to_string(), serde_json::json!(arg)),
    ]);

    let result = tool.execute(params).await?;
    if !result.success {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo08 验收失败：run_skill_script({}, {}) 执行失败: {}",
            skill_name,
            script,
            result.error.unwrap_or_else(|| "unknown error".to_string())
        )));
    }

    let json_body = extract_script_stdout(&result.output).ok_or_else(|| {
        echo_agent::error::ReactError::Other(format!(
            "demo08 验收失败：run_skill_script({}, {}) 返回格式不符合 <script_output> 协议",
            skill_name, script
        ))
    })?;

    serde_json::from_str::<serde_json::Value>(json_body).map_err(|e| {
        echo_agent::error::ReactError::Other(format!(
            "demo08 验收失败：run_skill_script({}, {}) 输出不是合法 JSON: {} | {}",
            skill_name,
            script,
            e,
            json_body.lines().take(5).collect::<Vec<_>>().join(" | ")
        ))
    })
}

async fn run_skill_script_stdout(
    tool: &RunSkillScriptTool,
    skill_name: &str,
    script: &str,
    args: &str,
) -> echo_agent::error::Result<String> {
    let params = HashMap::from([
        ("skill_name".to_string(), serde_json::json!(skill_name)),
        ("script".to_string(), serde_json::json!(script)),
        ("args".to_string(), serde_json::json!(args)),
    ]);

    let result = tool.execute(params).await?;
    let stdout = extract_script_stdout(&result.output).ok_or_else(|| {
        echo_agent::error::ReactError::Other(format!(
            "demo08 验收失败：run_skill_script({}, {}) 返回格式不符合 <script_output> 协议",
            skill_name, script
        ))
    })?;

    if !result.success {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo08 验收失败：run_skill_script({}, {}) 执行失败: {} | {}",
            skill_name,
            script,
            result.error.unwrap_or_else(|| "unknown error".to_string()),
            stdout.lines().take(5).collect::<Vec<_>>().join(" | ")
        )));
    }

    Ok(stdout.to_string())
}

fn extract_script_stdout(output: &str) -> Option<&str> {
    let (_, body) = output.split_once(">\n")?;
    let (body, _) = body.rsplit_once("</script_output>")?;
    let body = body.trim();
    let body = body
        .split_once("\n<stderr>")
        .map(|(stdout, _)| stdout)
        .unwrap_or(body);
    Some(body.trim())
}

fn required_json_field<'a>(
    value: &'a serde_json::Value,
    field: &str,
    context: &str,
) -> echo_agent::error::Result<&'a serde_json::Value> {
    value.get(field).ok_or_else(|| {
        echo_agent::error::ReactError::Other(format!("demo08 验收失败：{context} 缺少字段 {field}"))
    })
}

fn required_json_u64(
    value: &serde_json::Value,
    field: &str,
    context: &str,
) -> echo_agent::error::Result<u64> {
    required_json_field(value, field, context)?
        .as_u64()
        .ok_or_else(|| {
            echo_agent::error::ReactError::Other(format!(
                "demo08 验收失败：{context}.{field} 不是无符号整数"
            ))
        })
}

fn required_json_f64(
    value: &serde_json::Value,
    field: &str,
    context: &str,
) -> echo_agent::error::Result<f64> {
    required_json_field(value, field, context)?
        .as_f64()
        .ok_or_else(|| {
            echo_agent::error::ReactError::Other(format!(
                "demo08 验收失败：{context}.{field} 不是数值"
            ))
        })
}

fn required_json_str<'a>(
    value: &'a serde_json::Value,
    field: &str,
    context: &str,
) -> echo_agent::error::Result<&'a str> {
    required_json_field(value, field, context)?
        .as_str()
        .ok_or_else(|| {
            echo_agent::error::ReactError::Other(format!(
                "demo08 验收失败：{context}.{field} 不是字符串"
            ))
        })
}

// ── Part 5: Full Agent Integration ───────────────────────────────────────────

/// Full integration: agent discovers skills, injects catalog, registers 3 tools.
async fn demo_5_agent_integration() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(59));
    println!("Part 5: Agent + Skills (Full Progressive Disclosure)\n");

    let skills_dir = std::path::Path::new("examples/demo_skills");
    if !skills_dir.exists() {
        return Err(echo_agent::error::ReactError::Other(
            "demo08 验收失败：缺少 ./examples/demo_skills/ 目录".to_string(),
        ));
    }

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("skill-demo-agent")
        .system_prompt("你是一个多功能助手，能根据任务自动激活合适的技能。")
        .enable_tools()
        .build()?;

    let discovered = agent
        .discover_skills(&[DiscoveryScope::Custom(skills_dir.into())])
        .await?;
    if discovered.is_empty() || agent.skill_count() == 0 {
        return Err(echo_agent::error::ReactError::Other(
            "demo08 验收失败：Agent 未发现任何外部技能".to_string(),
        ));
    }

    println!("  Discovered skills: {:?}", discovered);
    println!("  Total skill count: {}", agent.skill_count());

    let tools = agent.list_tools();
    let tool_checks = [
        ("activate_skill", "Tier 2: load full instructions on demand"),
        (
            "read_skill_resource",
            "Tier 3a: read reference files on demand",
        ),
        (
            "run_skill_script",
            "Tier 3b: execute Python/Bash/TS scripts",
        ),
    ];
    println!("\n  Progressive disclosure tools:");
    for (name, purpose) in &tool_checks {
        let registered = tools.contains(&name.to_string());
        if !registered {
            return Err(echo_agent::error::ReactError::Other(format!(
                "demo08 验收失败：Agent 未自动注册技能工具 `{name}`"
            )));
        }
        println!(
            "    {:<24} {} {}",
            name,
            if registered { "✓" } else { "✗" },
            purpose
        );
    }

    println!("\n  Context protection:");
    println!("    Activated skill instructions are protected from");
    println!("    compression — they survive context compaction.");

    Ok(())
}

// ── Part 6: New Features Demo ────────────────────────────────────────────────

/// Demonstrates v2 features: inline execution, variable substitution, hooks,
/// conditional activation, and allowed-tools.
async fn demo_6_new_features() -> echo_agent::error::Result<()> {
    use echo_agent::skills::external::prompt_exec::{
        PromptContext, SkillSource, process_skill_content,
    };
    use echo_agent::skills::hooks::{HookAction, HookRegistry, HookRule, HooksDefinition};

    println!("{}", "─".repeat(59));
    println!("Part 6: New Features (v2)\n");

    // 6a: Variable substitution
    println!("  6a. Variable Substitution");
    println!("  ─────────────────────────");
    {
        let ctx = PromptContext {
            skill_dir: "/home/user/skills/my-skill".into(),
            session_id: "session-abc12345".into(),
            arguments: vec!["src/".into(), "--verbose".into()],
            source: SkillSource::Local,
            ..Default::default()
        };

        let template = "Skill at ${SKILL_DIR}\nSession: ${SESSION_ID}\nTarget: ${1}, Flags: ${2}\nAll: ${ARGUMENTS}";
        let result = process_skill_content(template, &ctx).await;
        if !result.contains("/home/user/skills/my-skill")
            || !result.contains("session-abc12345")
            || !result.contains("src/")
            || !result.contains("--verbose")
        {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：变量替换结果不完整".to_string(),
            ));
        }
        for line in result.lines() {
            println!("    {}", line);
        }
    }
    println!();

    // 6b: Inline shell execution
    println!("  6b. Inline Shell Execution");
    println!("  ──────────────────────────");
    {
        let ctx = PromptContext {
            skill_dir: ".".into(),
            source: SkillSource::Local,
            ..Default::default()
        };

        let content = "Host OS: !`uname -s 2>/dev/null || echo unknown`\nRust version:\n```!\nrustc --version 2>/dev/null || echo 'not installed'\n```";
        let result = process_skill_content(content, &ctx).await;
        if result.contains("!`") || result.contains("```!") || !result.contains("Host OS:") {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：inline shell execution 未正确展开".to_string(),
            ));
        }
        for line in result.lines() {
            println!("    {}", line);
        }
    }
    println!();

    // 6c: MCP source safety
    println!("  6c. MCP Source Safety");
    println!("  ─────────────────────");
    {
        let ctx = PromptContext {
            source: SkillSource::Mcp,
            ..Default::default()
        };

        let content = "Safe text. Dangerous: !`rm -rf /` more text";
        let result = process_skill_content(content, &ctx).await;
        if !result.contains("!`rm -rf /`") {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：MCP 来源内容不应执行 inline command".to_string(),
            ));
        }
        println!("    Input:  {}", content);
        println!("    Output: {}", result);
        println!("    (MCP skills never execute inline commands)");
    }
    println!();

    // 6d: Hooks system
    println!("  6d. Hooks System");
    println!("  ────────────────");
    {
        let mut registry = HookRegistry::new();

        let def = HooksDefinition {
            rules: HashMap::from([
                (
                    HookEvent::PreToolUse,
                    vec![HookRule {
                        matcher: "Bash".into(),
                        hooks: vec![HookAction::Prompt {
                            prompt: "Verify command safety before execution".into(),
                        }],
                    }],
                ),
                (
                    HookEvent::PostToolUse,
                    vec![HookRule {
                        matcher: "*".into(),
                        hooks: vec![HookAction::Prompt {
                            prompt: "Check output for sensitive data".into(),
                        }],
                    }],
                ),
            ]),
        };

        registry.register("security-guard", "/tmp/security", def);

        let pre = registry
            .run_pre_tool_use("Bash", &serde_json::json!({"command": "git status"}), "")
            .await;
        if pre.messages.is_empty() || pre.block {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：PreToolUse hook 未按预期触发".to_string(),
            ));
        }
        println!("    PreToolUse(Bash) → messages: {:?}", pre.messages);
        println!("    PreToolUse(Bash) → blocked:  {}", pre.block);

        let pre_read = registry
            .run_pre_tool_use("Read", &serde_json::json!({"path": "/etc/passwd"}), "")
            .await;
        if !pre_read.messages.is_empty() {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：Hook matcher 错误命中了 Read".to_string(),
            ));
        }
        println!(
            "    PreToolUse(Read) → messages: {:?} (no match)",
            pre_read.messages
        );

        let post = registry
            .run_post_tool_use("Read", &serde_json::json!({}), "file content here", "")
            .await;
        if post.messages.is_empty() {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：PostToolUse wildcard hook 未触发".to_string(),
            ));
        }
        println!(
            "    PostToolUse(Read) → messages: {:?} (wildcard match)",
            post.messages
        );
    }
    println!();

    // 6e: Conditional activation & allowed-tools
    println!("  6e. Conditional Activation & Allowed Tools");
    println!("  ──────────────────────────────────────────");
    {
        let mut loader = SkillLoader::new();
        let descriptors = loader.discover_from_dir("examples/demo_skills").await?;
        let constrained_count = descriptors
            .iter()
            .filter(|desc| !desc.paths.is_empty() || !desc.allowed_tools.is_empty())
            .count();
        if constrained_count == 0 {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：未发现带 paths/allowed-tools 约束的技能".to_string(),
            ));
        }

        for desc in &descriptors {
            if !desc.paths.is_empty() || !desc.allowed_tools.is_empty() {
                println!("    Skill '{}' has constraints:", desc.name);
                if !desc.paths.is_empty() {
                    println!("      Activates for: {:?}", desc.paths);
                }
                if !desc.allowed_tools.is_empty() {
                    println!("      Allowed tools: {:?}", desc.allowed_tools);
                }
            }
        }

        let mut registry = SkillRegistry::new();
        for desc in descriptors {
            registry.register_descriptor(desc);
        }
        let shared = shared_registry(registry);
        let tool = echo_agent::skills::external::activate_tool::ActivateSkillTool::new(
            shared,
            vec!["python-linter".to_string()],
        );

        let denied = tool
            .execute(HashMap::from([(
                "name".to_string(),
                serde_json::json!("python-linter"),
            )]))
            .await?;
        if denied.success
            || !denied
                .error
                .as_deref()
                .unwrap_or("")
                .contains("context_path")
        {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：conditional activation 未要求 context_path".to_string(),
            ));
        }
        println!("      Missing context_path → blocked ✓");

        let allowed = tool
            .execute(HashMap::from([
                ("name".to_string(), serde_json::json!("python-linter")),
                ("context_path".to_string(), serde_json::json!("demo.py")),
            ]))
            .await?;
        if !allowed.success {
            return Err(echo_agent::error::ReactError::Other(format!(
                "demo08 验收失败：conditional activation 匹配路径后仍未放行: {}",
                allowed.error.unwrap_or_default()
            )));
        }
        println!("      Matching context_path=demo.py → activated ✓");

        let temp_root =
            std::env::temp_dir().join(format!("echo-demo08-locked-skill-{}", std::process::id()));
        let locked_dir = temp_root.join("locked-skill");
        tokio::fs::create_dir_all(locked_dir.join("references")).await?;
        tokio::fs::create_dir_all(locked_dir.join("scripts")).await?;
        tokio::fs::write(locked_dir.join("SKILL.md"), "Locked skill body").await?;
        tokio::fs::write(locked_dir.join("references/guide.md"), "hello").await?;
        tokio::fs::write(locked_dir.join("scripts/test.py"), "print('hi')\n").await?;

        let mut locked_registry = SkillRegistry::new();
        locked_registry.register_descriptor(SkillDescriptor {
            source: None,
            name: "locked-skill".into(),
            description: "Demo skill with a narrow allowed-tools whitelist".into(),
            location: locked_dir.join("SKILL.md"),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec!["read_skill_resource".into()],
            shell: None,
            paths: vec![],
            triggers: vec![],
            hooks: None,
            depends_on: vec![],
            sandbox: None,
        });
        locked_registry.mark_activated("locked-skill");
        let locked_shared = shared_registry(locked_registry);

        let read_tool = ReadSkillResourceTool::new(locked_shared.clone());
        let read_result = read_tool
            .execute(HashMap::from([
                ("skill_name".to_string(), serde_json::json!("locked-skill")),
                ("path".to_string(), serde_json::json!("references/guide.md")),
            ]))
            .await?;
        if !read_result.success {
            return Err(echo_agent::error::ReactError::Other(format!(
                "demo08 验收失败：allowed-tools 已允许 read_skill_resource，但运行时仍拒绝: {}",
                read_result.error.unwrap_or_default()
            )));
        }
        println!("      allowed-tools permits read_skill_resource → allowed ✓");

        let run_tool = RunSkillScriptTool::new(locked_shared);
        let run_result = run_tool
            .execute(HashMap::from([
                ("skill_name".to_string(), serde_json::json!("locked-skill")),
                ("script".to_string(), serde_json::json!("scripts/test.py")),
            ]))
            .await?;
        if run_result.success
            || !run_result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("does not permit tool 'run_skill_script'")
        {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：allowed-tools 未真正拦截 run_skill_script".to_string(),
            ));
        }
        println!("      allowed-tools blocks run_skill_script → denied ✓");
        let _ = tokio::fs::remove_dir_all(temp_root).await;
    }
    println!();

    // 6f: Activation with inline execution
    println!("  6f. Activation with Inline Execution");
    println!("  ────────────────────────────────────");
    {
        let mut loader = SkillLoader::new();
        let descriptors = loader.discover_from_dir("examples/demo_skills").await?;

        let mut registry = SkillRegistry::new();
        for desc in descriptors {
            registry.register_descriptor(desc);
        }

        if registry.get_descriptor("project-stats").is_none() {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：缺少 project-stats 技能".to_string(),
            ));
        }
        let content = registry.activate("project-stats").await?;
        if content.instructions.contains("${SKILL_DIR}") || content.instructions.contains("!`") {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：激活后的 instructions 仍保留未展开占位符/命令".to_string(),
            ));
        }
        println!("    After activation, instructions contain resolved values:");
        for line in content.instructions.lines().take(8) {
            println!("    │ {}", line);
        }
        if content.instructions.lines().count() > 8 {
            println!("    │ ...");
        }
        println!("    (${{SKILL_DIR}} and !`uname -s` have been replaced with actual values)");
    }

    println!();
    Ok(())
}

// ── Part 7: XiaoHongShu Image Generator ──────────────────────────────────────

/// Demonstrates activating and executing the xiaohongshu-image-generator skill.
/// Shows: skill discovery → activation → script execution → output verification.
async fn demo_7_xiaohongshu_image_generator() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(59));
    println!("Part 7: XiaoHongShu Image Generator Skill\n");

    let skill_dir = std::path::Path::new("skills/redbook-image-generator-1.0.0");
    if !skill_dir.exists() {
        return Err(echo_agent::error::ReactError::Other(
            "demo08 验收失败：缺少 redbook-image-generator-1.0.0 技能目录".to_string(),
        ));
    }

    // 7a: Discover & activate
    println!("  7a. Skill Discovery & Activation");
    println!("  ──────────────────────────────────");
    {
        let mut loader = SkillLoader::new();
        let descriptors = loader.discover_from_dir("examples/demo_skills").await?;

        let xhs = descriptors
            .iter()
            .find(|d| d.name == "xiaohongshu-image-generator");

        let desc = xhs.ok_or_else(|| {
            echo_agent::error::ReactError::Other(
                "demo08 验收失败：skills/ 中未发现 xiaohongshu-image-generator".to_string(),
            )
        })?;
        println!("    Name:        {}", desc.name);
        println!("    Description: {}", desc.description);

        let mut registry = SkillRegistry::new();
        for desc in descriptors {
            registry.register_descriptor(desc);
        }

        let content = registry.activate("xiaohongshu-image-generator").await?;
        if content.instructions.trim().is_empty()
            || !content
                .resources
                .iter()
                .any(|res| res.relative_path.contains("generate_cover.py"))
        {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：小红书封面技能激活结果不完整".to_string(),
            ));
        }
        println!(
            "\n    Instructions ({} chars):",
            content.instructions.chars().count()
        );
        for line in content.instructions.lines() {
            println!("    │ {}", line);
        }

        println!("\n    Bundled resources ({}):", content.resources.len());
        for res in &content.resources {
            let icon = match res.kind {
                echo_agent::skills::external::SkillResourceKind::Script => "⚡",
                echo_agent::skills::external::SkillResourceKind::Reference => "📄",
                _ => "📦",
            };
            println!("      {} [{}] {}", icon, res.kind, res.relative_path);
        }
    }
    println!();

    // 7b: Generate cover image
    println!("  7b. Generate Cover Image");
    println!("  ─────────────────────────");
    {
        let mut loader = SkillLoader::new();
        let descriptors = loader.discover_from_dir("examples/demo_skills").await?;
        let mut registry = SkillRegistry::new();
        for desc in descriptors {
            registry.register_descriptor(desc);
        }
        registry.activate("xiaohongshu-image-generator").await?;
        let shared = shared_registry(registry);
        let tool = RunSkillScriptTool::new(shared)
            .with_sandbox_manager(Arc::new(echo_agent::sandbox::SandboxManager::local_only()));

        let title = "今日分享";
        let subtitle = "打卡第100天";
        let content = "Rust + AI = 🚀\\n技能系统真好用\\n继续加油！";
        let output_path = std::env::temp_dir().join("xhs_demo_cover.jpg");
        let args = format!(
            "\"{}\" \"{}\" \"{}\" \"{}\"",
            title,
            subtitle,
            content,
            output_path.display()
        );

        println!("    Title:    {}", title);
        println!("    Subtitle: {}", subtitle);
        println!("    Content:  {}", content.replace("\\n", " / "));

        let output = run_skill_script_stdout(
            &tool,
            "xiaohongshu-image-generator",
            "scripts/generate_cover.py",
            &args,
        )
        .await?;
        println!("\n    {}", output.trim());

        if !output_path.exists() {
            let lower = output.to_ascii_lowercase();
            if lower.contains("no module named") || lower.contains("cannot import name") {
                return Err(echo_agent::error::ReactError::Other(
                    "demo08 验收失败：缺少 Pillow/PIL 运行时依赖，无法验证封面图生成".to_string(),
                ));
            }
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：generate_cover.py 未生成输出图片".to_string(),
            ));
        }

        let meta = std::fs::metadata(&output_path)?;
        if meta.len() == 0 {
            let _ = std::fs::remove_file(&output_path);
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：封面图文件大小为 0".to_string(),
            ));
        }
        println!(
            "    Image saved: {} ({} bytes)",
            output_path.display(),
            meta.len()
        );
        let _ = std::fs::remove_file(&output_path);
    }
    println!();

    // 7c: Agent integration demo
    println!("  7c. Agent Integration");
    println!("  ──────────────────────");
    {
        let mut agent = ReactAgentBuilder::new()
            .model("qwen3-max")
            .name("xhs-skill-agent")
            .system_prompt("你是一个会使用技能的助手。")
            .enable_tools()
            .build()?;
        let discovered = agent
            .discover_skills(&[DiscoveryScope::Custom(
                std::path::Path::new("skills").into(),
            )])
            .await?;
        if !discovered
            .iter()
            .any(|name| name == "xiaohongshu-image-generator")
        {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：Agent 集成后未发现 xiaohongshu-image-generator".to_string(),
            ));
        }
        let tools = agent.list_tools();
        if !tools.contains(&"activate_skill".to_string()) {
            return Err(echo_agent::error::ReactError::Other(
                "demo08 验收失败：Agent 集成后缺少 activate_skill 工具".to_string(),
            ));
        }
        println!("    Agent discovered skills: {:?}", discovered);
        println!("    Agent registered tools: {:?}", tools);
    }

    println!();
    Ok(())
}
