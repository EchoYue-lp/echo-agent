# 追踪系统 — 执行轨迹与可观测性

## 概述

追踪系统将每次 Agent 执行记录为结构化的 `Run` 轨迹——捕获 LLM 调用、工具执行、阶段转换、错误和时间分解。追踪是可选启用的，并为评估和自进化流水线提供数据。

```
Agent.execute("任务")
  │
  ├── start_trace_run()     → Run { status: Running }
  ├── record_trace_event()  → LlmCall, ToolCall, ToolResult, PhaseTransition, ...
  ├── record_trace_event()  → ToolCall, ToolResult, FileEdit, ...
  └── finalize_trace_run()  → Run { status: Completed, final_output, timings }
                                   │
                                   ▼
                            RunStore（内存 / Jsonl）
                                   │
                          ┌────────┼────────┐
                          ▼                 ▼
                    EvalRunner         Analyzer
                    （回放）            （自进化）
```

---

## 核心类型

### Run

单次 Agent 执行的顶层记录：

```rust
pub struct Run {
    pub run_id: String,                    // 例如 "run_<uuid>"
    pub parent_run_id: Option<String>,     // 子 Agent 运行时设置
    pub session_id: String,                // 所属会话
    pub status: RunStatus,                 // Pending → Running → Completed/Failed/Cancelled
    pub input: String,                     // 触发此运行的用户输入
    pub events: Vec<RunEvent>,             // 按时间顺序的执行事件
    pub final_output: Option<String>,      // 最终输出文本（Completed 时设置）
    pub error: Option<String>,             // 错误消息（Failed 时设置）
    pub token_usage: TokenUsage,           // Token 分解
    pub timings: RunTimings,               // 时间分解
    pub started_at: DateTime<Utc>,         // 运行开始时间
    pub finished_at: Option<DateTime<Utc>>,// 运行结束时间
}
```

### RunStatus

```rust
pub enum RunStatus {
    Pending,    // 已创建但未开始
    Running,    // 执行中
    Completed,  // 成功完成
    Failed,     // 执行失败
    Cancelled,  // 被用户或系统取消
}
```

### TokenUsage

```rust
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}
```

### RunTimings

```rust
pub struct RunTimings {
    pub total_duration_ms: u64,   // 墙钟时间
    pub llm_duration_ms: u64,     // LLM 调用耗时
    pub tool_duration_ms: u64,    // 工具执行耗时
}
```

### RunSummary

用于列出运行的轻量摘要（不含完整事件历史）：

```rust
pub struct RunSummary {
    pub run_id: String,
    pub session_id: String,
    pub status: RunStatus,
    pub input_preview: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub token_usage: TokenUsage,
    pub total_duration_ms: u64,
}
```

---

## RunEvent — 11 种事件类型

`RunEvent` 是一个带标签的联合体，包含 11 个变体，每个捕获特定的执行时刻：

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    LlmCall { messages, prompt_tokens, completion_tokens, duration_ms },
    ToolCall { call_id, name, args, risk, duration_ms },
    ToolResult { call_id, name, success, output_preview, output_truncated, duration_ms },
    ToolError { call_id, name, message },
    Error { message },
    Checkpoint { id },
    PermissionDecision { tool, decision, reason },
    FileEdit { tool, path },
    TestRun { command, passed, failure_count },
    PhaseTransition { phase, iteration },
    SubAgentRun { agent_name, task, outcome },
}
```

### 各事件的触发时机

| 事件 | 阶段 | 说明 |
|------|------|------|
| `LlmCall` | 思考 | 每次 LLM API 调用后——记录 Token 数和延迟 |
| `ToolCall` | 执行 | 工具执行前——记录名称、参数（已脱敏）、风险等级 |
| `ToolResult` | 执行 | 工具成功后——记录成功标志、输出预览（前 200 字符） |
| `ToolError` | 执行 | 工具失败后——记录错误消息 |
| `Error` | 任意 | 运行级别错误 |
| `Checkpoint` | 任意 | 保存检查点时 |
| `PermissionDecision` | 执行 | 权限策略评估后——"allow"、"deny" 或 "ask" |
| `FileEdit` | 执行 | 写工具编辑文件后 |
| `TestRun` | 执行 | 测试命令运行后 |
| `PhaseTransition` | 循环 | 每个 ReAct 阶段："recall"、"think"、"act"、"finalize" |
| `SubAgentRun` | 调度 | 子 Agent 完成时——"completed"、"failed"、"cancelled" |

### 密钥脱敏

`RunEvent::new_tool_call()` 在构造事件前自动对工具参数调用 `redact_secrets()`。确保工具参数中的 API 密钥、密码和 Token 不会存储在轨迹中。

---

## RunStore — 持久化 Trait

```rust
#[async_trait]
pub trait RunStore: Send + Sync {
    async fn save(&self, run: Run) -> Result<()>;
    async fn load(&self, run_id: &str) -> Result<Option<Run>>;
    async fn list_by_session(&self, session_id: &str) -> Result<Vec<RunSummary>>;
    async fn list_all(&self, limit: usize) -> Result<Vec<RunSummary>>;

