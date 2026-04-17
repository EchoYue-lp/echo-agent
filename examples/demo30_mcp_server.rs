//! demo30_mcp_server.rs —— MCP Server 演示
//!
//! 演示如何将 echo-agent 的 Tool 暴露为 MCP 服务端，
//! 让外部 MCP 客户端（Claude Desktop、Cursor 等）发现和调用工具。
//!
//! # 新特性展示
//!
//! - `McpServer::builder()` / `McpServer::from_tools()` 构建器
//! - MCP 2025-11-25 协议 + 向下兼容版本协商
//! - `handle_json_rpc()` 公共 API（用于自定义传输层或测试）
//! - `tools/list`、`tools/call`、`ping` 完整流程
//!
//! ```bash
//! cargo run --example demo30_mcp_server
//! ```

use echo_agent::mcp::McpServer;
use echo_core::error::Result;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::Arc;

// ── 示例工具 ────────────────────────────────────────────────────────────────

struct WeatherTool;

impl Tool for WeatherTool {
    fn name(&self) -> &str {
        "get_weather"
    }
    fn description(&self) -> &str {
        "Get current weather for a city"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "city": { "type": "string", "description": "City name" }
            },
            "required": ["city"]
        })
    }
    fn execute(&self, params: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let city = params
                .get("city")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Ok(ToolResult::success(format!("{city}: 25°C, partly cloudy")))
        })
    }
}

struct CalculatorTool;

impl Tool for CalculatorTool {
    fn name(&self) -> &str {
        "calculate"
    }
    fn description(&self) -> &str {
        "Evaluate a math expression (a op b)"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "a": { "type": "number" },
                "op": { "type": "string", "enum": ["+", "-", "*", "/"] },
                "b": { "type": "number" }
            },
            "required": ["a", "op", "b"]
        })
    }
    fn execute(&self, params: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let a = params.get("a").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let b = params.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let op = params.get("op").and_then(|v| v.as_str()).unwrap_or("+");
            let result = match op {
                "+" => a + b,
                "-" => a - b,
                "*" => a * b,
                "/" if b != 0.0 => a / b,
                "/" => return Ok(ToolResult::error("Division by zero")),
                _ => return Ok(ToolResult::error(format!("Unknown operator: {op}"))),
            };
            Ok(ToolResult::success(format!("{result}")))
        })
    }
}

// ── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("         Echo Agent × MCP Server 演示 (demo30)");
    println!("═══════════════════════════════════════════════════════\n");

    // ── Part 1: 构建 MCP Server ─────────────────────────────────────────────
    println!("--- Part 1: McpServer::builder() ---\n");

    let server = McpServer::builder()
        .name("echo-demo-server")
        .version("1.0.0")
        .instructions("Demo MCP server with weather and calculator tools.")
        .tool(Arc::new(WeatherTool))
        .tool(Arc::new(CalculatorTool))
        .build();

    println!("  Server: echo-demo-server v1.0.0");
    println!("  Tools:  get_weather, calculate\n");

    // ── Part 2: Initialize 握手 + 版本协商 ───────────────────────────────────
    println!("--- Part 2: Initialize + 版本协商 (2025-11-25) ---\n");

    let test_versions = ["2025-11-25", "2025-03-26", "2024-11-05", "2099-01-01"];
    for version in &test_versions {
        let req = json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": version,
                "capabilities": {},
                "clientInfo": { "name": "demo-client", "version": "1.0" }
            }
        });

        let resp = send(&server, &req).await;
        let negotiated = resp["result"]["protocolVersion"].as_str().unwrap_or("?");
        let tag = if negotiated == *version {
            "echo"
        } else {
            "fallback→latest"
        };
        println!("  请求 {version:>12}  →  回复 {negotiated:<12}  ({tag})");
    }

    // initialized 通知
    let notif = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    server.handle_json_rpc(&notif.to_string()).await;
    println!("\n  notifications/initialized sent");

    // ── Part 3: tools/list ───────────────────────────────────────────────────
    println!("\n--- Part 3: tools/list ---\n");

    let resp = send(
        &server,
        &json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}
        }),
    )
    .await;

    let tools = resp["result"]["tools"].as_array().unwrap();
    println!("  发现 {} 个工具:", tools.len());
    for t in tools {
        println!(
            "    - {:15} : {}",
            t["name"].as_str().unwrap_or("?"),
            t["description"].as_str().unwrap_or("")
        );
    }

    // ── Part 4: tools/call ───────────────────────────────────────────────────
    println!("\n--- Part 4: tools/call ---\n");

    let resp = send(
        &server,
        &json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "get_weather", "arguments": { "city": "Beijing" } }
        }),
    )
    .await;
    println!(
        "  get_weather(Beijing) → {}",
        resp["result"]["content"][0]["text"].as_str().unwrap_or("?")
    );

    let resp = send(
        &server,
        &json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "calculate", "arguments": { "a": 42, "op": "*", "b": 3 } }
        }),
    )
    .await;
    println!(
        "  calculate(42 * 3)    → {}",
        resp["result"]["content"][0]["text"].as_str().unwrap_or("?")
    );

    let resp = send(
        &server,
        &json!({
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": { "name": "calculate", "arguments": { "a": 10, "op": "/", "b": 0 } }
        }),
    )
    .await;
    let is_err = resp["result"]["isError"].as_bool().unwrap_or(false);
    println!(
        "  calculate(10 / 0)    → {} (isError={is_err})",
        resp["result"]["content"][0]["text"].as_str().unwrap_or("?")
    );

    // ── Part 5: ping + 错误处理 ──────────────────────────────────────────────
    println!("\n--- Part 5: ping + 错误处理 ---\n");

    let resp = send(
        &server,
        &json!({ "jsonrpc": "2.0", "id": 6, "method": "ping" }),
    )
    .await;
    println!(
        "  ping → {}",
        if resp.get("error").is_none() {
            "ok"
        } else {
            "error"
        }
    );

    let resp = send(
        &server,
        &json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "nonexistent", "arguments": {} }
        }),
    )
    .await;
    println!(
        "  tools/call(nonexistent) → error: {}",
        resp["error"]["message"].as_str().unwrap_or("?")
    );

    let resp = send(
        &server,
        &json!({
            "jsonrpc": "2.0", "id": 8, "method": "foo/bar"
        }),
    )
    .await;
    println!(
        "  foo/bar → error code: {} (Method not found)",
        resp["error"]["code"]
    );

    let resp_str = server.handle_json_rpc("not valid json").await;
    let resp: Value = serde_json::from_str(&resp_str).unwrap_or_default();
    println!(
        "  invalid JSON → error code: {} (Parse error)",
        resp["error"]["code"]
    );

    // ── Part 6: from_tools 便捷构建 ──────────────────────────────────────────
    println!("\n--- Part 6: McpServer::from_tools() ---\n");

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(WeatherTool), Arc::new(CalculatorTool)];
    let server2 = McpServer::from_tools(tools)
        .name("quick-server")
        .version("0.2.0")
        .build();

    let resp = send(
        &server2,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }),
    )
    .await;
    let count = resp["result"]["tools"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    println!("  from_tools() 构建的 quick-server → {count} 个工具");

    println!("\n═══════════════════════════════════════════════════════");
    println!("  demo30 完成");
    println!("═══════════════════════════════════════════════════════");

    // 取消注释下面的代码即可作为真实 stdio MCP Server 运行：
    // server.serve_stdio().await?;

    Ok(())
}

async fn send(server: &McpServer, req: &Value) -> Value {
    let resp_str = server.handle_json_rpc(&req.to_string()).await;
    serde_json::from_str(&resp_str).unwrap_or(json!({}))
}
