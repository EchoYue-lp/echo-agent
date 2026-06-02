//! demo62_prompt_templates —— Prompt Template Manager 综合演示
//!
//! 展示 `PromptTemplateManager` 的核心能力：
//!
//! 1. 注册模板 + 变量替换 `{{name}}`
//! 2. 默认值语法 `{{name:default}}`
//! 3. 条件块 `{{#if var}}…{{#else}}…{{#endif}}`
//! 4. 模板组合 / 继承（通过 render_template 嵌套渲染）
//! 5. 与 ModeEngine / LocalizedModeEngine 集成
//!
//! ```bash
//! cargo run --example demo62_prompt_templates
//! ```

use echo_agent::prelude::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("echo_agent=info")
        .init();

    print_banner();

    // ── Part 1: 基础变量替换 ─────────────────────────────────────────────────
    separator("Part 1: 基础变量替换 {{variable}}");
    demo_basic_substitution()?;

    // ── Part 2: 默认值语法 ──────────────────────────────────────────────────
    separator("Part 2: 默认值语法 {{variable:default}}");
    demo_default_values()?;

    // ── Part 3: 条件块 ──────────────────────────────────────────────────────
    separator("Part 3: 条件块 {{#if}}...{{#else}}...{{#endif}}");
    demo_conditional_blocks()?;

    // ── Part 4: 模板组合 / 继承 ─────────────────────────────────────────────
    separator("Part 4: 模板组合 — 先渲染子模板，再嵌入父模板");
    demo_template_composition()?;

    // ── Part 5: 与 ModeEngine 集成 ──────────────────────────────────────────
    separator("Part 5: 与 ModeEngine / LocalizedModeEngine 集成");
    demo_mode_engine_integration()?;

    // ── Part 6: 线程安全共享 ────────────────────────────────────────────────
    separator("Part 6: Arc<PromptTemplateManager> 线程安全共享");
    demo_thread_safe_sharing()?;

    println!("\n{}", "═".repeat(64));
    println!("  demo62 完成 ✓");
    println!("{}", "═".repeat(64));

    Ok(())
}

// ── Part 1: 基础变量替换 ─────────────────────────────────────────────────────

fn demo_basic_substitution() -> echo_agent::error::Result<()> {
    let manager = PromptTemplateManager::new();

    // 注册一个简单的问候模板
    manager.register("greeting", "Hello, {{name}}! Welcome to {{project}}.");

    let result = manager.render("greeting", &[("name", "Alice"), ("project", "EchoAgent")])?;
    println!("  模板:  \"Hello, {{{{name}}}}! Welcome to {{{{project}}}}.\"");
    println!("  渲染:  \"{result}\"");
    assert_eq!(result, "Hello, Alice! Welcome to EchoAgent.");

    // 多变量系统提示
    manager.register(
        "system_prompt",
        "You are a {{role}} assistant specialized in {{domain}}. \
         Always respond in {{language}}.",
    );
    let prompt = manager.render(
        "system_prompt",
        &[
            ("role", "coding"),
            ("domain", "Rust development"),
            ("language", "Chinese"),
        ],
    )?;
    println!("\n  系统提示模板渲染:");
    println!("  → \"{prompt}\"");
    assert!(prompt.contains("coding"));
    assert!(prompt.contains("Rust development"));
    assert!(prompt.contains("Chinese"));

    println!("  → 基础替换 ✓");
    Ok(())
}

// ── Part 2: 默认值语法 ──────────────────────────────────────────────────────

fn demo_default_values() -> echo_agent::error::Result<()> {
    let manager = PromptTemplateManager::new();

    // {{name:Guest}} — 如果未提供 name，则使用 "Guest"
    manager.register(
        "welcome",
        "Welcome, {{name:Guest}}! Your plan: {{plan:Free}}.",
    );

    // 不提供任何变量 → 使用默认值
    let with_defaults = manager.render("welcome", &[])?;
    println!("  无变量: \"{with_defaults}\"");
    assert_eq!(with_defaults, "Welcome, Guest! Your plan: Free.");

    // 提供部分变量 → 混合使用
    let partial = manager.render("welcome", &[("name", "Bob")])?;
    println!("  部分提供: \"{partial}\"");
    assert_eq!(partial, "Welcome, Bob! Your plan: Free.");

    // 提供全部变量 → 覆盖默认值
    let full = manager.render("welcome", &[("name", "Carol"), ("plan", "Pro")])?;
    println!("  全部提供: \"{full}\"");
    assert_eq!(full, "Welcome, Carol! Your plan: Pro.");

    println!("  → 默认值语法 ✓");
    Ok(())
}

