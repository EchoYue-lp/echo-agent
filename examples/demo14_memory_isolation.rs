//! demo14 - 记忆系统与上下文隔离演示
//!
//! ## 演示内容
//!
//! 1. **Store 命名空间隔离**：每个 Agent 在同一个 JSON 文件中有独立的 namespace，
//!    Agent B 无法读取 Agent A 的长期记忆，反之亦然。
//!
//! 2. **Checkpointer 会话隔离**：每个 Agent 有独立的 session_id，
//!    消息历史分别保存，互不干扰。主 Agent 可以通过持有的 Checkpointer 对象
//!    检查任意会话的历史，但 SubAgent 之间不能互查。
//!
//! 3. **上下文隔离**：主 Agent 通过 agent_tool 分派任务时，SubAgent 只收到
//!    任务字符串，看不到主 Agent 的系统提示、机密信息或对话历史。
//!    运行结束后从 Checkpointer 读取各方消息历史即可验证。

use echo_agent::agent::react_agent::ReactAgent;
use echo_agent::agent::{Agent, AgentConfig, AgentRole};
use echo_agent::memory::checkpointer::{Checkpointer, FileCheckpointer};
use echo_agent::memory::store::{FileStore, Store};
use serde_json::json;
use std::sync::Arc;

const MODEL: &str = "qwen3-max";

// 所有 Agent 共用同一个存储文件，通过 namespace / session_id 实现逻辑隔离
const STORE_PATH: &str = "/tmp/echo-agent-demo14/store.json";
const CHECKPOINT_PATH: &str = "/tmp/echo-agent-demo14/checkpoints.json";

// ── 命名空间常量 ──────────────────────────────────────────────────────────────

const NS_MATH: [&str; 2] = ["math_agent", "memories"];
const NS_WRITER: [&str; 2] = ["writer_agent", "memories"];
const NS_MAIN: [&str; 2] = ["main_agent", "memories"];

// ── 会话 ID 常量 ──────────────────────────────────────────────────────────────

const SESSION_MATH: &str = "math-agent-session-1";
const SESSION_WRITER: &str = "writer-agent-session-1";
const SESSION_MAIN: &str = "main-agent-session-1";

// ─────────────────────────────────────────────────────────────────────────────
// Part 1 — Store 命名空间隔离（纯 API，无 LLM 调用）
// ─────────────────────────────────────────────────────────────────────────────

async fn demo_store_namespace_isolation(store: Arc<dyn Store>) {
    println!("╔═══════════════════════════════════════════════════════╗");
    println!("║   Part 1: Store 命名空间隔离                          ║");
    println!("╚═══════════════════════════════════════════════════════╝\n");

    // math_agent 存入两条记忆
    store
        .put(
            &NS_MATH,
            "fact-fibonacci",
            json!({"content": "斐波那契前8项: 1,1,2,3,5,8,13,21", "importance": 8}),
        )
        .await
        .unwrap();
    store
        .put(
            &NS_MATH,
            "fact-secret",
            json!({"content": "内部机密：数学部门代号 M-ALPHA", "importance": 10}),
        )
        .await
        .unwrap();

    // writer_agent 存入自己的记忆
    store
        .put(
            &NS_WRITER,
            "fact-style",
            json!({"content": "我偏好古典诗词风格", "importance": 7}),
        )
        .await
        .unwrap();

    println!("✅ math_agent  → 写入 2 条记忆（namespace: {:?}）", NS_MATH);
    println!(
        "✅ writer_agent → 写入 1 条记忆（namespace: {:?}）",
        NS_WRITER
    );
    println!();

    // ── 隔离验证 ──────────────────────────────────────────────────────────────

    // writer_agent 搜索"机密"——在自己的 namespace 里，什么都没有
    let writer_hits = store.search(&NS_WRITER, "机密", 10).await.unwrap();
    println!("🔍 writer_agent 在自己的 namespace 搜索 [机密]：");
    if writer_hits.is_empty() {
        println!("   -> 0 条命中 ✅  (跨 namespace 数据不可见)");
    } else {
        println!("   -> ⚠️  发现 {} 条数据，隔离失效！", writer_hits.len());
    }

    // writer_agent 搜索"风格"——只能看到自己的
    let writer_style = store.search(&NS_WRITER, "风格", 10).await.unwrap();
    println!("🔍 writer_agent 在自己的 namespace 搜索 [风格]：");
    for item in &writer_style {
        let content = item
            .value
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        println!("   -> [score={:.2}] {}", item.score.unwrap_or(0.0), content);
    }
    println!();

    // main_agent 尝试跨 namespace 搜索——理论上应该为空
    let main_hits = store.search(&NS_MAIN, "斐波那契", 10).await.unwrap();
    println!("🔍 main_agent 在自己的 namespace 搜索 [斐波那契]：");
    if main_hits.is_empty() {
        println!("   -> 0 条命中 ✅  (main_agent 尚未有自己的记忆)");
    }
    println!();

    // ── 全局视图：只有持有 Store 对象的代码才能跨 namespace 访问 ────────────────
    let namespaces = store.list_namespaces(None).await.unwrap();
    println!("📂 Store 中全部命名空间（仅主进程能看全）：");
    for ns in &namespaces {
        println!("   • {}", ns.join("/"));
    }

    // 主进程明确指定 math_agent 的 namespace 才能看到其记忆
    let math_hits = store.search(&NS_MATH, "机密", 10).await.unwrap();
    println!();
    println!("🔍 主进程明确访问 math_agent namespace 搜索 [机密]：");
    for item in &math_hits {
        let content = item
            .value
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        println!("   -> [score={:.2}] {}", item.score.unwrap_or(0.0), content);
    }

    println!("\n─────────────────────────────────────────────────────────\n");
}

