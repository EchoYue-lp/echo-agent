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
//! cargo test --all-features --locked contract_demo30_mcp_server -- --nocapture
//! ```

use echo_agent::error::Result;
use echo_agent::mcp::McpServer;
use echo_agent::tools::{Tool, ToolParameters, ToolResult};
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

#[tokio::test]
async fn contract_demo30_mcp_server() -> Result<()> {
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

        let resp = send(&server, &req).await?;
        if *version == "2099-01-01" {
            let error_code =
                required_value(&resp, "/error/code", "未知协议版本未返回 JSON-RPC 错误码")?
                    .as_i64()
                    .ok_or_else(|| {
                        echo_agent::error::ReactError::Other(
                            "demo30 验收失败：未知协议版本错误码不是整数".to_string(),
                        )
                    })?;
            let error_message =
                required_str(&resp, "/error/message", "未知协议版本未返回错误消息")?;
            if error_code != -32600 || !error_message.contains("不支持的协议版本") {
                return Err(echo_agent::error::ReactError::Other(format!(
                    "demo30 验收失败：未知协议版本错误契约不正确: code={error_code}, message={error_message}"
                )));
            }
            println!("  请求 {version:>12}  →  error {error_code} ({error_message})");
            continue;
        }

        let negotiated = required_str(
            &resp,
            "/result/protocolVersion",
            "initialize 响应缺少 protocolVersion",
        )?;
        if negotiated != *version {
            return Err(echo_agent::error::ReactError::Other(format!(
                "demo30 验收失败：已支持协议版本未原样协商: requested={version}, negotiated={negotiated}"
            )));
        }
        println!("  请求 {version:>12}  →  回复 {negotiated:<12}  (echo)");
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
    .await?;

    let tools = required_array(&resp, "/result/tools", "tools/list 未返回 tools 数组")?;
    println!("  发现 {} 个工具:", tools.len());
    for t in tools {
        let name = required_str(t, "/name", "tools/list 工具缺少 name")?;
        let description = required_str(t, "/description", "tools/list 工具缺少 description")?;
        println!("    - {:15} : {}", name, description);
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
    .await?;
    let weather_text = required_str(
        &resp,
        "/result/content/0/text",
        "get_weather 响应缺少文本内容",
    )?;
    if weather_text.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo30 验收失败：get_weather 返回空文本".to_string(),
        ));
    }
    println!("  get_weather(Beijing) → {weather_text}");

    let resp = send(
        &server,
        &json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "calculate", "arguments": { "a": 42, "op": "*", "b": 3 } }
        }),
    )
    .await?;
    let calculation_text = required_str(
        &resp,
        "/result/content/0/text",
        "calculate 响应缺少文本内容",
    )?;
    if calculation_text != "126" {
        return Err(echo_agent::error::ReactError::Other(
            "demo30 验收失败：calculate(42 * 3) 返回值不正确".to_string(),
        ));
    }
    println!("  calculate(42 * 3)    → {calculation_text}");

    let resp = send(
        &server,
        &json!({
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": { "name": "calculate", "arguments": { "a": 10, "op": "/", "b": 0 } }
        }),
    )
    .await?;
    let is_err = required_bool(&resp, "/result/isError", "除零响应缺少布尔字段 isError")?;
    if !is_err {
        return Err(echo_agent::error::ReactError::Other(
            "demo30 验收失败：calculate(10 / 0) 未标记 isError".to_string(),
        ));
    }
    let division_text = required_str(&resp, "/result/content/0/text", "除零响应缺少文本内容")?;
    println!("  calculate(10 / 0)    → {division_text} (isError={is_err})");

    // ── Part 5: ping + 错误处理 ──────────────────────────────────────────────
    println!("\n--- Part 5: ping + 错误处理 ---\n");

    let resp = send(
        &server,
        &json!({ "jsonrpc": "2.0", "id": 6, "method": "ping" }),
    )
    .await?;
    if resp.get("error").is_some() {
        return Err(echo_agent::error::ReactError::Other(
            "demo30 验收失败：ping 返回 error".to_string(),
        ));
    }
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
    .await?;
    let error_message = required_str(&resp, "/error/message", "调用不存在工具时未返回错误消息")?;
    if error_message.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo30 验收失败：调用不存在工具时未返回错误消息".to_string(),
        ));
    }
    println!("  tools/call(nonexistent) → error: {error_message}");

    let resp = send(
        &server,
        &json!({
            "jsonrpc": "2.0", "id": 8, "method": "foo/bar"
        }),
    )
    .await?;
    let error_code = required_value(&resp, "/error/code", "未知方法未返回错误码")?;
    if !error_code.is_number() {
        return Err(echo_agent::error::ReactError::Other(
            "demo30 验收失败：未知方法未返回错误码".to_string(),
        ));
    }
    println!("  foo/bar → error code: {} (Method not found)", error_code);

    let resp_str = server.handle_json_rpc("not valid json").await;
    let resp: Value = serde_json::from_str(&resp_str)?;
    let parse_error_code = required_value(&resp, "/error/code", "非法 JSON 未返回 parse error")?;
    if !parse_error_code.is_number() {
        return Err(echo_agent::error::ReactError::Other(
            "demo30 验收失败：非法 JSON 未返回 parse error".to_string(),
        ));
    }
    println!(
        "  invalid JSON → error code: {} (Parse error)",
        parse_error_code
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
    .await?;
    let count = required_array(&resp, "/result/tools", "from_tools() 响应缺少 tools 数组")?.len();
    if count != 2 {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo30 验收失败：from_tools() 构建后的工具数错误: {count}"
        )));
    }
    println!("  from_tools() 构建的 quick-server → {count} 个工具");

    println!("\n═══════════════════════════════════════════════════════");
    println!("  demo30 完成");
    println!("═══════════════════════════════════════════════════════");

    // 取消注释下面的代码即可作为真实 stdio MCP Server 运行：
    // server.serve_stdio().await?;

    Ok(())
}

async fn send(server: &McpServer, req: &Value) -> Result<Value> {
    let resp_str = server.handle_json_rpc(&req.to_string()).await;
    let resp = serde_json::from_str(&resp_str)?;
    Ok(resp)
}

fn required_value<'a>(value: &'a Value, pointer: &str, context: &str) -> Result<&'a Value> {
    value
        .pointer(pointer)
        .ok_or_else(|| echo_agent::error::ReactError::Other(format!("demo30 验收失败：{context}")))
}

fn required_str<'a>(value: &'a Value, pointer: &str, context: &str) -> Result<&'a str> {
    required_value(value, pointer, context)?
        .as_str()
        .ok_or_else(|| {
            echo_agent::error::ReactError::Other(format!(
                "demo30 验收失败：{context}，字段不是字符串"
            ))
        })
}

fn required_bool(value: &Value, pointer: &str, context: &str) -> Result<bool> {
    required_value(value, pointer, context)?
        .as_bool()
        .ok_or_else(|| {
            echo_agent::error::ReactError::Other(format!(
                "demo30 验收失败：{context}，字段不是布尔值"
            ))
        })
}

fn required_array<'a>(value: &'a Value, pointer: &str, context: &str) -> Result<&'a Vec<Value>> {
    required_value(value, pointer, context)?
        .as_array()
        .ok_or_else(|| {
            echo_agent::error::ReactError::Other(format!(
                "demo30 验收失败：{context}，字段不是数组"
            ))
        })
}