// ── Part 3: 条件块 ──────────────────────────────────────────────────────────

fn demo_conditional_blocks() -> echo_agent::error::Result<()> {
    let manager = PromptTemplateManager::new();

    // 简单条件：有变量时包含，无变量时省略
    manager.register(
        "report",
        "## Summary\nBase report.{{#if detail}} Additional details: {{detail}}.{{#endif}} End.",
    );

    let with_detail = manager.render("report", &[("detail", "CPU usage at 85%")])?;
    let without_detail = manager.render("report", &[])?;
    println!("  有条件变量:   \"{with_detail}\"");
    println!("  无条件变量:   \"{without_detail}\"");
    assert!(with_detail.contains("CPU usage at 85%"));
    assert!(!without_detail.contains("Additional details"));

    // 条件 + else 分支
    manager.register(
        "access",
        "{{#if premium}}✅ Premium features enabled.{{#else}}🔒 Standard features only.{{#endif}}",
    );

    let premium = manager.render("access", &[("premium", "true")])?;
    let standard = manager.render("access", &[])?;
    println!("\n  Premium 用户: \"{premium}\"");
    println!("  Standard 用户: \"{standard}\"");
    assert!(premium.contains("Premium"));
    assert!(standard.contains("Standard"));

    // 嵌套条件
    manager.register(
        "nested",
        "{{#if auth}}\
         Authenticated. {{#if admin}}Admin panel unlocked.\
         {{#else}}User dashboard.{{#endif}}\
         {{#else}}Please log in.{{#endif}}",
    );

    let admin = manager.render("nested", &[("auth", "yes"), ("admin", "yes")])?;
    let user = manager.render("nested", &[("auth", "yes")])?;
    let guest = manager.render("nested", &[])?;
    println!("\n  嵌套条件:");
    println!("    Admin:  \"{admin}\"");
    println!("    User:   \"{user}\"");
    println!("    Guest:  \"{guest}\"");
    assert!(admin.contains("Admin panel"));
    assert!(user.contains("User dashboard"));
    assert!(guest.contains("Please log in"));

    println!("  → 条件块 ✓");
    Ok(())
}

// ── Part 4: 模板组合 / 继承 ──────────────────────────────────────────────────

fn demo_template_composition() -> echo_agent::error::Result<()> {
    let manager = PromptTemplateManager::new();

    // 子模板：独立的片段
    manager.register("fragment_role", "You are a {{role}} assistant.");
    manager.register("fragment_workflow", "Follow this workflow: {{steps}}.");

    // 先渲染子模板
    let role_text = manager.render("fragment_role", &[("role", "data analysis")])?;
    let workflow_text = manager.render(
        "fragment_workflow",
        &[("steps", "explore → clean → analyze → visualize")],
    )?;

    // 用 render_template 直接渲染一个组合模板（不注册）
    let composed = manager.render_template(
        "{{role_block}}\n{{workflow_block}}\nAlways be concise.",
        &[
            ("role_block", &role_text),
            ("workflow_block", &workflow_text),
        ],
    );

    println!("  子模板 1 (role):     \"{role_text}\"");
    println!("  子模板 2 (workflow): \"{workflow_text}\"");
    println!("  组合结果:");
    for line in composed.lines() {
        println!("    {line}");
    }

    assert!(composed.contains("data analysis"));
    assert!(composed.contains("explore"));
    assert!(composed.contains("concise"));

    println!("  → 模板组合 ✓");
    Ok(())
}

// ── Part 5: 与 ModeEngine 集成 ──────────────────────────────────────────────

