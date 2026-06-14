# 配置参考

## 概述

echo-agent 提供两种配置方式：

1. **Rust API** — `AgentConfig` + `ReactAgentBuilder`，编程式配置
2. **YAML 文件** — `echo-agent.yaml`，声明式配置

---

## AgentConfig — 运行时配置

核心运行时配置结构体。通过 `AgentConfig::new()` 构造，使用 builder 方法修改。

### 必填参数

```rust
let config = AgentConfig::new(model_name, agent_name, system_prompt);
```

| 参数 | 类型 | 说明 |
|------|------|------|
| `model_name` | `&str` | LLM 模型标识符（如 `"qwen3-max"`） |
| `agent_name` | `&str` | Agent 名称，用于日志和标识 |
| `system_prompt` | `&str` | 系统提示词，定义 Agent 角色和能力 |

### 预设构造器

| 预设 | 工具 | 记忆 | 任务 | CoT | 使用场景 |
|------|------|------|------|-----|----------|
| `AgentConfig::minimal(model, prompt)` | 关 | 关 | 关 | 关 | 简单 LLM 包装 |
| `AgentConfig::standard(model, name, prompt)` | 开 | 关 | 关 | 开 | 通用 Agent |
| `AgentConfig::full_featured(model, name, prompt)` | 开 | 开 | 开 | 开 | 全功能 Agent |

### 所有字段

#### 核心设置

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `model_name` | `String` | *（必填）* | LLM 模型标识符 |
| `agent_name` | `String` | *（必填）* | Agent 名称 |
| `system_prompt` | `String` | *（必填）* | 系统提示词 |
| `role` | `AgentRole` | `Worker` | `Orchestrator` 或 `Worker` |
| `max_iterations` | `usize` | `10` | 每轮最大推理步数 |
| `temperature` | `Option<f32>` | `None`（模型默认） | LLM 温度（0.0–2.0） |
| `max_tokens` | `Option<u32>` | `None`（模型默认） | 最大生成 Token 数 |

#### 功能开关

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enable_tool` | `bool` | `false` | 启用工具调用 |
| `enable_task` | `bool` | `false` | 启用任务规划工具 |
| `enable_human_in_loop` | `bool` | `false` | 启用人工审批 |
| `enable_subagent` | `bool` | `false` | 启用子 Agent 调度 |
| `enable_memory` | `bool` | `false` | 启用长期记忆工具 |
| `enable_cot` | `bool` | `false` | 启用链式思考提示 |

#### 工具设置

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `allowed_tools` | `Vec<String>` | `[]`（全部允许） | 工具白名单 |
| `tool_error_feedback` | `bool` | `true` | 将工具错误反馈给 LLM |
| `force_read_before_edit` | `bool` | `false` | 写入/编辑/删除前必须先读取 |
| `plan_mode` | `bool` | `false` | 仅使用只读工具 |
| `max_tool_output_tokens` | `Option<usize>` | `None` | 自动截断超过限制的工具输出 |
| `tool_execution` | `ToolExecutionConfig` | *（见下文）* | 工具执行设置 |

#### 记忆与持久化

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `memory_path` | `String` | `"~/.echo-agent/store.json"` | 记忆存储文件路径 |
| `session_id` | `Option<String>` | `None` | 进程内 run-grouping 标签（不持久化） |
| `conversation_id` | `Option<String>` | `None` | 对话 ID —— 同时作为 `ConversationStore` 与 `RuntimeStateStore` 的键 |

#### 上下文与压缩

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `token_limit` | `usize` | `usize::MAX` | 上下文 Token 限制 |
| `compress_threshold_ratio` | `f64` | `0.2` | 可用比例低于此值时触发压缩 |
| `response_format` | `Option<ResponseFormat>` | `None`（文本） | 结构化输出格式 |
| `auto_project_rules` | `bool` | `true` | 自动加载 `.echo-agent/AGENT.md` |
| `working_dir` | `Option<PathBuf>` | `None`（当前目录） | 项目规则文件的工作目录 |

#### LLM 容错

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `llm_max_retries` | `usize` | `3` | LLM 失败后最大重试次数 |
| `llm_retry_delay_ms` | `u64` | `500` | 初始重试延迟（指数退避） |

#### 流式与回调

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `stream_buffer_size` | `usize` | `256` | 流式通道缓冲区大小 |
| `callbacks` | `Vec<Arc<dyn AgentCallback>>` | `[]` | 事件回调 |

#### 高级设置

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `_reasoning_effort` | `String` | `"medium"` | 推理强度：low/medium/high |
| `token_budget_config` | `TokenBudgetConfig` | *（见下文）* | 上下文窗口预算 |

### Builder 方法

所有字段都可通过 builder 链式调用设置：

```rust
let config = AgentConfig::new("qwen3-max", "assistant", "你是有帮助的助手")
    .enable_tool()
    .enable_memory()
    .enable_cot()
    .max_iterations(20)
    .token_limit(100_000)
    .temperature(0.7)
    .tool_error_feedback(true)
    .force_read_before_edit(true)
    .build();
