# 工具系统（Tools）

## 是什么

工具（Tool）是 Agent 与外部世界交互的唯一手段。LLM 通过 JSON Schema 了解工具的能力，决定何时调用、传入什么参数，框架负责实际执行并将结果返回给 LLM。

---

## 解决什么问题

LLM 本身是纯文本模型，不能直接：
- 执行代码或系统命令
- 查询实时数据（天气、股价、数据库）
- 读写文件
- 调用外部 API

工具系统提供了标准化的桥梁，让 LLM 能以"声明式调用"的方式驱动任意外部能力。

---

## 架构

```
Tool trait                       ← 所有工具实现的统一接口
    │
ToolManager                      ← 注册表 + 执行器
    ├─ register(tool)
    ├─ execute_tool(name, params) ← 统一执行入口（含超时、重试、并发限流）
    └─ to_openai_tools()          ← 序列化为 OpenAI function-calling 格式

内置工具（builtin）：
    ├─ final_answer              ← Agent 输出最终结果（必须注册）
    ├─ plan                      ← 触发规划模式（Planner 角色）
    ├─ create_task / update_task ← 管理 DAG 子任务
    ├─ agent_tool                ← 分派任务给 SubAgent（Orchestrator 角色）
    ├─ human_in_loop             ← 向人类请求文本输入
    ├─ remember / recall / forget ← 长期记忆操作
    └─ think                     ← CoT 显式思维工具（已被 CoT 文本方案替代）

扩展工具（开箱即用）：
    ├─ tools/files     ← 文件读写
    ├─ tools/shell     ← Shell 命令执行
    └─ tools/others    ← 数学计算、天气查询等示例工具
```

---

## 如何实现一个自定义工具

实现 `Tool` trait 即可：

```rust
use echo_agent::tools::{Tool, ToolParameters, ToolResult};
use echo_agent::error::Result;
use serde_json::{Value, json};
use async_trait::async_trait;

struct TranslateTool;

#[async_trait]
impl Tool for TranslateTool {
    fn name(&self) -> &str {
        "translate"
    }

    fn description(&self) -> &str {
        "将文本翻译为目标语言"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text":   { "type": "string", "description": "要翻译的文本" },
                "target": { "type": "string", "description": "目标语言，如 'en', 'zh', 'ja'" }
            },
            "required": ["text", "target"]
        })
    }

    async fn execute(&self, params: ToolParameters) -> Result<ToolResult> {
        let text   = params["text"].as_str().unwrap_or("");
        let target = params["target"].as_str().unwrap_or("en");
        // 调用实际翻译 API ...
        let result = format!("（已翻译到 {}）{}", target, text);
        Ok(ToolResult::success(result))
    }
}
```

---

## 注册与使用

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new("qwen3-max", "agent", "你是一个翻译助手")
    .enable_tool(true);

let mut agent = ReactAgent::new(config);
agent.add_tool(Box::new(TranslateTool));
// 或批量注册：agent.add_tools(vec![...]);

let answer = agent.execute("把'你好世界'翻译成英文").await?;
```

---

## 工具执行配置（超时 / 重试 / 并发）

`ToolExecutionConfig` 控制所有工具的执行行为：

```rust
use echo_agent::tools::ToolExecutionConfig;

let exec_config = ToolExecutionConfig {
    timeout_ms:      5_000,  // 单次超时 5 秒（0 = 不限制）
    retry_on_fail:   true,   // 失败自动重试
    max_retries:     2,      // 最多重试 2 次
    retry_delay_ms:  300,    // 首次重试延迟 300ms，指数退避
    max_concurrency: Some(3),// 并行工具调用最多 3 个同时执行
};

let config = AgentConfig::new("qwen3-max", "agent", "...")
    .tool_execution(exec_config);
```

**指数退避重试**：第 1 次重试延迟 300ms，第 2 次 600ms，第 3 次 1200ms...

---

## 限制特定工具

通过 `allowed_tools` 白名单，限制 Agent 只能使用指定工具，常用于 SubAgent 的能力边界控制：

```rust
use echo_agent::tools::others::math::{AddTool, SubtractTool};

let config = AgentConfig::new("qwen3-max", "math_only", "只做加减法")
    .allowed_tools(vec!["add".to_string(), "subtract".to_string()]);

let mut agent = ReactAgent::new(config);
// 即使注册了其他工具，只有 add 和 subtract 会实际暴露给 LLM
agent.add_tools(vec![
    Box::new(AddTool),
    Box::new(SubtractTool),
]);
```

---

## 内置工具列表

| 工具名 | 模块 | 说明 |
|--------|------|------|
| `final_answer` | builtin | 输出最终结果（自动注册） |
| `plan` | builtin | 触发任务规划（Planner 模式） |
| `create_task` | builtin | 创建 DAG 子任务 |
| `update_task` | builtin | 更新子任务状态 |
| `list_tasks` | builtin | 列出所有子任务 |
| `agent_tool` | builtin | 分派任务到 SubAgent |
| `human_in_loop` | builtin | 请求人类输入 |
| `remember` | builtin | 向 Store 写入记忆 |
| `recall` | builtin | 从 Store 检索记忆 |
| `forget` | builtin | 从 Store 删除记忆 |
| `read_file` | files | 读取文件内容 |
| `write_file` | files | 写入文件内容 |
| `shell` | shell | 执行 Shell 命令 |
| `add`/`subtract`/... | others | 数学运算（示例） |
| `get_weather` | others | 天气查询（示例） |

对应示例：`examples/demo01_tools.rs`、`examples/demo09_file_shell.rs`、`examples/demo13_tool_execution.rs`
