//! demo09_file_shell.rs —— 文件系统 + Shell 综合演示
//!
//! 演示 FileSystemSkill 与 ShellSkill 的协同使用：
//! Agent 作为一个"代码仓库小助手"，完成以下工作流：
//!
//! ```text
//! Part 1: Skill 元数据与安全策略展示（不需要 LLM）
//! Part 2: Shell 安全检查独立测试（不需要 LLM）
//! Part 3: Agent 执行真实任务（需要 LLM 配置）
//!   场景 A —— 文件读写工作流
//!             list_dir → write_file → read_file → update_file
//!   场景 B —— Shell 命令组合
//!             git status → git log → grep 搜索
//!   场景 C —— File + Shell 协同
//!             写入文件 → shell 统计行数/单词数 → 追加摘要
//! ```
//!
//! # 运行
//! ```bash
//! cargo run --example demo09_file_shell
//! ```

use echo_agent::agent::Agent;
use echo_agent::agent::react_agent::{AgentConfig, ReactAgent};
use echo_agent::skills::Skill;
use echo_agent::skills::builtin::{FileSystemSkill, ShellSkill};
use echo_agent::tools::shell::{CommandSafety, ShellTool};

// ── 入口 ──────────────────────────────────────────────────────────────────────

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
        println!(
            "    Prompt注入: {}",
            if skill.system_prompt_injection().is_some() {
                "✓"
            } else {
                "✗"
            }
        );
        println!();
    }
}

// ── Part 2: Shell 安全策略验证 ───────────────────────────────────────────────

fn demo_shell_safety() {
    println!("{}", "─".repeat(55));
    println!("Part 2: Shell 三级安全策略验证\n");

    let tool = ShellTool::new();

    let cases: &[(&str, &str)] = &[
        // 安全命令
        ("ls -la /tmp", "文件查看"),
        ("git status", "git 只读子命令"),
        ("git log --oneline -5", "git log"),
        ("cargo check", "cargo 构建检查"),
        ("grep -r TODO src/", "代码搜索"),
        ("rg 'fn main' --type rust", "ripgrep"),
        // 需确认命令
        ("rm -rf /tmp/test", "文件删除"),
        ("git add .", "git 暂存"),
        ("git commit -m 'fix'", "git 提交"),
        ("curl https://api.example.com", "网络请求"),
        ("npm install lodash", "包安装"),
        ("python3 script.py", "脚本执行"),
        // 危险命令
        ("sudo rm -rf /", "极危 - 需要 sudo"),
        ("dd if=/dev/zero of=/dev/sda", "极危 - 磁盘覆写"),
        ("git reset --hard HEAD~3", "极危 - git 硬重置"),
        ("chmod 777 /etc/passwd", "极危 - 权限修改"),
        ("shutdown -h now", "极危 - 系统关机"),
    ];

    let mut safe_count = 0;
    let mut approval_count = 0;
    let mut dangerous_count = 0;

    for (cmd, desc) in cases {
        let safety = tool.check_command_safety(cmd);
        let (icon, label) = match &safety {
            CommandSafety::Safe => {
                safe_count += 1;
                ("✅", "Safe      ")
            }
            CommandSafety::RequiresApproval(_) => {
                approval_count += 1;
                ("⚠️ ", "NeedApproval")
            }
            CommandSafety::Dangerous(_) => {
                dangerous_count += 1;
                ("🚫", "Dangerous ")
            }
        };
        println!("  {icon} [{label}] {desc}");
        println!("    命令: `{cmd}`");
        if let CommandSafety::RequiresApproval(reason) | CommandSafety::Dangerous(reason) = &safety
        {
            println!("    原因: {reason}");
        }
        println!();
    }

    println!(
        "  统计: ✅ Safe×{safe_count}  ⚠️  NeedApproval×{approval_count}  🚫 Dangerous×{dangerous_count}\n"
    );
}

// ── Part 3: Agent 真实任务执行 ───────────────────────────────────────────────