fn demo_mode_engine_integration() -> echo_agent::error::Result<()> {
    // ── 5a. DefaultModeEngine 内置模板 ──
    let default_engine = DefaultModeEngine;
    let coding_config = default_engine.mode_config(&AgentMode::Coding);
    println!("  [DefaultModeEngine] Coding 模式:");
    println!(
        "    Display: {} {}",
        coding_config.icon, coding_config.display_name
    );
    println!("    推荐工具: {:?}", coding_config.recommended_tools);
    println!(
        "    系统提示 (前80字符): {:.80}...",
        coding_config.system_prompt_template
    );

    // ── 5b. LocalizedModeEngine 中文本地化 ──
    let zh_engine = LocalizedModeEngine::with_chinese();
    let zh_coding = zh_engine.mode_config(&AgentMode::Coding);
    println!("\n  [LocalizedModeEngine] 编程模式 (中文):");
    println!("    Display: {} {}", zh_coding.icon, zh_coding.display_name);
    println!(
        "    系统提示 (前80字符): {:.80}...",
        zh_coding.system_prompt_template
    );

    // ── 5c. 从 DefaultModeEngine 加载到 PromptTemplateManager ──
    let manager = PromptTemplateManager::with_default_mode_templates();
    let names = manager.template_names();
    println!("\n  [PromptTemplateManager::with_default_mode_templates()]");
    println!("    已注册模板: {:?}", names);
    assert!(manager.contains("mode_general"));
    assert!(manager.contains("mode_coding"));
    assert!(manager.contains("mode_research"));
    assert!(manager.contains("mode_data"));
    assert!(manager.contains("mode_writing"));

    // 渲染一个内置模式模板
    let general_prompt = manager.render("mode_general", &[])?;
    println!("    mode_general 渲染: {:.60}...", general_prompt);

    // ── 5d. 自定义模式覆盖 + 模板注册 ──
    let custom_engine = LocalizedModeEngine::new().with_override(
        AgentMode::Coding,
        "你是 {{team}} 团队的专属编程助手。".into(),
    );
    let custom_config = custom_engine.mode_config(&AgentMode::Coding);
    // 将带变量的自定义模板注册到 PromptTemplateManager
    let custom_manager = PromptTemplateManager::new();
    custom_manager.register("custom_coding", &custom_config.system_prompt_template);
    let rendered = custom_manager.render("custom_coding", &[("team", "Platform")])?;
    println!("\n  [自定义模式覆盖]");
    println!("    渲染: \"{rendered}\"");
    assert_eq!(rendered, "你是 Platform 团队的专属编程助手。");

    println!("  → ModeEngine 集成 ✓");
    Ok(())
}

// ── Part 6: 线程安全共享 ────────────────────────────────────────────────────

fn demo_thread_safe_sharing() -> echo_agent::error::Result<()> {
    let manager = Arc::new(PromptTemplateManager::new());
    manager.register("shared", "Hello, {{name}}!");

    let m1 = Arc::clone(&manager);
    let m2 = Arc::clone(&manager);

    let r1 = m1.render("shared", &[("name", "Thread-A")])?;
    let r2 = m2.render("shared", &[("name", "Thread-B")])?;

    println!("  Arc 副本 1: \"{r1}\"");
    println!("  Arc 副本 2: \"{r2}\"");
    assert_eq!(r1, "Hello, Thread-A!");
    assert_eq!(r2, "Hello, Thread-B!");

    // 验证 contains / remove / template_names
    assert!(manager.contains("shared"));
    assert_eq!(manager.template_names().len(), 1);
    assert!(manager.remove("shared"));
    assert!(!manager.contains("shared"));
    println!("  contains / remove / template_names ✓");

    // render_or_raw — 静态模板优化（跳过解析）
    manager.register("static_text", "No variables here.");
    let raw = manager.render_or_raw("static_text", &[])?;
    assert_eq!(raw, "No variables here.");
    println!("  render_or_raw (静态优化) ✓");

    // 错误处理 — 未注册的模板名
    let err = manager.render("nonexistent", &[]);
    assert!(err.is_err());
    println!("  未注册模板错误处理 ✓");

    println!("  → 线程安全共享 ✓");
    Ok(())
}

// ── 辅助 ─────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("{}", "═".repeat(64));
    println!("      Echo Agent × Prompt Template Manager (demo62)");
    println!("{}", "═".repeat(64));
    println!();
}

fn separator(title: &str) {
    println!("{}", "─".repeat(64));
    println!("{title}\n");
}
