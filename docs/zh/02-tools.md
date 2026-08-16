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
    ├─ task_create / task_update ← 版本化任务图 CRUD
    ├─ task_list                 ← 读取已提交任务图版本
    ├─ agent_tool                ← 分派任务给已注册 Subagent
    ├─ human_in_loop             ← 向人类请求文本输入
    ├─ remember / recall / forget ← 长期记忆操作
    └─ think                     ← CoT 显式思维工具（已被 CoT 文本方案替代）

扩展工具（开箱即用）：
    ├─ tools/files       ← 文件读写（2 个工具）
    ├─ tools/shell       ← Shell 命令执行
    ├─ tools/web         ← Web 搜索 + 网页获取（feature: web）
    ├─ tools/media       ← PDF、Excel、Word、图片（feature: media）
    ├─ tools/data        ← Polars 数据分析（13 个工具，feature: data）
    ├─ tools/chart       ← 图表生成（feature: chart）
    ├─ tools/rag         ← RAG 索引/搜索/分块（feature: rag）
    ├─ tools/research    ← ArXiv、Semantic Scholar、PDF 下载、BibTeX（feature: research）
    ├─ tools/database    ← SQL 数据库工具（feature: database）
    └─ tools/others      ← 数学计算、天气查询等示例工具

总计：26 个功能分类，67 个已注册工具。
```

---

## Tool trait 完整定义

> **v0.2.0 起 trait 签名已变更。** 不再使用 `#[async_trait]`，`execute` 返回 `BoxFuture`。

```rust
pub trait Tool: Send + Sync {
    /// 工具名称（暴露给 LLM 的稳定标识符）
    fn name(&self) -> &str;

    /// 工具描述（人类可读）
    fn description(&self) -> &str;

    /// JSON Schema 参数定义
    fn parameters(&self) -> serde_json::Value;

    /// 执行工具（核心方法，必须实现）
    fn execute<'a>(
        &'a self,
        parameters: ToolParameters,
    ) -> BoxFuture<'a, Result<ToolResult>>;

    // ── 以下为可选方法，均有默认实现 ──

    /// 流式执行，产生增量 ToolStreamEvent
    fn execute_stream<'a>(
        &'a self,
        params: ToolParameters,
    ) -> BoxFuture<'a, Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>>;

    /// 是否支持流式执行（默认 false）
    fn supports_streaming(&self) -> bool;

    /// 执行前参数校验（默认 Ok(())）
    fn validate_parameters<'a>(
        &'a self,
        params: &'a ToolParameters,
    ) -> BoxFuture<'a, Result<()>>;

    /// 所需权限列表（默认空 = 无需权限）
    fn permissions(&self) -> Vec<ToolPermission>;

    /// 风险级别（默认 Standard）
    fn risk_level(&self) -> ToolRiskLevel;

    /// 人类可读的能力描述（默认根据 risk_level 生成）
    fn capability_description(&self) -> &str;
}
```

### 方法说明

| 方法 | 必须实现 | 默认值 | 用途 |
|------|---------|--------|------|
| `name()` | ✅ | — | 工具标识符 |
| `description()` | ✅ | — | LLM 看到的工具描述 |
| `parameters()` | ✅ | — | JSON Schema 参数定义 |
| `execute()` | ✅ | — | 核心执行逻辑 |
| `execute_stream()` | ❌ | 包装 `execute()` 为单个 `Complete` 事件 | 流式进度输出 |
| `supports_streaming()` | ❌ | `false` | 声明是否支持流式 |
| `validate_parameters()` | ❌ | `Ok(())` | 执行前参数校验 |
| `permissions()` | ❌ | `vec![]` | 声明所需权限 |
| `risk_level()` | ❌ | `Standard` | 风险分类 |
| `capability_description()` | ❌ | 根据 risk_level 生成 | 人类可读能力描述 |

---

## ToolResult 与 ToolResultKind

### ToolResult

```rust
pub struct ToolResult {
    pub kind: ToolResultKind,        // 结果类型分类
    pub success: bool,               // 是否成功
    pub output: String,              // 文本输出
    pub error: Option<String>,       // 错误信息
    pub bytes: Option<Vec<u8>>,      // 二进制输出
    pub data: Option<Value>,         // 结构化 JSON 数据
    pub truncated: bool,             // 是否被截断
    pub mime_type: Option<String>,   // MIME 类型
    pub metadata: HashMap<String, String>, // 元数据
}
```

**构造函数：**

| 方法 | 用途 |
|------|------|
| `ToolResult::success(output)` | 成功文本结果 |
| `ToolResult::success_json(data)` | 成功 JSON 结果 |
| `ToolResult::success_with_kind(kind, output)` | 带类型分类的成功结果 |
| `ToolResult::error(msg)` | 失败结果 |
| `ToolResult::binary(bytes)` | 二进制输出 |

**链式构造器：**

```rust
ToolResult::success("output")
    .with_meta("file_path", "/tmp/result.csv")
    .with_mime_type("text/csv")
    .with_truncated(true)
```

### ToolResultKind