async fn demo_agent_tasks() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 3: Agent 执行真实任务\n");

    if !has_llm_config() {
        println!("跳过 Part 3：未检测到 LLM API 密钥。");
        println!("（配置 QWEN_API_KEY / OPENAI_API_KEY / DEEPSEEK_API_KEY 后可启用）\n");
        return Ok(());
    }

    let work_dir = "/tmp/echo_agent_demo";

    // ── 场景 A: 文件读写工作流 ──────────────────────────────────────────────
    println!("{}", "·".repeat(40));
    println!("场景 A: 文件读写工作流\n");
    {
        let system_prompt = format!(
            "你是一个文件管理助手，所有文件操作都在 `{work_dir}` 目录下进行。\n\
             遇到不存在的目录，先用 create_file 创建占位文件（目录会自动创建），\
             或用 write_file 写入时父目录会自动创建。\n\
             请一步一步完成任务，每步调用一个工具，认真检查结果。"
        );

        let config = AgentConfig::new("qwen3-max", "file-agent", &system_prompt)
            .enable_tool(true)
            .enable_task(false)
            .enable_human_in_loop(false)
            .enable_subagent(false)
            .max_iterations(15);

        let mut agent = ReactAgent::new(config);
        agent.add_skill(Box::new(FileSystemSkill::with_base_dir(work_dir)));

        let task = format!(
            "请完成以下文件操作（所有路径都在 {work_dir} 目录下）：\n\
             1. 在 {work_dir}/notes.md 写入以下内容（覆盖写）：\n\
                ```\n\
                # 项目笔记\n\n\
                ## 2026-02-27\n\
                - 完成了 shell 工具的实现\n\
                - 完成了文件工具的实现（create/delete/read/write/update/move/list）\n\
                - 修复了 UpdateFileTool 的 seek bug\n\
                - 修复了 MoveFileTool 的路径校验逻辑\n\
                ```\n\
             2. 读取 {work_dir}/notes.md，确认内容已写入\n\
             3. 用 update_file 把 \"seek bug\" 改为 \"文件指针 bug（已修复）\"\n\
             4. 在 {work_dir}/notes.md 末尾追加一行：`- 替换 lazy_static 为 std::sync::LazyLock`\n\
             5. 再次读取文件，输出最终内容"
        );

        println!("任务: {task}\n");
        match agent.execute(&task).await {
            Ok(result) => println!("✓ 结果:\n{result}\n"),
            Err(e) => println!("✗ 失败: {e}\n"),
        }
    }

    // ── 场景 B: Shell 命令使用 ──────────────────────────────────────────────
    println!("{}", "·".repeat(40));
    println!("场景 B: Shell 命令查询\n");
    {
        let system_prompt = "你是一个代码仓库助手，帮助用户了解当前项目的状态。\n\
             使用 shell 工具执行命令，每步只执行一条命令，根据结果分析并汇报。";

        let config = AgentConfig::new("qwen3-max", "shell-agent", system_prompt)
            .enable_tool(true)
            .enable_task(false)
            .enable_human_in_loop(false)
            .enable_subagent(false)
            .max_iterations(10);

        let mut agent = ReactAgent::new(config);
        agent.add_skill(Box::new(ShellSkill::new()));

        let task = "请帮我了解当前 Rust 项目的基本情况：\n\
                    1. 用 git status 查看当前有哪些文件被修改了\n\
                    2. 用 git log --oneline -5 查看最近 5 条提交记录\n\
                    3. 统计 src/ 目录下共有多少个 .rs 文件（用 find 命令）\n\
                    最后给出一段简洁的项目状态摘要";

        println!("任务: {task}\n");
        match agent.execute(task).await {
            Ok(result) => println!("✓ 结果:\n{result}\n"),
            Err(e) => println!("✗ 失败: {e}\n"),
        }
    }

    // ── 场景 C: File + Shell 协同 ───────────────────────────────────────────
    println!("{}", "·".repeat(40));
    println!("场景 C: 文件系统 + Shell 协同工作\n");
    {
        let system_prompt = format!(
            "你是一个代码分析助手，可以读写文件（限制在 {work_dir} 目录），也可以执行安全的 shell 命令。\n\
             充分利用两种能力：先用 shell 收集信息，再用文件工具整理保存结果。"
        );

        let config = AgentConfig::new("qwen3-max", "combo-agent", &system_prompt)
            .enable_tool(true)
            .enable_task(false)
            .enable_human_in_loop(false)
            .enable_subagent(false)
            .max_iterations(20);

        let mut agent = ReactAgent::new(config);
        agent.add_skill(Box::new(FileSystemSkill::with_base_dir(work_dir)));
        agent.add_skill(Box::new(ShellSkill::new()));

        let task = format!(
            "请完成一个「代码统计报告」任务：\n\
             1. 用 shell 统计项目 src/ 目录下所有 .rs 文件的数量\n\
             2. 用 shell 统计 src/ 目录下所有 .rs 文件的总行数（find + wc 组合）\n\
             3. 用 shell 找出 src/ 中包含 'pub struct' 定义的文件列表（grep -rl）\n\
             4. 将以上统计结果整理成 Markdown 报告，\
                写入 {work_dir}/code_report.md\n\
             5. 读取 {work_dir}/code_report.md 确认写入成功，输出报告内容"
        );

        println!("任务: {task}\n");
        match agent.execute(&task).await {
            Ok(result) => println!("✓ 结果:\n{result}\n"),
            Err(e) => println!("✗ 失败: {e}\n"),
        }
    }

    Ok(())
}

fn has_llm_config() -> bool {
    std::env::var("QWEN_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
        || std::env::var("DEEPSEEK_API_KEY").is_ok()
}