    // 默认实现：load → push event → save
    async fn append_event(&self, run_id: &str, event: RunEvent) -> Result<()>;
}
```

### 内置实现

| 实现 | 存储方式 | 使用场景 |
|------|---------|----------|
| `InMemoryRunStore` | `RwLock<HashMap>` | 测试、短期会话 |
| `JsonlRunStore` | 追加写入 `.jsonl` 文件 | 生产环境、持久化轨迹 |

#### InMemoryRunStore

基于 `RwLock<HashMap<String, Run>>`。额外辅助方法：`len()`、`is_empty()`。

```rust
let store = InMemoryRunStore::new();
```

#### JsonlRunStore

基于文件的持久化。每个运行存储为 `{dir}/{run_id}.jsonl`（追加写入；最后一行为当前状态）。构造时扫描已有文件填充内存缓存。

```rust
let store = JsonlRunStore::new(PathBuf::from("./traces"))?;
```

---

## Agent 集成

追踪系统是**可选启用的**。通过 Builder 接入：

```rust
use echo_agent::prelude::*;
use echo_agent::trace::JsonlRunStore;

let store = Arc::new(JsonlRunStore::new(PathBuf::from("./traces"))?);

let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("你是有帮助的助手")
    .with_run_store(store.clone())  // 启用追踪
    .build()?;
```

或通过 `AgentRunner`：

```rust
let runner = AgentRunner::new(agent)
    .with_run_store(store);
```

### 运行生命周期

```
1. start_trace_run(input)
   → 创建 Run { status: Running, run_id: "run_<uuid>" }
   → 保存到 store

2. record_trace_event(event)   （多次调用）
   → 通过 store.append_event() 将事件追加到 Run
   → 即发即忘（错误被静默丢弃）

3. finalize_trace_run(status, output, error)
   → 设置 status、final_output、finished_at
   → 保存最终状态到 store
   → 清除 current_run_id
```

### 事件触发位置

| 源文件 | 触发的事件 |
|--------|-----------|
| `react_loop.rs` | `LlmCall`、`PhaseTransition`、finalize |
| `execution.rs` | `PermissionDecision`、`ToolCall`、`ToolError`、`ToolResult`、`FileEdit` |
| `pipeline.rs` | `ToolCall`、`ToolResult`、`ToolError` |
| `stream_channel.rs` | `ToolCall`、`ToolError`、`ToolResult` |
| `approval.rs` | `PermissionDecision` |

---

## 下游消费者

追踪系统为两个子系统提供数据：

### 评估系统

评估运行器使用轨迹进行：
- **TrajectoryReplay**：离线分析工具使用模式、约束违规
- **RegressionSuite**：从过去成功的运行构建回归测试用例
- **指标**：从轨迹中提取 Token 用量、时间和工具调用次数

### 自进化流水线

改进系统使用轨迹进行：
- **Analyzer**：检测失败模式（先写后读、过度重试）
- **BackgroundReviewer**：从对话轨迹中提取记忆和技能信号
- **TrajectorySaver**：将轨迹转换为 ShareGPT 格式用于模型微调
- **ChangeLog**：记录记忆/技能/规则变更（自进化审计日志）

```
┌──────────┐     ┌──────────────┐     ┌─────────────────┐
│ RunStore │────▶│ TrajectoryReplay │──▶│ 评估报告         │
│ (轨迹)   │     └──────────────┘     └─────────────────┘
│          │
│          │     ┌──────────────┐     ┌─────────────────┐
│          │────▶│ Analyzer     │────▶│ ImprovementLoop │
│          │     └──────────────┘     └─────────────────┘
│          │
│          │     ┌──────────────┐     ┌─────────────────┐
│          │────▶│TrajectorySaver│───▶│ ShareGPT JSONL  │
└──────────┘     └──────────────┘     └─────────────────┘
```

---

## JSON 输出格式

每个轨迹事件序列化为带 `type` 鉴别器的 JSON：

```json
{
  "run_id": "run_abc123",
  "status": "completed",
  "input": "读取 src/main.rs",
  "events": [
    {
      "type": "phase_transition",
      "phase": "recall",
      "iteration": 0
    },
    {
      "type": "llm_call",
      "messages": 3,
      "prompt_tokens": 150,
      "completion_tokens": 45,
      "duration_ms": 320
    },
    {
      "type": "tool_call",
      "call_id": "call_1",
      "name": "read_file",
      "args": {"path": "src/main.rs"},
      "risk": null,
      "duration_ms": 5
    },
    {
      "type": "tool_result",
      "call_id": "call_1",
      "name": "read_file",
      "success": true,
      "output_preview": "fn main() { ...",
      "output_truncated": false,
      "duration_ms": 5
    },
    {
      "type": "phase_transition",
      "phase": "finalize",
      "iteration": 1
    }
  ],
  "token_usage": {
    "prompt_tokens": 150,
    "completion_tokens": 45,
    "total_tokens": 195
  },
  "timings": {
    "total_duration_ms": 850,
    "llm_duration_ms": 320,
    "tool_duration_ms": 5
  }
}
```

---

## Feature 开关

追踪系统**没有 Feature 开关**——始终编译和可用。所有类型通过 `prelude` 模块无条件重新导出。

下游消费者（`eval`、`improve`）有各自的 feature flag，但它们依赖的追踪基础设施始终存在。

```toml
[dependencies]
echo_agent = { version = "0.2" }                          # 追踪始终包含
echo_agent = { version = "0.2", features = ["eval"] }     # + 评估回放
echo_agent = { version = "0.2", features = ["improve"] }  # + 自进化分析
```