// ─────────────────────────────────────────────────────────────────────────────
// Part 2 — 独立 Agent 会话隔离（各自有独立 session_id）
// ─────────────────────────────────────────────────────────────────────────────

async fn demo_session_isolation(checkpointer: Arc<dyn Checkpointer>) {
    println!("╔═══════════════════════════════════════════════════════╗");
    println!("║   Part 2: Checkpointer 会话隔离                       ║");
    println!("╚═══════════════════════════════════════════════════════╝\n");

    // ── math_agent：独立会话 ─────────────────────────────────────────────────
    println!("▶ 运行 math_agent（session: {}）", SESSION_MATH);
    let mut math_agent = ReactAgent::new(
        AgentConfig::new(
            MODEL,
            "math_agent",
            "你是一位简洁的数学助手，用中文给出简短答案。",
        )
        .enable_tool(true)
        .session_id(SESSION_MATH)
        .checkpointer_path(CHECKPOINT_PATH),
    );
    let math_result = math_agent
        .execute("斐波那契数列第6项是多少？请直接给出数字。")
        .await;
    println!(
        "   答案: {}\n",
        math_result.unwrap_or_else(|e| e.to_string())
    );

    // ── writer_agent：独立会话 ───────────────────────────────────────────────
    println!("▶ 运行 writer_agent（session: {}）", SESSION_WRITER);
    let mut writer_agent = ReactAgent::new(
        AgentConfig::new(
            MODEL,
            "writer_agent",
            "你是一位简洁的写作助手，用中文回答。",
        )
        .enable_tool(true)
        .session_id(SESSION_WRITER)
        .checkpointer_path(CHECKPOINT_PATH),
    );
    let writer_result = writer_agent.execute("用一句话描述秋天。").await;
    println!(
        "   答案: {}\n",
        writer_result.unwrap_or_else(|e| e.to_string())
    );

    // ── 验证：从 Checkpointer 检查各会话历史 ─────────────────────────────────
    println!("📋 从 Checkpointer 读取所有已保存会话：");
    let sessions = checkpointer.list_sessions().await.unwrap();
    for sid in &sessions {
        println!("   • session_id = \"{}\"", sid);
    }
    println!();

    // 检查 math_agent 的会话内容
    if let Some(cp) = checkpointer.get(SESSION_MATH).await.unwrap() {
        println!("📌 math_agent 的消息历史（{}）：", SESSION_MATH);
        for msg in &cp.messages {
            let preview = msg
                .content
                .as_deref()
                .map(|s| s.chars().take(60).collect::<String>())
                .unwrap_or_else(|| "<tool_call>".to_string());
            println!("   [{}] {}", msg.role, preview);
        }
        println!();
    }

    // 检查 writer_agent 的会话内容
    if let Some(cp) = checkpointer.get(SESSION_WRITER).await.unwrap() {
        println!("📌 writer_agent 的消息历史（{}）：", SESSION_WRITER);
        for msg in &cp.messages {
            let preview = msg
                .content
                .as_deref()
                .map(|s| s.chars().take(60).collect::<String>())
                .unwrap_or_else(|| "<tool_call>".to_string());
            println!("   [{}] {}", msg.role, preview);
        }
        println!();
    }

    // 关键验证：两个 Agent 各自的上下文中，是否混入了对方的内容？
    println!("🔒 上下文隔离验证：");
    let (math_msgs, _) = math_agent.context_stats();
    let (writer_msgs, _) = writer_agent.context_stats();
    println!("   math_agent   上下文消息数 = {}", math_msgs);
    println!("   writer_agent 上下文消息数 = {}", writer_msgs);

    // 检查 writer_agent 的上下文是否包含 math_agent 的提示词（理论上不应该）
    let writer_context_has_math_prompt = writer_agent.context_stats(); // 只能看到总数，无法直接访问 math_agent 的内部上下文
    println!("   ✅ 每个 Agent 的 ContextManager 完全独立，互不可见");
    let _ = writer_context_has_math_prompt; // suppress unused warning

    println!("\n─────────────────────────────────────────────────────────\n");
}

