//! demo35_dynamic_tools —— 动态 Tool 注册 / 注销 / 替换
//!
//! 演示在 ReAct 循环中根据阶段动态调整可用工具集：
//! - `remove_tool(name)` — 移除不再需要的工具
//! - `replace_tool(tool)` — 替换为同名的新版本工具
//! - `add_tool(tool)` — 动态添加新工具
//!
//! ```bash
//! cargo run --example demo35_dynamic_tools
//! ```

use echo_agent::prelude::*;

fn main() {
    println!("═══ Dynamic Tool Registration Demo ═══\n");

    let config = AgentConfig::minimal("qwen3-max", "phase_agent");
    let mut agent = echo_agent::agent::react_agent::ReactAgent::new(config);

    // ── Phase 0: 初始状态 ─────────────────────────────────────────────────
    println!("[Phase 0] 初始工具集：{:?}", agent.tool_names());
    println!("    → 只有 final_answer（内置）\n");

    // ── Phase 1: 研究阶段 — 添加搜索工具 ──────────────────────────────────
    agent.add_tool(Box::new(MockTool::new("search_web")));
    agent.add_tool(Box::new(MockTool::new("read_document")));
    println!("[Phase 1] 研究阶段 — 添加搜索工具");
    println!("    tools: {:?}\n", agent.tool_names());

    // ── Phase 2: 编码阶段 — 移除搜索工具，添加执行工具 ────────────────────
    let removed_search = agent.remove_tool("search_web");
    assert!(removed_search.is_some(), "search_web 应存在");
    let removed_read = agent.remove_tool("read_document");
    assert!(removed_read.is_some(), "read_document 应存在");

    agent.add_tool(Box::new(MockTool::new("execute_code")));
    agent.add_tool(Box::new(MockTool::new("write_file")));
    println!("[Phase 2] 编码阶段 — 替换为执行工具");
    println!("    removed: search_web, read_document");
    println!("    added:   execute_code, write_file");
    println!("    tools: {:?}\n", agent.tool_names());

    assert!(!agent.tool_names().contains(&"search_web"));
    assert!(agent.tool_names().contains(&"execute_code"));

    // ── Phase 3: 替换工具 — 用安全版本替换 execute_code ──────────────────
    let old = agent.replace_tool(Box::new(MockTool::new("execute_code")));
    assert!(old.is_some(), "应返回旧版 execute_code");
    println!("[Phase 3] 替换 execute_code → 安全版本");
    println!("    replace_tool 返回旧工具: {}", old.is_some());
    println!("    tools: {:?}\n", agent.tool_names());

    // ── Phase 4: 部署阶段 — 再次调整 ────────────────────────────────────
    agent.remove_tool("write_file");
    agent.add_tool(Box::new(MockTool::new("deploy_service")));
    agent.add_tool(Box::new(MockTool::new("health_check")));
    println!("[Phase 4] 部署阶段 — 最终工具集");
    println!("    tools: {:?}\n", agent.tool_names());

    // ── Edge case: 移除不存在的工具 ───────────────────────────────────────
    let not_found = agent.remove_tool("nonexistent_tool");
    assert!(not_found.is_none());
    println!("[Edge] remove_tool('nonexistent_tool') = None ✓");

    // ── Edge case: replace_tool 新工具不存在时直接注册 ────────────────────
    let old = agent.replace_tool(Box::new(MockTool::new("brand_new_tool")));
    assert!(old.is_none());
    assert!(agent.tool_names().contains(&"brand_new_tool"));
    println!("[Edge] replace_tool 不存在时自动注册新工具 ✓");

    println!("\n═══ Demo Complete ═══");
}
