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

## RunEvent — 17 种事件类型

`RunEvent` 是带有 snake-case `type` 鉴别器的联合体，目前包含 17 个变体。
`src/trace/mod.rs` 中的枚举是序列化合同；下面的矩阵是权威 producer 映射。
仅仅因为测试 fixture 可以构造某个变体，并不代表生产路径已经产生该事实。

### 权威 Producer 矩阵

| 变体 | 权威 producer | 结算规则 |
|------|---------------|----------|
| `BudgetDecision` | `src/agent/react/run/stream_channel.rs` | 只记录已发出的 wind-down 或 final-only 预算决策，并非每次运行都会产生。 |
| `LlmCall` | `src/agent/react/run/phases/think.rs` | provider 返回后追加，记录该调用已知的 usage 与耗时事实。 |
| `ContextCompression` | `src/agent/react/capabilities.rs`、`src/agent/react/run/phases/compact.rs` | 手动或自动压缩完成后追加。 |
| `ToolCall` | `src/agent/react/run/pipeline.rs`；`src/agent/snapshot.rs` 中的 synthetic unstarted call | 正常调用在 `ExecuteStage` 入口记录；synthetic call 仅用于关闭未进入该阶段的 invocation。 |
| `ToolExecutionSkipped` | `src/agent/snapshot.rs` | 与未进入 `ExecuteStage` 的 invocation 的 synthetic terminal 成对出现，不是从工具失败推断出来的。 |
| `ToolResult` | `src/agent/react/run/pipeline.rs`、`src/agent/snapshot.rs` | 记录真实结果，或为未启动 invocation 记录一次 synthetic failed result。 |
| `ToolError` | `src/agent/react/run/pipeline.rs`、`src/agent/snapshot.rs` | 记录失败结果或 interrupted synthetic terminal，并保留已知的 typed failure。 |
| `Error` | `src/trace/mod.rs::apply_run_finalization` | 仅当失败 run 尚无 run-level error event 时，由 run-store finalizer 补写。 |
| `Checkpoint` | `src/agent/snapshot.rs` | Runtime checkpoint compare-and-save 结算后追加。 |
| `CheckpointResumed` | `src/agent/react/mod.rs`（由 `src/agent/react/run/stream_channel.rs` 调用） | 执行继续前记录持久化 runtime checkpoint 的 hydration。 |
| `TranscriptProjectionSettlement` | `src/agent/snapshot.rs` | 记录 typed conversation projection 的结算或 reconciliation 结果。 |
| `PermissionDecision` | `src/agent/react/run/pipeline.rs`（`PermissionStage` 与 hook 路径） | 记录每次观察到的 hook、protected-path 或 permission 决定；它不是最终授权收据。 |
| `FileRead` | `src/agent/snapshot.rs::record_tool_effect` | 从确认的 `ToolEffect::FileRead` 投影实际解析路径；读取失败或猜测路径不产生。 |
| `FileEdit` | `src/agent/snapshot.rs::record_tool_effect` | 变更确认后从 `ToolEffect::FileEdit` 投影；dry-run、提案和泛化成功不产生。 |
| `TestRun` | `src/agent/snapshot.rs::record_tool_effect`；`src/eval/runner.rs::record_test_run` | 必须是已完成的测试命令 effect 或 evaluator 的显式 criterion；没有结构化计数时 `failure_count` 保持 `None`。 |
| `PhaseTransition` | `src/agent/react/run/react_loop.rs` | 由 run loop 记录 ReAct 阶段转换。 |
| `SubagentRun` | `src/agent/snapshot.rs::record_tool_effect`，由 `src/tools/builtin/agent_dispatch.rs` 提供 effect | 记录已结算的 Subagent 结果，包括失败和取消；launch acknowledgement 不是终态。 |

通用 shell 工具不会根据命令名、路径、输出文本或 exit code 推断
`FileEdit`/`TestRun`。只有工具自己确认的 `ToolEffect`，或 evaluator 明确完成的命令边界，
才能产生这些事实，避免 trace projection 变成第二套副作用权威。

