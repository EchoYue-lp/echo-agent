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
//! | `allowed-tools` | Restrict available tools per skill |
//!
//! # Run
//! ```bash
//! cargo run --example demo08_external_skills
//! ```

use echo_agent::prelude::*;
use echo_agent::skills::external::loader::{DiscoveryScope, SkillLoader};
use echo_agent::skills::registry::SkillRegistry;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

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

    let skills_dir = std::path::Path::new("skills");
    if !skills_dir.exists() {
        println!("  [skip] ./skills/ directory not found");
        return Ok(());
    }

    let mut loader = SkillLoader::new();
    let descriptors = loader.discover_from_dir(skills_dir).await?;

    println!("  Discovered {} skills:\n", descriptors.len());

    for desc in &descriptors {
        println!("  {} {}", "●", desc.name);
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
    let descriptors = loader.discover_from_dir("skills").await?;

    let mut registry = SkillRegistry::new();
    for desc in descriptors {
        registry.register_descriptor(desc);
    }

    match registry.catalog_prompt() {
        Some(catalog) => {
            println!(
                "  Catalog ({} skills, {} chars):\n",
                registry.descriptor_count(),
                catalog.len()
            );
            for line in catalog.lines() {
                println!("  │ {}", line);
            }
            println!("\n  ↑ This is the ONLY text injected at startup.");
            println!("    Full instructions are loaded on-demand via activate_skill.");
        }
        None => {
            println!("  No skills discovered, catalog is empty.");
        }
    }

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
    let descriptors = loader.discover_from_dir("skills").await?;

    if descriptors.is_empty() {
        println!("  [skip] No skills to activate");
        return Ok(());
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

    println!("  Instructions ({} chars):", content.instructions.len());
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
    println!(
        "\n  Dedup: activate again → {}",
        if registry.mark_activated(&skill_name) {
            "new (bug!)"
        } else {
            "already activated ✓ (skipped, no wasted tokens)"
        }
    );

    println!();
    Ok(())
}

// ── Part 4: Script Execution ─────────────────────────────────────────────────

/// Tier 3b: Execute scripts from the skill's scripts/ directory.
/// Shows Python, Bash, and TS script execution with automatic interpreter detection.
async fn demo_4_script_execution() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(59));
    println!("Part 4: Script Execution (Tier 3b — run_skill_script)\n");

    let scripts_dir = std::path::Path::new("skills/project-stats/scripts");
    if !scripts_dir.exists() {
        println!("  [skip] project-stats skill not found");
        return Ok(());
    }

    let project_dir = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_string());

    // 4a: Python script (count lines)
    println!("  4a. Python: count_lines.py");
    println!("  ─────────────────────────");
    {
        let output = tokio::process::Command::new("python3")
            .arg("skills/project-stats/scripts/count_lines.py")
            .arg(&project_dir)
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    let summary = &json["summary"];
                    println!(
                        "    Files: {}, Lines: {}, Code: {}, Languages: {}",
                        summary["total_files"],
                        summary["total_lines"],
                        summary["code_lines"],
                        summary["languages_detected"]
                    );
                    if let Some(langs) = json["languages"].as_array() {
                        println!("    Top languages:");
                        for lang in langs.iter().take(5) {
                            println!(
                                "      {:>5.1}%  {} ({} files, {} lines)",
                                lang["percentage"].as_f64().unwrap_or(0.0),
                                lang["language"].as_str().unwrap_or("?"),
                                lang["files"],
                                lang["code_lines"]
                            );
                        }
                    }
                } else {
                    println!("    {}", stdout.chars().take(200).collect::<String>());
                }
            }
            Ok(out) => {
                println!(
                    "    [error] exit code {:?}: {}",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            Err(e) => println!("    [skip] python3 not available: {}", e),
        }
    }

    println!();

    // 4b: Shell script (find TODOs)
    println!("  4b. Bash: find_todos.sh");
    println!("  ───────────────────────");
    {
        let output = tokio::process::Command::new("bash")
            .arg("skills/project-stats/scripts/find_todos.sh")
            .arg(&project_dir)
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    let summary = &json["summary"];
                    println!(
                        "    Total markers: {} (TODO={}, FIXME={}, HACK={}, XXX={})",
                        summary["total"],
                        summary["TODO"],
                        summary["FIXME"],
                        summary["HACK"],
                        summary["XXX"]
                    );
                } else {
                    for line in stdout.lines().take(5) {
                        println!("    {}", line);
                    }
                }
            }
            Ok(out) => {
                println!(
                    "    [error] exit code {:?}: {}",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stderr)
                        .chars()
                        .take(200)
                        .collect::<String>()
                );
            }
            Err(e) => println!("    [skip] bash not available: {}", e),
        }
    }

    println!();

    // 4c: TypeScript script (dependency summary)
    println!("  4c. TypeScript: dep_summary.ts");
    println!("  ──────────────────────────────");
    {
        // Detect best TS runtime (same logic as RunSkillScriptTool)
        let (runtime, args) = detect_ts_runtime_for_demo();
        println!("    Using runtime: {}", runtime);

        let mut cmd = tokio::process::Command::new(&runtime);
        for arg in &args {
            cmd.arg(arg);
        }
        cmd.arg("skills/project-stats/scripts/dep_summary.ts");
        cmd.arg(&project_dir);

        match tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output()).await {
            Ok(Ok(out)) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    let total = json["total_dependencies"].as_u64().unwrap_or(0);
                    println!("    Total dependencies: {}", total);
                    if let Some(managers) = json["managers"].as_array() {
                        for mgr in managers {
                            println!(
                                "    {} ({}): {} direct, {} dev",
                                mgr["manager"].as_str().unwrap_or("?"),
                                mgr["file"].as_str().unwrap_or("?"),
                                mgr["direct_count"],
                                mgr["dev_count"]
                            );
                        }
                    }
                } else {
                    for line in stdout.lines().take(5) {
                        println!("    {}", line);
                    }
                }
            }
            Ok(Ok(out)) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                println!(
                    "    [error] exit {:?}: {}",
                    out.status.code(),
                    stderr.chars().take(200).collect::<String>()
                );
            }
            Ok(Err(e)) => println!("    [skip] {} not available: {}", runtime, e),
            Err(_) => println!("    [timeout] script took too long"),
        }
    }

    println!();
    Ok(())
}