```

---

## ReactAgentBuilder — 高级构建器

高级构建器，用于构造 `ReactAgent`。处理 LLM 客户端注入、工具注册、记忆设置、护栏、快照等。

### 基本用法

```rust
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .name("my-agent")
    .system_prompt("你是代码助手")
    .enable_tools()
    .enable_memory()
    .enable_cot()
    .build()?;
```

### 预设

| 预设 | 说明 |
|------|------|
| `ReactAgentBuilder::simple(model, prompt)` | 无工具，最小配置 |
| `ReactAgentBuilder::standard(model, name, prompt)` | 启用工具 |
| `ReactAgentBuilder::full_featured(model, name, prompt)` | 工具 + 记忆 + 规划 |

### LLM 配置

```rust
// 选项 1：从模型名自动创建
ReactAgentBuilder::new()
    .model("qwen3-max")

// 选项 2：显式 LLM 客户端
ReactAgentBuilder::new()
    .llm_client(my_client)

// 选项 3：带配置
ReactAgentBuilder::new()
    .llm_config(LlmConfig { base_url, api_key, model })
```

### 工具配置

```rust
ReactAgentBuilder::new()
    .enable_tools()                    // 启用工具调用
    .tool(Box::new(MyTool::new()))     // 注册单个工具
    .tools(vec![...])                  // 注册多个工具
```

### 功能开关

```rust
ReactAgentBuilder::new()
    .enable_memory()                   // 长期记忆
    .enable_planning()                 // DAG 任务规划
    .enable_human_in_loop()            // 审批守卫（需要 "human-loop" feature）
    .enable_subagent()                 // 子 Agent 调度（需要 "subagent" feature）
    .enable_cot()                      // 链式思考
    .disable_cot()                     // 禁用 CoT
```

### 结构化输出

```rust
// 从 Rust 类型自动生成 JSON Schema
ReactAgentBuilder::new()
    .output_type::<MyResponse>()

// 显式格式
ReactAgentBuilder::new()
    .response_format(ResponseFormat::JsonSchema {
        json_schema: JsonSchemaSpec { name, schema, strict }
    })
```

### 护栏与权限

```rust
ReactAgentBuilder::new()
    .guard(my_guard)                              // 添加输入/输出护栏
    .guards(vec![guard1, guard2])                 // 添加多个护栏
    .with_content_guard(ContentGuardMode::Block)  // PII 检测（需要 "content-guard" feature）
    .permission_service(my_service)               // 统一权限服务（推荐）
    .audit_logger(my_logger)                      // 审计日志
```

### 持久化与追踪

```rust
ReactAgentBuilder::new()
    .store(memory_store)                          // 长期记忆存储
    .with_memory_tools(store)                     // 注册 remember/recall/forget 工具
    .state_store(state_store)                     // RuntimeStateStore 用于崩溃恢复
    .session_id("sess_1")                         // 会话标签
    .conversation_id("conv_1")                    // 对话记录 ID
    .with_run_store(run_store)                    // 执行追踪
```

### 快照与熔断器

```rust
ReactAgentBuilder::new()
    .snapshot_policy(SnapshotPolicy::EveryN(5))   // 快照频率
    .max_snapshots(20)                            // 最大保留快照数
    .with_circuit_breaker(CircuitBreakerConfig {  // LLM 故障保护
        failure_threshold: 5,
        success_threshold: 2,
        timeout: Duration::from_secs(60),
    })
```

### 构建

```rust
// 构建为 ReactAgent
let agent: ReactAgent = builder.build()?;