后台 dispatch 也遵循同一边界：launch acknowledgement 不携带 `SubagentRun`；invocation-scoped
effect sink 在之后记录且只记录一个终态。Detached background owner 的持久 admission、代次
隔离、shutdown 取消以及 evidence settlement 仍由 embedding application 负责（对应 Issue
#38 与 #61 的 framework/consumer residual），详见 [ADR 0059](../adr/0059-observed-tool-effects-and-background-dispatch.md)。

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

    // 默认实现：load → 提交首个终态 → save
    async fn finalize_run(
        &self,
        run_id: &str,
        status: RunStatus,
        output: Option<&str>,
        error: Option<&str>,
    ) -> Result<bool>;
}
```

允许 event append 与 finalization 并发的 backend 必须在同一个 mutation authority
下覆盖这两个方法。run 不存在时 `finalize_run` 返回 `false`；内置 store 会保留晚到
event，并以第一个 terminal result 为准。

### 自定义 backend 的 retention 合同

React producer 在调用自定义 `save`、`append_event` 前会应用默认的
`ContentRetentionPolicy`；默认 `finalize_run` 实现会清洗终态输出和错误字段。覆盖
`save`、`append_event` 或 `finalize_run` 的 backend 必须在接纳持久写入前重复应用相同或
更严格的策略。`Run::apply_retention` 与 `RunEvent::apply_retention` 是可复用的边界
helper，独立的终态字符串使用 `ContentRetentionPolicy::sanitize_text`。

Retention 处理 prompt、输出、错误、tool 参数和人类可读原因等用户/模型/tool 内容。
`run_id`、session/turn/execution ID、call ID、tool 名称、路径、状态、计数器和时间戳等
typed 寻址与 effect 事实保持不变，以便诊断查询和回放；它们不是 secret 内容存储，调用方
不得把 credential 放入仍需要寻址的 identity 字段。

自定义 store 和 audit sink 在部分写入或持久性未知时必须返回错误。producer 保留已接纳
状态的可见性，并通过 diagnostic delivery 报告错误；backend 错误不得改写 Agent 执行终态。
字段分类与自定义 backend 责任见 [ADR 0074](../adr/0074-trace-audit-retention-contract.md)。

### 内置实现

| 实现 | 存储方式 | 使用场景 |
|------|---------|----------|
| `InMemoryRunStore` | `RwLock<HashMap>` | 测试、短期会话 |
| `JsonlRunStore` | snapshot 加 event line 的 `.jsonl` 文件 | 生产环境、持久化轨迹 |

#### InMemoryRunStore

基于 `RwLock<HashMap<String, Run>>`。额外辅助方法：`len()`、`is_empty()`。

```rust
let store = InMemoryRunStore::new();
```

#### JsonlRunStore

基于文件的持久化。每个运行存储为 `{dir}/{run_id}.jsonl`：第一行是压缩后的
`Run` snapshot，后续行是单个 `RunEvent`。追加 event 时增加一行；save 与
finalization 会原子压缩回一个当前 snapshot。构造时扫描已有文件填充内存缓存。

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

### 运行生命周期

```
1. start_trace_run(input)
   → 创建 Run { status: Running, run_id: "run_<uuid>" }
   → 保存到 store；拒绝写入时不发布 trace run ID

2. record_trace_event(event)   （多次调用）
   → 通过 store.append_event() 将事件追加到 Run
   → 报告被拒绝的投递，但不改变 Agent 执行结果

3. finalize_trace_run(status, output, error)
   → 设置 status、final_output、finished_at
   → 保存最终状态到 store
   → 不修改产品/业务 current_run_id
```

### 诊断投递失败

Trace 与 Audit 持久化是 Agent 执行的观测结果，不是第二个执行终态。它们的
Store/Logger 直接方法返回 `Result`，要求持久化的调用方必须处理该结果。Agent
集成中的可选诊断写入失败时，producer 继续执行，同时向 observer 发送结构化的
`DiagnosticDeliveryFailure`。

Agent producer 只通过有界、非阻塞的 `try_send` 提交 failure。进程内诊断 dispatcher
发送 tracing target `echo_agent::diagnostic_delivery`，包含稳定的 `record_kind`、
`operation`、`record_id_present`、`occurred_at` 与 `error` 字段。应用还可以安装一个结构化 observer；
它会从同一 dispatcher 收到 failure fact：

```rust
use echo_agent::audit::{DiagnosticDeliveryFailure, DiagnosticDeliveryObserver};
use echo_agent::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct DiagnosticCounter(AtomicUsize);

impl DiagnosticDeliveryObserver for DiagnosticCounter {
    fn on_failure(&self, _failure: DiagnosticDeliveryFailure) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

let diagnostic_failures = Arc::new(DiagnosticCounter::default());
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("你是有帮助的助手")
    .diagnostic_delivery_observer(diagnostic_failures.clone())
    .build()?;
# Ok::<(), echo_agent::error::ReactError>(())
```

observer 没有控制型返回值，也不在 Agent producer 上运行。队列饱和、断开、初始化
失败、重入上报与 observer unwind 都会增加 `diagnostic_delivery_dropped_count()`。
阻塞 observer 只会延迟后续诊断通知，不会延迟 Completed、Failed 或 Cancelled producer
结算；进程 abort/termination 不属于进程内恢复范围。技能使用指标仍是独立的
best-effort telemetry，不会被提升为 Trace/Audit 投递权威。失败策略与业界依据见
[ADR 0053](../adr/0053-trace-audit-persistence-visibility.md)。

### Producer 源文件索引

[权威 Producer 矩阵](#权威-producer-矩阵)是事件归属的唯一来源。主要 producer 模块为
`src/agent/react/run/react_loop.rs`、`src/agent/react/run/phases/think.rs`、
`src/agent/react/run/phases/compact.rs`、`src/agent/react/run/pipeline.rs`、
`src/agent/react/run/stream_channel.rs`、`src/agent/snapshot.rs`、
`src/eval/runner.rs` 和 `src/trace/mod.rs`。旧的泛化源文件索引已废弃；新增或复核
producer 时应以变体级矩阵为准。

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
echo_agent = { version = "0.2", features = ["improve"] }  # + 轨迹与生命周期辅助
echo_agent = { version = "0.2", features = ["improve", "eval"] } # + 评测驱动分析
```
