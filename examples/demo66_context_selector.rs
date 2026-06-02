//! demo66_context_selector.rs - ContextSelector 文件选择示例
//!
//! 本示例演示如何使用 ContextSelector 基于任务描述评分和选择相关文件，
//! 包括符号匹配、最近修改和 Git 变更的加权评分。
//!
//! 运行方式: cargo run --example demo66_context_selector

use echo_agent::context::ContextSelector;
use std::collections::HashMap;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== demo66: Context Selector ===\n");

    // 示例 1: 基本用法
    println!("--- 示例 1: 基本用法 ---");
    basic_usage()?;

    // 示例 2: 自定义权重
    println!("\n--- 示例 2: 自定义权重 ---");
    custom_weights()?;

    // 示例 3: 多任务场景
    println!("\n--- 示例 3: 多任务场景 ---");
    multiple_tasks()?;

    // 示例 4: 实际项目场景
    println!("\n--- 示例 4: 实际项目场景 ---");
    real_world_scenario()?;

    println!("\n=== demo66 完成 ===");
    Ok(())
}

/// 示例 1: 基本用法
fn basic_usage() -> Result<(), Box<dyn std::error::Error>> {
    let selector = ContextSelector::new();

    // 模拟项目中的文件及其符号
    let mut symbols = HashMap::new();
    symbols.insert(
        PathBuf::from("src/auth.rs"),
        vec!["login".into(), "authenticate".into(), "AuthError".into()],
    );
    symbols.insert(
        PathBuf::from("src/database.rs"),
        vec!["connect".into(), "query".into(), "DatabasePool".into()],
    );
    symbols.insert(
        PathBuf::from("src/utils.rs"),
        vec!["format".into(), "parse".into(), "validate".into()],
    );
    symbols.insert(
        PathBuf::from("src/api.rs"),
        vec!["endpoint".into(), "handler".into(), "Response".into()],
    );

    // 模拟最近修改的文件
    let recent = vec![
        PathBuf::from("src/database.rs"),
        PathBuf::from("src/utils.rs"),
    ];

    // 模拟 Git 变更的文件
    let git_changed = vec![PathBuf::from("src/auth.rs"), PathBuf::from("src/api.rs")];

    let task = "fix the login authentication bug";

    // 获取评分结果
    let scored = selector.score_files(task, &symbols, &recent, &git_changed);

    println!("任务: {}", task);
    println!("\n文件评分结果 (从高到低):");
    for (i, (file, score)) in scored.iter().enumerate() {
        println!("  [{}] {} (score: {:.2})", i + 1, file.display(), score);
    }

    // 选择最相关的文件
    let selected = selector.select_relevant(task, &symbols, &recent, &git_changed);
    println!("\n选择的相关文件 (最多 {} 个):", selector.max_files);
    for (i, file) in selected.iter().enumerate() {
        println!("  [{}] {}", i + 1, file.display());
    }

    Ok(())
}

/// 示例 2: 自定义权重
fn custom_weights() -> Result<(), Box<dyn std::error::Error>> {
    let mut symbols = HashMap::new();
    symbols.insert(
        PathBuf::from("src/api.rs"),
        vec!["endpoint".into(), "handler".into()],
    );
    symbols.insert(
        PathBuf::from("src/models.rs"),
        vec!["User".into(), "Post".into()],
    );
    symbols.insert(
        PathBuf::from("src/handlers.rs"),
        vec!["create_user".into(), "update_post".into()],
    );
    symbols.insert(
        PathBuf::from("src/config.rs"),
        vec!["load_config".into(), "Settings".into()],
    );

    let recent = vec![PathBuf::from("src/config.rs")];
    let git_changed = vec![PathBuf::from("src/models.rs")];

    let task = "Update API endpoint";

    // 配置 1: 优先符号匹配
    println!("\n配置 1: 优先符号匹配 (symbol_weight: 2.0)");
    let selector1 = ContextSelector {
        symbol_weight: 2.0,
        recency_weight: 0.3,
        git_diff_weight: 0.5,
        max_files: 3,
    };
    let selected1 = selector1.select_relevant(task, &symbols, &recent, &git_changed);
    print_selection(&selected1);

    // 配置 2: 优先最近修改
    println!("\n配置 2: 优先最近修改 (recency_weight: 2.0)");
    let selector2 = ContextSelector {
        symbol_weight: 0.5,
        recency_weight: 2.0,
        git_diff_weight: 0.8,
        max_files: 3,
    };
    let selected2 = selector2.select_relevant(task, &symbols, &recent, &git_changed);
    print_selection(&selected2);

    // 配置 3: 优先 Git 变更
    println!("\n配置 3: 优先 Git 变更 (git_diff_weight: 2.0)");
    let selector3 = ContextSelector {
        symbol_weight: 0.5,
        recency_weight: 0.3,
        git_diff_weight: 2.0,
        max_files: 3,
    };
    let selected3 = selector3.select_relevant(task, &symbols, &recent, &git_changed);
    print_selection(&selected3);

    Ok(())
}