```rust
pub enum ToolResultKind {
    Text,                                    // 纯文本
    Json,                                    // 结构化 JSON
    Image { mime_type: String },             // 图片
    Table { columns: Vec<String>, rows: Vec<Vec<String>> }, // 表格
    Diff { unified_diff: String },           // unified diff
    FileReference { path: String },          // 文件引用
    CommandOutput { exit_code: Option<i32> }, // 命令输出
    StructuredError { error_code: String },  // 结构化错误
}
```

下游消费者（CLI 渲染、trace 分析、eval 打分）可根据 `kind` 做差异化处理，无需解析 `output` 字符串。

---

## ToolStreamEvent（流式工具事件）

```rust
pub enum ToolStreamEvent {
    /// 进度通知
    Progress { message: String, percent: Option<u8> },
    /// 增量输出片段
    PartialOutput { chunk: String },
    /// 终止事件，携带最终 ToolResult（流在此事件后结束）
    Complete(ToolResult),
}
```

实现流式工具：

```rust
impl Tool for LongRunningTool {
    // ... name / description / parameters ...

    fn supports_streaming(&self) -> bool { true }

    fn execute_stream<'a>(
        &'a self,
        params: ToolParameters,
    ) -> BoxFuture<'a, Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>> {
        Box::pin(async move {
            let stream = async_stream::stream! {
                yield ToolStreamEvent::Progress {
                    message: "Starting...".into(),
                    percent: Some(0),
                };
                // ... 中间步骤 ...
                yield ToolStreamEvent::PartialOutput {
                    chunk: "partial result...".into(),
                };
                yield ToolStreamEvent::Progress {
                    message: "Done".into(),
                    percent: Some(100),
                };
                yield ToolStreamEvent::Complete(
                    ToolResult::success("final result")
                );
            };
            Ok(Box::pin(stream))
        })
    }

    fn execute<'a>(
        &'a self,
        params: ToolParameters,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        // 非流式回退
        Box::pin(async move { Ok(ToolResult::success("final result")) })
    }
}
```

---

## ToolRiskLevel（风险级别）

```rust
pub enum ToolRiskLevel {
    ReadOnly,   // 只读操作，无副作用
    Standard,   // 标准操作，有限副作用
    Dangerous,  // 危险操作，不可逆副作用
}
```

工具通过 `risk_level()` 声明风险：

```rust
impl Tool for DeleteFileTool {
    fn risk_level(&self) -> ToolRiskLevel { ToolRiskLevel::Dangerous }
    fn capability_description(&self) -> &str { "删除文件 — 不可逆操作" }
    // ...
}
```

风险分类器 `ToolRiskClassifier`（在 `echo-execution` 中）会自动根据工具名称细分为 7 类风险：

| 类别 | 风险等级 | 示例工具 |
|------|---------|---------|
| `ReadOnly` | 0 | `read_file`、`grep`、`git_status` |
| `NetworkCall` | 1 | `web_fetch`、`web_search` |
| `FileWrite` | 2 | `edit_file`、`write_file` |
| `GitWrite` | 2 | `git_commit`、`git_push` |
| `DatabaseWrite` | 2 | `db_execute`、`sql` |
| `ShellExec` | 3 | `shell`、`execute` |
| `Destructive` | 3 | `delete_file`、`drop_table` |

详见 [安全与权限](./security.md) 了解完整的权限模型、规则引擎和风险分类。

---

## ToolCallParams（类型安全参数）

从 LLM 传来的原始 JSON 参数可以通过 `ToolCallParams` 做类型安全的提取和校验：

```rust
use echo_agent::tools::{ToolCallParams, ParamValue};

let params = ToolCallParams::from_value(&raw_json);

// 类型安全提取
let path: Option<&str> = params.get_str("path");
let count: Option<f64> = params.get_number("count");
let force: Option<bool> = params.get_bool("force");

// 必填参数校验
params.validate_required("path", "string")?;
```

---

## 如何实现一个自定义工具

实现 `Tool` trait（注意：不再使用 `#[async_trait]`，`execute` 返回 `BoxFuture`）：