/// Detect the best TypeScript runtime for the demo.
fn detect_ts_runtime_for_demo() -> (String, Vec<String>) {
    for cmd in &["bun", "deno"] {
        if std::process::Command::new(cmd)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return if *cmd == "deno" {
                ("deno".into(), vec!["run".into(), "--allow-read".into()])
            } else {
                (cmd.to_string(), vec![])
            };
        }
    }
    ("npx".into(), vec!["tsx".into()])
}

// ── Part 5: Full Agent Integration ───────────────────────────────────────────

/// Full integration: agent discovers skills, injects catalog, registers 3 tools.
async fn demo_5_agent_integration() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(59));
    println!("Part 5: Agent + Skills (Full Progressive Disclosure)\n");

    let skills_dir = std::path::Path::new("skills");
    if !skills_dir.exists() {
        println!("  [skip] ./skills/ directory not found");
        return Ok(());
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
        let registered = tools.iter().any(|t| *t == *name);
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
            pre_tool_use: vec![HookRule {
                matcher: "Bash".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "Verify command safety before execution".into(),
                }],
            }],
            post_tool_use: vec![HookRule {
                matcher: "*".into(),
                hooks: vec![HookAction::Prompt {
                    prompt: "Check output for sensitive data".into(),
                }],
            }],
        };

        registry.register("security-guard", "/tmp/security", def);

        let pre = registry
            .run_pre_tool_use("Bash", &serde_json::json!({"command": "git status"}))
            .await;
        println!("    PreToolUse(Bash) → messages: {:?}", pre.messages);
        println!("    PreToolUse(Bash) → blocked:  {}", pre.block);

        let pre_read = registry
            .run_pre_tool_use("Read", &serde_json::json!({"path": "/etc/passwd"}))
            .await;
        println!(
            "    PreToolUse(Read) → messages: {:?} (no match)",
            pre_read.messages
        );

        let post = registry
            .run_post_tool_use("Read", &serde_json::json!({}), "file content here")
            .await;
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
        let descriptors = loader.discover_from_dir("skills").await?;

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
    }
    println!();

    // 6f: Activation with inline execution
    println!("  6f. Activation with Inline Execution");
    println!("  ────────────────────────────────────");
    {
        let mut loader = SkillLoader::new();
        let descriptors = loader.discover_from_dir("skills").await?;

        let mut registry = SkillRegistry::new();
        for desc in descriptors {
            registry.register_descriptor(desc);
        }

        if registry.get_descriptor("project-stats").is_some() {
            let content = registry.activate("project-stats").await?;
            println!("    After activation, instructions contain resolved values:");
            for line in content.instructions.lines().take(8) {
                println!("    │ {}", line);
            }
            if content.instructions.lines().count() > 8 {
                println!("    │ ...");
            }
            println!("    (${{SKILL_DIR}} and !`uname -s` have been replaced with actual values)");
        }
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
        println!("  [skip] redbook-image-generator skill not found");
        return Ok(());
    }

    // 7a: Discover & activate
    println!("  7a. Skill Discovery & Activation");
    println!("  ──────────────────────────────────");
    {
        let mut loader = SkillLoader::new();
        let descriptors = loader.discover_from_dir("skills").await?;

        let xhs = descriptors
            .iter()
            .find(|d| d.name == "xiaohongshu-image-generator");

        match xhs {
            Some(desc) => {
                println!("    Name:        {}", desc.name);
                println!("    Description: {}", desc.description);

                let mut registry = SkillRegistry::new();
                for desc in descriptors {
                    registry.register_descriptor(desc);
                }

                let content = registry.activate("xiaohongshu-image-generator").await?;
                println!("\n    Instructions ({} chars):", content.instructions.len());
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
            None => {
                println!("    [skip] xiaohongshu-image-generator skill not found in skills/");
                return Ok(());
            }
        }
    }
    println!();

    // 7b: Generate cover image
    println!("  7b. Generate Cover Image");
    println!("  ─────────────────────────");
    {
        let script = "skills/redbook-image-generator-1.0.0/scripts/generate_cover.py";
        let output_path = "/tmp/xhs_demo_cover.jpg";

        // Check Python + PIL availability
        let check = tokio::process::Command::new("python3")
            .arg("-c")
            .arg("from PIL import Image; print('ok')")
            .output()
            .await;

        match check {
            Ok(out) if out.status.success() => {
                let title = "今日分享";
                let subtitle = "打卡第100天";
                let content = "Rust + AI = 🚀\\n技能系统真好用\\n继续加油！";

                println!("    Title:   {}", title);
                println!("    Subtitle: {}", subtitle);
                println!("    Content: {}", content.replace("\\n", " / "));

                let output = tokio::process::Command::new("python3")
                    .arg(script)
                    .arg(title)
                    .arg(subtitle)
                    .arg(content)
                    .env("OUTPUT_PATH", output_path)
                    .output()
                    .await;

                match output {
                    Ok(out) if out.status.success() => {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        println!("\n    {}", stdout.trim());

                        // Verify image was created
                        if std::path::Path::new(output_path).exists() {
                            let meta = std::fs::metadata(output_path)?;
                            println!("    Image saved: {} ({} bytes)", output_path, meta.len());
                        }
                    }
                    Ok(out) => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        println!(
                            "    [error] exit code {:?}: {}",
                            out.status.code(),
                            stderr.chars().take(300).collect::<String>()
                        );
                    }
                    Err(e) => println!("    [error] failed to run script: {}", e),
                }
            }
            _ => {
                println!("    [skip] Python PIL not available, using mock output:");
                println!("    Would generate: {}x{} image", 1080, 1440);
                println!("    Title:   今日分享");
                println!("    Subtitle: 打卡第100天");
                println!("    Content: Rust + AI = 🚀 / 技能系统真好用 / 继续加油！");
                println!("    Output:  /tmp/xhs_demo_cover.jpg");
            }
        }
    }
    println!();

    // 7c: Agent integration demo
    println!("  7c. Agent Integration");
    println!("  ──────────────────────");
    {}

    println!();
    Ok(())
}