// 构建为 boxed trait 对象
let agent: Box<dyn Agent> = builder.build_boxed()?;
```

### 验证规则

- `model` 不能为空
- `max_iterations` 必须 > 0
- `enable_subagent` 要求 `enable_builtin_tools` 为 true

---

## ToolExecutionConfig

控制单个工具的执行行为：

```rust
pub struct ToolExecutionConfig {
    pub timeout_ms: u64,             // 默认：30_000（30 秒）
    pub retry_on_fail: bool,         // 默认：false
    pub max_retries: u32,            // 默认：2
    pub retry_delay_ms: u64,         // 默认：200
    pub max_concurrency: Option<usize>,       // 默认：None
    pub max_read_concurrency: Option<usize>,  // 默认：Some(32)
}
```

---

## TokenBudgetConfig

细粒度上下文窗口预算管理：

```rust
pub struct TokenBudgetConfig {
    pub total_window: Option<usize>,  // 默认：None（从模型自动检测）
    pub system_pct: f64,              // 默认：0.10（10%）
    pub tool_pct: f64,                // 默认：0.05（5%）
    pub output_pct: f64,              // 默认：0.10（10%）
    pub safety_pct: f64,              // 默认：0.10（10%）
    pub enabled: bool,                // 默认：true
}
```

使用默认值时，对话历史获得上下文窗口的 **65%**。

自动检测的模型窗口大小：
- `claude` → 200K
- `gpt-5.5` → 128K
- `qwen3` → 128K
- 默认 → 128K

---

## ResponseFormat

```rust
pub enum ResponseFormat {
    Text,                                          // 纯文本（默认）
    JsonObject,                                    // JSON 对象
    JsonSchema { json_schema: JsonSchemaSpec },    // JSON Schema 约束
}

pub struct JsonSchemaSpec {
    pub name: String,
    pub schema: serde_json::Value,
    pub strict: bool,  // 默认：true
}
```

---

## CircuitBreakerConfig

防止 LLM 连续故障：

```rust
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,     // 默认：5（连续失败 → Open）
    pub success_threshold: u32,     // 默认：2（连续成功 → Closed）
    pub timeout: Duration,          // 默认：60 秒（Open 持续时间后转 HalfOpen）
}
```

---

## SnapshotPolicy

控制 Agent 状态快照时机：

```rust
pub enum SnapshotPolicy {
    EveryIteration,  // 每次 ReAct 迭代后快照（默认）
    EveryN(usize),   // 每 N 次迭代快照
    Manual,          // 仅在显式调用时
}
```

---

## YAML 配置

echo-agent 支持通过 `echo-agent.yaml` 进行声明式配置。

### 文件搜索顺序

1. `$ECHO_AGENT_CONFIG` 环境变量
2. `./echo-agent.yaml`（当前目录）
3. `~/.echo-agent/config.yaml`（用户主目录）
4. 内置默认值

### 完整示例

```yaml
model:
  name: "qwen3.6-plus"
  temperature: 0.7
  max_tokens: 4096

agent:
  name: "echo-assistant"
  system_prompt: "你是一个智能助手"
  max_iterations: 10
  enable_tools: true
  enable_memory: true
  enable_human_in_loop: true
  memory_path: "~/.echo-agent/memory"
  tool_timeout_ms: 120000
  token_limit: 0
  compress_strategy: "sliding"
  compress_window: 20

mcp:
  config_path: "~/.echo-agent/mcp.yaml"

channels:
  feishu:
    enabled: false
    app_id: ""
    app_secret: ""
    mode: "long_poll"

server:
  host: "0.0.0.0"
  port: 3000
  max_body_bytes: 1048576

logging:
  level: "info"
```

### YAML 段落

| 段落 | 结构体 | 说明 |
|------|--------|------|
| `model` | `ModelConfig` | LLM 模型名称、温度、最大 Token 数 |
| `agent` | `AgentYamlConfig` | Agent 行为开关和路径 |
| `mcp` | `McpYamlConfig` | MCP 配置文件路径 |
| `channels` | `ChannelsConfig` | IM 渠道集成（QQ、飞书） |
| `webhooks` | `WebhooksConfig` | Webhook 端点 |
| `hooks` | `HooksDefinition` | 生命周期钩子规则 |
| `server` | `ServerConfig` | HTTP 服务器主机/端口 |
| `logging` | `LoggingConfig` | 日志级别 |

### 环境变量覆盖

| 环境变量 | 效果 |
|---------|------|
| `ECHO_AGENT_CONFIG` | 显式配置文件路径 |
| `QQ_APP_ID` | 设置 QQ 渠道 app_id，自动启用 QQ |
| `QQ_CLIENT_SECRET` | 设置 QQ 渠道 client_secret |
| `FEISHU_APP_ID` | 设置飞书渠道 app_id，自动启用飞书 |
| `FEISHU_APP_SECRET` | 设置飞书渠道 app_secret |
| `MCP_CONFIG_PATH` | 设置 MCP 配置文件路径 |

---

## Feature Flags

所有功能都是可选启用的。`default` feature 集为**空**。`full` 元功能启用所有功能。

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["mcp", "web", "shell"] }
```

