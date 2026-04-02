//! demo32_token_budget —— Token 预算管控
//!
//! 演示两项核心能力：
//! 1. `max_tool_output_tokens` — 工具输出超限自动截断
//! 2. `compress_threshold_ratio` — 可用 token 不足时主动触发压缩
//!
//! ```bash
//! cargo run --example demo32_token_budget
//! ```

use echo_agent::prelude::*;

fn main() -> echo_agent::error::Result<()> {
    println!("═══ Token Budget Control Demo ═══\n");

    // ── 1. 配置 max_tool_output_tokens ─────────────────────────────────────
    println!("[1] AgentConfig 链式配置");
    let config = AgentConfig::new("qwen3-max", "budget_agent", "你是助手")
        .max_tool_output_tokens(2000)
        .compress_threshold_ratio(0.3);

    println!(
        "    max_tool_output_tokens  = {:?}",
        config.get_max_tool_output_tokens()
    );
    println!(
        "    compress_threshold_ratio = {}",
        config.get_compress_threshold_ratio()
    );

    // ── 2. Builder 方式设置 ────────────────────────────────────────────────
    println!("\n[2] ReactAgentBuilder 链式配置");
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("budget_agent")
        .max_tool_output_tokens(1500)
        .build()?;

    println!(
        "    max_tool_output_tokens = {:?}",
        agent.config().get_max_tool_output_tokens()
    );

    // ── 3. 默认值 ─────────────────────────────────────────────────────────
    println!("\n[3] 默认值检查");
    let default_config = AgentConfig::new("model", "agent", "prompt");
    assert_eq!(default_config.get_max_tool_output_tokens(), None);
    println!("    max_tool_output_tokens 默认 = None（不限制）");

    let ratio = default_config.get_compress_threshold_ratio();
    assert!((ratio - 0.2).abs() < f64::EPSILON);
    println!("    compress_threshold_ratio 默认 = {ratio}（剩余 20% 时触发压缩）");

    // ── 4. 截断行为说明 ───────────────────────────────────────────────────
    println!("\n[4] 截断机制说明");
    println!("    当 Agent 执行工具并获取结果后，内部流程为：");
    println!("    ① tokenizer.count_tokens(output) 统计输出 token 数");
    println!("    ② 若超过 max_tool_output_tokens，按比例截断");
    println!("    ③ 在末尾追加 [输出已截断，共 N tokens，保留前 M tokens]");
    println!("    ④ LLM 看到截断提示后可以决定是否需要更精确的信息");

    // ── 5. 压缩阈值说明 ───────────────────────────────────────────────────
    println!("\n[5] 压缩触发机制");
    println!("    token_limit = 10000, compress_threshold_ratio = 0.2");
    println!("    → 当已用 token ≥ 10000 × (1 - 0.2) = 8000 时");
    println!("    → think() 前主动触发上下文压缩，避免溢出");

    // ── 6. clamp 验证 ────────────────────────────────────────────────────
    println!("\n[6] compress_threshold_ratio 自动 clamp 到 [0.0, 1.0]");
    let clamped = AgentConfig::new("m", "a", "p").compress_threshold_ratio(2.0);
    assert!((clamped.get_compress_threshold_ratio() - 1.0).abs() < f64::EPSILON);
    println!(
        "    输入 2.0 → clamp 到 {}",
        clamped.get_compress_threshold_ratio()
    );

    let clamped = AgentConfig::new("m", "a", "p").compress_threshold_ratio(-0.5);
    assert!((clamped.get_compress_threshold_ratio() - 0.0).abs() < f64::EPSILON);
    println!(
        "    输入 -0.5 → clamp 到 {}",
        clamped.get_compress_threshold_ratio()
    );

    println!("\n═══ Demo Complete ═══");
    Ok(())
}