/// 示例 3: 多任务场景
fn multiple_tasks() -> Result<(), Box<dyn std::error::Error>> {
    let selector = ContextSelector::new();

    let mut symbols = HashMap::new();
    symbols.insert(
        PathBuf::from("src/auth.rs"),
        vec!["login".into(), "token".into(), "Session".into()],
    );
    symbols.insert(
        PathBuf::from("src/database.rs"),
        vec!["query".into(), "insert".into(), "Connection".into()],
    );
    symbols.insert(
        PathBuf::from("src/cache.rs"),
        vec!["get".into(), "set".into(), "invalidate".into()],
    );
    symbols.insert(
        PathBuf::from("src/api.rs"),
        vec!["route".into(), "handler".into()],
    );
    symbols.insert(
        PathBuf::from("src/models.rs"),
        vec!["User".into(), "Post".into()],
    );

    let recent = vec![PathBuf::from("src/api.rs"), PathBuf::from("src/models.rs")];

    let git_changed = vec![PathBuf::from("src/database.rs")];

    // 任务 1: 认证相关
    let task1 = "Implement OAuth2 authentication";
    let selected1 = selector.select_relevant(task1, &symbols, &recent, &git_changed);
    println!("任务 1: {}", task1);
    print_selection(&selected1);

    // 任务 2: 数据库优化
    let task2 = "Optimize database queries";
    let selected2 = selector.select_relevant(task2, &symbols, &recent, &git_changed);
    println!("\n任务 2: {}", task2);
    print_selection(&selected2);

    // 任务 3: 缓存问题
    let task3 = "Fix cache invalidation bug";
    let selected3 = selector.select_relevant(task3, &symbols, &recent, &git_changed);
    println!("\n任务 3: {}", task3);
    print_selection(&selected3);

    Ok(())
}

/// 示例 4: 实际项目场景
fn real_world_scenario() -> Result<(), Box<dyn std::error::Error>> {
    let selector = ContextSelector {
        symbol_weight: 1.0,
        recency_weight: 0.6,
        git_diff_weight: 0.8,
        max_files: 8,
    };

    // 模拟一个真实项目的符号映射
    let mut symbols = HashMap::new();

    // 核心模块
    symbols.insert(
        PathBuf::from("src/agent/react/mod.rs"),
        vec!["ReactAgent".into(), "execute".into()],
    );
    symbols.insert(
        PathBuf::from("src/agent/react/builder.rs"),
        vec!["ReactAgentBuilder".into(), "build".into()],
    );
    symbols.insert(
        PathBuf::from("src/agent/react/run/mod.rs"),
        vec!["run_react_loop".into()],
    );
    symbols.insert(
        PathBuf::from("src/agent/react/run/execution.rs"),
        vec!["execute_tool".into(), "ToolExecution".into()],
    );
    symbols.insert(
        PathBuf::from("src/agent/react/run/pipeline.rs"),
        vec!["ToolExecutionPipeline".into(), "PipelineStage".into()],
    );

    // 工具模块
    symbols.insert(
        PathBuf::from("src/tools/mod.rs"),
        vec!["Tool".into(), "ToolManager".into()],
    );
    symbols.insert(
        PathBuf::from("src/tools/builtin/memory.rs"),
        vec!["MemoryTool".into(), "recall".into()],
    );

    // 上下文模块
    symbols.insert(
        PathBuf::from("src/context/mod.rs"),
        vec!["ContextAssembler".into(), "ContextSelector".into()],
    );
    symbols.insert(
        PathBuf::from("src/context/assembler.rs"),
        vec!["assemble".into(), "ContextBudget".into()],
    );

    // 最近修改
    let recent = vec![
        PathBuf::from("src/agent/react/builder.rs"),
        PathBuf::from("src/context/assembler.rs"),
    ];

    // Git 变更
    let git_changed = vec![
        PathBuf::from("src/agent/react/run/pipeline.rs"),
        PathBuf::from("src/tools/builtin/memory.rs"),
    ];

    // 场景: 修复 React agent 的执行流水线问题
    let task = "Fix the tool execution pipeline in React agent loop";

    let scored = selector.score_files(task, &symbols, &recent, &git_changed);
    let selected = selector.select_relevant(task, &symbols, &recent, &git_changed);

    println!("实际项目场景:");
    println!("任务: {}", task);
    println!("\n评分结果:");
    for (i, (file, score)) in scored.iter().enumerate().take(10) {
        // 标记文件类型
        let file_type = if file.to_str().unwrap().contains("test") {
            "🧪 test"
        } else if file.to_str().unwrap().contains("config")
            || file.to_str().unwrap().contains("Cargo")
        {
            "⚙️ config"
        } else {
            "📄 source"
        };

        println!(
            "  [{}] {} {} (score: {:.2})",
            i + 1,
            file_type,
            file.display(),
            score
        );
    }

    println!("\n选择的相关文件:");
    for (i, file) in selected.iter().enumerate() {
        let file_type = if file.to_str().unwrap().contains("test") {
            "🧪 test"
        } else if file.to_str().unwrap().contains("config")
            || file.to_str().unwrap().contains("Cargo")
        {
            "⚙️ config"
        } else {
            "📄 source"
        };

        println!("  [{}] {} {}", i + 1, file_type, file.display());
    }

    println!("\n💡 提示: 可以使用这些文件作为上下文来修复问题");

    Ok(())
}

/// 打印选择结果
fn print_selection(selected: &[PathBuf]) {
    for (i, file) in selected.iter().enumerate() {
        println!("  [{}] {}", i + 1, file.display());
    }
}