### 核心功能

| Feature | 说明 |
|---------|------|
| `subagent` | 子 Agent 调度和 TeamAgent |
| `mcp` | Model Context Protocol 集成 |
| `tasks` | 任务规划和 DAG 调度 |
| `self-reflection` | 自我反思/评估循环 |
| `human-loop` | 基于 WebSocket 的人工审批 |
| `plan-execute` | 规划执行 Agent 模式 |

### 工具功能

| Feature | 说明 |
|---------|------|
| `web` | 网页搜索/获取工具 |
| `shell` | Shell 命令执行 |
| `files` | 文件操作工具 |
| `git` | Git 工具 |
| `database` | SQL 数据库工具 |
| `media` | PDF/Excel/Word/图片工具 |
| `data` | Polars 数据处理工具 |
| `chart` | 图表生成工具 |
| `research` | ArXiv、Semantic Scholar、PDF 工具 |
| `sandbox` | 沙箱脚本执行 |

### 基础设施功能

| Feature | 说明 |
|---------|------|
| `sqlite` | SQLite 状态存储 |
| `telemetry` | OpenTelemetry 追踪 + 指标 |
| `a2a` | Agent-to-Agent HTTP 服务 |
| `channels` | IM 渠道集成（QQ、飞书） |
| `rag` | 检索增强生成 |
| `semantic-memory` | 基于嵌入的语义记忆 |
| `workflow` | 工作流 DSL 引擎 |
| `multimodal` | 多模态输入支持 |
| `content-guard` | PII 检测/脱敏 |
| `eval` | 评估框架 |
| `improve` | 自进化框架 |
| `testing` | 测试工具 |

---

## TelemetryConfig

OpenTelemetry 配置：

```rust
pub struct TelemetryConfig {
    pub otlp_endpoint: String,    // 默认："http://localhost:4317"
    pub service_name: String,     // 默认："echo-agent"
    pub enable_console: bool,     // 默认：true
}
```

需要 `telemetry` feature flag。

---

## 快速参考

### 最小 Agent

```rust
let config = AgentConfig::minimal("qwen3-max", "说你好");
let agent = ReactAgent::new(config);
```

### 带工具的标准 Agent

```rust
let config = AgentConfig::standard("qwen3-max", "assistant", "你是有帮助的助手");
let mut agent = ReactAgent::new(config);
agent.add_tool(Box::new(MyTool::new()));
```

### 通过 Builder 构建全功能 Agent

```rust
let agent = ReactAgentBuilder::full_featured("qwen3-max", "assistant", "你是有帮助的助手")
    .tool(Box::new(FileTool::new()))
    .tool(Box::new(ShellTool::new()))
    .with_run_store(run_store)
    .guard(my_guard)
    .build()?;
```

### 基于 YAML 的配置

```rust
let config = AppConfig::load()?;  // 加载 echo-agent.yaml
```

---

## 模型窗口注册表（v0.2.1 新增）

动态注册和查询模型的上下文窗口大小：

```rust
use echo_core::budget::{register_model_window, resolve_model_window};

// 注册自定义模型的窗口大小
register_model_window("my-custom-model", 128_000);

// 查询窗口大小（未知模型回退到启发式估计）
let window = resolve_model_window("qwen3-max");  // 从注册表查
let fallback = resolve_model_window("unknown-model");  // 启发式估计
```

内置模型已有默认窗口大小注册。可通过 `register_model_window()` 覆盖或扩展。

---

## 全局事件总线（EventBus）

统一的 `tokio::broadcast` 事件通道，供 Webhook / Trace / UI / Audit 订阅同一事件流：

```rust
use echo_agent::event_bus::{GLOBAL_EVENT_BUS, BusEvent};

// 订阅事件
let mut rx = GLOBAL_EVENT_BUS.subscribe();

// 发送事件
GLOBAL_EVENT_BUS.send(AgentEvent::Token("hello".into()));

// 发送带 run 上下文的事件
GLOBAL_EVENT_BUS.send_for_run(event, "run-123");

// 接收事件
while let Ok(bus_event) = rx.recv().await {
    println!("Event: {:?}, run_id: {:?}", bus_event.event, bus_event.run_id);
}
```

`BusEvent` 包含 `run_id` 和 `agent_id`，支持多 Agent 场景下的事件过滤。

容量 1024，消费端落后时收到 `RecvError::Lagged`。