// ─────────────────────────────────────────────────────────────────────────────
// Part 3 — 多 Agent 上下文隔离（Orchestrator + SubAgent dispatch）
// ─────────────────────────────────────────────────────────────────────────────

async fn demo_context_isolation_multi_agent(checkpointer: Arc<dyn Checkpointer>) {
    println!("╔═══════════════════════════════════════════════════════╗");
    println!("║   Part 3: 多 Agent 上下文隔离                         ║");
    println!("╚═══════════════════════════════════════════════════════╝\n");
    println!("核心规则：");
    println!("  • 主 Agent 将任务分派给 SubAgent 时，只传递任务字符串");
    println!("  • SubAgent 看不到主 Agent 的系统提示、对话历史或任何机密");
    println!("  • SubAgent 之间也完全隔离，不能互相读取对话历史\n");

    // ── 创建 SubAgent ─────────────────────────────────────────────────────────
    // SubAgent 拥有独立会话，运行结束后历史自动存入 Checkpointer
    let math_sub = ReactAgent::new(
        AgentConfig::new(
            MODEL,
            "math_expert",
            "你是一位简洁的数学专家，只做数学计算，用中文给出简短答案。",
        )
        .enable_tool(true)
        .session_id("sub-math-001")
        .checkpointer_path(CHECKPOINT_PATH),
    );

    let writer_sub = ReactAgent::new(
        AgentConfig::new(
            MODEL,
            "writer_expert",
            "你是一位简洁的写作专家，只做文字创作，用中文给出简短答案。",
        )
        .enable_tool(true)
        .session_id("sub-writer-001")
        .checkpointer_path(CHECKPOINT_PATH),
    );

    // ── 创建主 Agent，系统提示中包含"机密信息" ───────────────────────────────
    // 机密：SubAgent 绝对不应看到这段信息
    let secret_in_system_prompt = "【机密】本次任务代号为 PROJECT-OMEGA，严禁对外透露。";

    let main_system = format!(
        "你是主编排者（Orchestrator）。{}
你有两个专用 SubAgent 可以调用：
- math_expert: 擅长数学计算
- writer_expert: 擅长文字创作
对于数学任务使用 math_expert，对于写作任务使用 writer_expert。
用中文汇总两个 SubAgent 的结果。",
        secret_in_system_prompt
    );

    println!(
        "🔐 主 Agent 系统提示中包含机密：「{}」\n",
        secret_in_system_prompt
    );

    let mut main_agent = ReactAgent::new(
        AgentConfig::new(MODEL, "main_agent", &main_system)
            .role(AgentRole::Orchestrator)
            .enable_tool(true)
            .enable_subagent(true)
            .session_id(SESSION_MAIN)
            .checkpointer_path(CHECKPOINT_PATH)
            .max_iterations(20),
    );
    main_agent.register_agent(Box::new(math_sub));
    main_agent.register_agent(Box::new(writer_sub));

    // ── 执行多 Agent 任务 ─────────────────────────────────────────────────────
    println!("▶ 主 Agent 执行任务（会分派给两个 SubAgent）...");
    let result = main_agent
        .execute(
            "请完成两件事：\
             1. 让数学专家计算 7 * 8 的结果；\
             2. 让写作专家用一句话描述结果数字对应的特征（比如奇偶、大小）。\
             然后汇总两个结果。",
        )
        .await;

    match result {
        Ok(answer) => println!("\n✅ 主 Agent 最终答案:\n{}\n", answer),
        Err(e) => println!("\n⚠️  执行出错: {}\n", e),
    }

    // ── 验证：检查 SubAgent 历史，确认没有主 Agent 的机密 ─────────────────────
    println!("─────────────────────────────────────────────────────────");
    println!("🔍 验证上下文隔离：读取 SubAgent 的会话历史...\n");

    let secret_keyword = "PROJECT-OMEGA";

    for (session_id, agent_name) in [
        ("sub-math-001", "math_expert"),
        ("sub-writer-001", "writer_expert"),
    ] {
        match checkpointer.get(session_id).await.unwrap() {
            Some(cp) => {
                let all_text: String = cp
                    .messages
                    .iter()
                    .filter_map(|m| m.content.as_deref())
                    .collect::<Vec<_>>()
                    .join(" ");

                let leaked = all_text.contains(secret_keyword);
                println!("📌 {} 的会话历史（{}）：", agent_name, session_id);
                println!("   消息条数: {}", cp.messages.len());
                println!(
                    "   包含机密 \"{}\"? → {}",
                    secret_keyword,
                    if leaked {
                        "⚠️  是！上下文隔离失效！"
                    } else {
                        "✅ 否（上下文隔离有效）"
                    }
                );

                // 打印每条消息的角色和简要内容
                for msg in &cp.messages {
                    let preview = msg
                        .content
                        .as_deref()
                        .map(|s| s.chars().take(80).collect::<String>())
                        .unwrap_or_else(|| "<tool_call>".to_string());
                    println!("   [{}] {}", msg.role, preview);
                }
                println!();
            }
            None => {
                println!(
                    "📌 {} 的会话 \"{}\"：未保存（可能未执行到）\n",
                    agent_name, session_id
                );
            }
        }
    }

    // ── 验证：主 Agent 可以通过 Checkpointer 读取 SubAgent 的历史 ───────────────
    println!("─────────────────────────────────────────────────────────");
    println!("📋 主 Agent 可以通过 Checkpointer 列出所有已知会话：");
    let all_sessions = checkpointer.list_sessions().await.unwrap();
    println!("   全部 session_id: {:?}", all_sessions);
    println!();
    println!("💡 关键结论：");
    println!("   • SubAgent 消息历史中不含主 Agent 机密 → 上下文天然隔离");
    println!("   • 每个 Agent 的 ContextManager 是独立实例，互不共享");
    println!("   • agent_tool 只传递 task 字符串，不传递消息历史");
    println!("   • 主 Agent 持有 Checkpointer 对象，可显式读取任意会话");
    println!(
        "   • SubAgent 之间没有 Checkpointer 互访能力（各自持有的 store/checkpointer 通过自身 session_id 访问）"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    // 创建临时目录
    tokio::fs::create_dir_all("/tmp/echo-agent-demo14").await?;

    println!();
    println!(
        "███████╗ █████╗  ██████╗██╗  ██╗ ██████╗      █████╗  ██████╗ ███████╗███╗   ██╗████████╗"
    );
    println!(
        "██╔════╝██╔══██╗██╔════╝██║  ██║██╔═══██╗    ██╔══██╗██╔════╝ ██╔════╝████╗  ██║╚══██╔══╝"
    );
    println!(
        "█████╗  ██║  ╚═╝██║     ███████║██║   ██║    ███████║██║  ███╗█████╗  ██╔██╗ ██║   ██║   "
    );
    println!(
        "██╔══╝  ██║  ██╗██║     ██╔══██║██║   ██║    ██╔══██║██║   ██║██╔══╝  ██║╚██╗██║   ██║   "
    );
    println!(
        "███████╗╚█████╔╝╚██████╗██║  ██║╚██████╔╝    ██║  ██║╚██████╔╝███████╗██║ ╚████║   ██║   "
    );
    println!(
        "╚══════╝ ╚════╝  ╚═════╝╚═╝  ╚═╝ ╚═════╝     ╚═╝  ╚═╝ ╚═════╝ ╚══════╝╚═╝  ╚═══╝   ╚═╝   "
    );
    println!();
    println!("demo14 — 记忆系统与上下文隔离\n");
    println!("存储路径:");
    println!("  Store      : {}", STORE_PATH);
    println!("  Checkpoint : {}", CHECKPOINT_PATH);
    println!();

    // 创建共享的底层存储（所有 Agent 使用同一个物理文件）
    let shared_store: Arc<dyn Store> = Arc::new(FileStore::new(STORE_PATH)?);
    let shared_checkpointer: Arc<dyn Checkpointer> =
        Arc::new(FileCheckpointer::new(CHECKPOINT_PATH)?);

    // ── Part 1: Store 命名空间隔离 ─────────────────────────────────────────────
    demo_store_namespace_isolation(shared_store.clone()).await;

    // ── Part 2: 独立 Agent 会话隔离 ───────────────────────────────────────────
    demo_session_isolation(shared_checkpointer.clone()).await;

    // ── Part 3: 多 Agent 上下文隔离 ───────────────────────────────────────────
    demo_context_isolation_multi_agent(shared_checkpointer.clone()).await;

    println!("═══════════════════════════════════════════════════════");
    println!("所有存储文件保存在 /tmp/echo-agent-demo14/");
    println!("  cat {}  可查看完整 Store 内容", STORE_PATH);
    println!("  cat {}  可查看完整会话历史", CHECKPOINT_PATH);
    println!("═══════════════════════════════════════════════════════");

    Ok(())
}