```rust
use echo_agent::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};
use echo_agent::tools::permission::ToolPermission;
use echo_agent::error::Result;
use echo_agent::tools::ToolCallParams;
use serde_json::{Value, json};
use futures::future::BoxFuture;

struct TranslateTool;

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

    fn execute<'a>(
        &'a self,
        params: ToolParameters,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let typed = ToolCallParams::from_params(&params);
            let text = typed.get_str("text").unwrap_or("");
            let target = typed.get_str("target").unwrap_or("en");
            // 调用实际翻译 API ...
            let result = format!("（已翻译到 {}）{}", target, text);
            Ok(ToolResult::success(result))
        })
    }

    // ── 可选：声明权限和风险 ──

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]  // 需要网络访问
    }

    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly  // 只读操作
    }

    // ── 可选：参数校验 ──

    fn validate_parameters<'a>(
        &'a self,
        params: &'a ToolParameters,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let typed = ToolCallParams::from_params(params);
            typed.validate_required("text", "string")
                .map_err(|e| echo_agent::error::ReactError::Other(e))?;
            typed.validate_required("target", "string")
                .map_err(|e| echo_agent::error::ReactError::Other(e))?;
            Ok(())
        })
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

通过 `allowed_tools` 白名单，限制 Agent 只能使用指定工具，常用于 Subagent 的能力边界控制：

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
| `task_create` | builtin | 原子创建任务图或追加任务 |
| `task_update` | builtin | 应用乐观并发任务图 patch |
| `task_list` | builtin | 读取已提交任务图版本 |
| `agent_tool` | builtin | 分派任务到 Subagent |
| `human_in_loop` | builtin | 请求人类输入 |
| `remember` | builtin | 向 Store 写入记忆 |
| `recall` | builtin | 从 Store 检索记忆 |
| `forget` | builtin | 从 Store 删除记忆 |
| `read_file` | files | 读取文件内容 |
| `write_file` | files | 写入文件内容 |
| `shell` | shell | 执行 Shell 命令 |
| `add`/`subtract`/... | others | 数学运算（示例） |
| `get_weather` | others | 天气查询（示例） |
| `web_search` | web | Web 搜索（需 `web` feature） |
| `web_fetch` | web | 获取网页内容并转文本（需 `web` feature） |
| `arxiv_search` | research | 搜索 ArXiv 学术论文（需 `research` feature） |
| `semantic_scholar_search` | research | 搜索 Semantic Scholar（需 `research` feature） |
| `pdf_fetch` | research | 从 URL 下载并解析 PDF（需 `research` feature） |
| `bibtex_generate` | research | 从论文元数据生成 BibTeX（需 `research` feature） |
| `rag_index` | rag | 文档分块和向量索引（需 `rag` feature） |
| `rag_search` | rag | 索引文档的语义搜索（需 `rag` feature） |
| `excel_read` / `excel_write` / ... | media | Excel 读写/分析（6 个工具，需 `media` feature） |
| `data_read` / `data_filter` / ... | data | Polars 数据分析（13 个工具，需 `data` feature） |
| `generate_chart` | chart | 图表生成（需 `chart` feature） |
| `db_query` / `db_schema` | database | SQL 数据库工具（需 `database` feature） |

对应示例：`examples/demo01_tools.rs`、`examples/demo09_file_shell.rs`、`examples/demo13_tool_execution.rs`、`examples/demo64_tool_pipeline.rs`

---

## ToolChoice 枚举（v0.2.1 新增）

控制 LLM 如何调用工具的类型安全枚举：

| 变体 | 含义 | OpenAI 格式 |
|------|------|------------|
| `Auto` | 模型自行决定是否调用工具（默认） | `"auto"` |
| `None` | 禁止调用任何工具 | `"none"` |
| `Required` | 必须调用至少一个工具 | `"required"` |
| `Function { name }` | 强制调用指定工具 | `{"type":"function","function":{"name":"..."}}` |

```rust
use echo_agent::llm::ToolChoice;

// 让模型自行决定
let choice = ToolChoice::Auto;

// 强制调用特定工具
let choice = ToolChoice::function("web_search");

// 禁止工具调用
let choice = ToolChoice::None;
```

---

## 工具执行管线（ToolExecutionPipeline）

> **新增于 v0.2.0。** 可配置的多阶段工具执行流水线。

工具调用不再直接执行，而是经过一条可插拔的管线处理。每个阶段可以检查、修改、拦截或增强工具执行行为。

### 管线阶段

```
Tool Call → InterventionStage → ParseValidate → PlanMode → PreToolUseHook
           → Permission → ReadBeforeEdit → Callback(Start) → Execution
           → TraceRecording → PostToolUseHook → OutputGuard → Truncation
           → Callback(End)
```

| 阶段 | 作用 |
|------|------|
| **InterventionStage** | 干预回调：block / cancel / redirect / modify_args |
| **ParseValidate** | 参数解析与类型校验 |
| **PlanMode** | 在计划模式下拦截写操作工具 |
| **PreToolUseHook** | PreToolUse 钩子：可修改输入或阻止执行 |
| **Permission** | 权限检查（PermissionService 统一管线） |
| **ReadBeforeEdit** | 编辑前强制先读取文件（防止盲写） |
| **Callback(Start)** | on_tool_start 回调 |
| **Execution** | 实际执行工具 |
| **TraceRecording** | 记录 Trace 事件 |
| **PostToolUseHook** | PostToolUse 钩子 |
| **OutputGuard** | 输出内容守卫检查 |
| **Truncation** | 输出截断（token 预算） |
| **Callback(End)** | on_tool_end 回调 |

### 配置管线

```rust
use echo_agent::agent::react::run::pipeline::ToolExecutionPipeline;
use echo_agent::prelude::*;

let pipeline = ToolExecutionPipeline::default();

let agent = ReactAgentBuilder::new()
    .tool_execution_pipeline(pipeline)
    .build(config);
```

详见 [demo64_tool_pipeline.rs](../../examples/demo64_tool_pipeline.rs)。
