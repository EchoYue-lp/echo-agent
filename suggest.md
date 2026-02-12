# Agent 框架项目建议

## 一、整体架构评价

项目结构合理，核心模块划分清晰：`agent` / `tools` / `llm` / `tasks` / `human_loop`。Trait-based 设计（`Agent`、`Tool`）为扩展留了好的接口。ReAct 循环 + Planning 模式 + 人工审批三个核心流程都已实现。

---

## 二、需要修复的问题

### 1. `Tool::execute` 是同步的，限制了异步工具的实现

当前 `Tool` trait 的 `execute` 方法是同步的：

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;
    fn execute(&self, parameters: ToolParameters) -> Result<ToolResult>;
}
```

但真实场景中工具经常需要做网络请求（如天气查询、搜索引擎调用），建议将 `execute` 改为 `async`：

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;
    async fn execute(&self, parameters: ToolParameters) -> Result<ToolResult>;
}
```

### 2. `execute_tool` 中同步阻塞读 stdin 会阻塞 tokio 运行时

`std::io::stdin().read_line()` 是同步阻塞调用，在 tokio 运行时中会阻塞整个线程。应该使用 `tokio::io::AsyncBufReadExt` 或用 `tokio::task::spawn_blocking` 包装。

### 3. `unwrap()` 使用过多，生产代码中需要妥善处理

多处使用了 `.unwrap()`，比如 `RwLock::read().unwrap()` 在 lock poisoning 情况下会 panic。建议统一转换为 `Result` 错误处理，或者使用 `parking_lot::RwLock`（不会 poison）。

### 4. `execute_loop` 中缺少终止条件

`FinalAnswer` 的 `break` 只跳出了 `for` 循环，外层 `loop` 永远不会退出。需要用标签 `break` 或返回值来终止外层循环。

### 5. `get_next_task` 的排序方向可能反了

升序排列后 `first()` 取到的是优先级**最低**的任务。如果 10 代表最高优先级，应该用 `b.priority.cmp(&a.priority)` 降序排列。

---

## 三、架构级改进建议

### 1. 引入 Builder 模式构建 Agent

当前 `ReactAgent::new()` 内部硬编码注册了多个内置工具（`FinalAnswerTool`、`ThinkTool`、`HumanInLoop`、`PlanTool` 等），用户无法选择性启用/禁用。建议：

```rust
let agent = ReactAgentBuilder::new("my-agent", "high")
    .system_prompt("你是一个助手")
    .with_planning(true)        // 可选启用 planning
    .with_human_loop(true)      // 可选启用人工审批
    .add_tool(Box::new(WeatherTool))
    .add_danger_tool(Box::new(DeleteFileTool))
    .max_iterations(20)
    .build();
```

### 2. Tool trait 增加元数据支持

建议为 Tool 增加更丰富的元数据：

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;

    /// 工具是否默认标记为危险
    fn is_dangerous(&self) -> bool { false }

    /// 工具执行的预估耗时
    fn timeout(&self) -> Duration { Duration::from_secs(30) }

    /// 工具的分类标签
    fn tags(&self) -> Vec<&str> { vec![] }

    async fn execute(&self, parameters: ToolParameters) -> Result<ToolResult>;
}
```

### 3. 将 HumanApproval 做成可插拔的策略

当前审批逻辑硬编码为 stdin 读取。实际场景可能是 WebSocket 推送、HTTP 回调、Slack 审批等。建议抽象为 trait：

```rust
#[async_trait]
pub trait ApprovalStrategy: Send + Sync {
    async fn request_approval(&self, tool_name: &str, params: &Value) -> ApprovalResult;
}

// 默认实现：控制台交互
pub struct ConsoleApproval;

// 用户可实现自己的：
pub struct WebhookApproval { url: String }
pub struct SlackApproval { channel: String }
```

### 4. 增加中间件/钩子机制

在工具执行前后增加钩子，方便做日志、监控、限流等：

```rust
#[async_trait]
pub trait Middleware: Send + Sync {
    /// 工具执行前
    async fn before_tool(&self, tool_name: &str, params: &Value) -> Result<()>;
    /// 工具执行后
    async fn after_tool(&self, tool_name: &str, result: &ToolResult) -> Result<()>;
    /// LLM 调用前
    async fn before_llm(&self, messages: &[Message]) -> Result<()>;
    /// LLM 调用后
    async fn after_llm(&self, response: &ChatCompletionResponse) -> Result<()>;
}
```

### 5. 支持 Streaming 输出

当前 `chat` 函数虽有 `stream` 参数但并未实现流式处理。对于用户体验来说这非常重要，建议使用 `tokio::sync::mpsc` 或 `futures::Stream` 实现。

### 6. Memory/上下文压缩

已预留了 `compression` 模块。建议优先实现，因为长对话会导致 token 暴涨。常见策略：

- **滑动窗口**：只保留最近 N 轮对话
- **摘要压缩**：用 LLM 对历史对话生成摘要
- **重要性评分**：对每条消息打分，只保留高分消息

---

## 四、代码质量改进

### 1. 使用 `thiserror` 简化错误定义

当前手动实现了大量 `Display`、`From` trait，推荐使用 `thiserror` crate：

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReactError {
    #[error("LLM Error: {0}")]
    Llm(#[from] LlmError),
    #[error("Tool Error: {0}")]
    Tool(#[from] ToolError),
    // ...
}
```

能减少约一半的 `error.rs` 代码量。

### 2. 使用 `tracing` 替代 `println!`

当前大量使用 `println!` 进行日志输出，建议引入 `tracing` crate：

```rust
use tracing::{info, debug, warn};

// 替代 println!
info!(tool = %tool_name, "调用工具");
debug!(params = %arguments, "工具参数");
warn!("即将执行危险操作: {}", tool_name);
```

好处：结构化日志、可配置级别、支持多种输出格式（JSON/pretty）、可集成到外部监控系统。

### 3. `client.rs` 中遗留了调试打印

```rust
println!("=============>\n {:?} \n================", completion_response);
```

这行应该用 `tracing::debug!` 替代或删除。

### 4. 每次请求都创建新的 `reqwest::Client`

`reqwest::Client` 内部维护连接池，应复用。建议在 `Agent` 或全局持有一个 `Client` 实例。

### 5. 项目命名

当前项目名为 `demo_react`，如果打算做成正式框架，建议取一个更专业的名字，比如 `rustact`、`agent-rs`、`forgeai` 等。

---

## 五、功能扩展建议

| 优先级 | 功能 | 说明 |
|--------|------|------|
| P0 | Tool async 化 | 让工具支持异步执行，这是基础能力 |
| P0 | 修复已知 bug | 上面提到的 5 个问题 |
| P1 | Streaming 支持 | SSE 流式输出提升用户体验 |
| P1 | Memory 压缩 | 长对话必备 |
| P1 | 中间件系统 | 日志/监控/限流 |
| P2 | 多 Agent 编排 | 支持 Agent 之间协作（已预留 subagent） |
| P2 | 工具执行超时 | 防止工具无限阻塞 |
| P2 | 重试机制 | LLM 调用失败自动重试（指数退避） |
| P3 | 并行工具执行 | OpenAI 返回多个 tool_calls 时并行执行 |
| P3 | 持久化存储 | 对话历史、任务状态持久化 |

---

## 六、总结

项目骨架很好，核心的 ReAct 循环、工具系统、任务规划、人工审批四大模块都已搭建到位。主要需要改进的方向：

1. **修复 5 个具体 bug**（排序方向、循环终止、阻塞 IO、unwrap、async tool）
2. **引入 `thiserror` + `tracing`** 提升代码质量
3. **策略模式抽象** HumanApproval，让框架更灵活
4. **Builder 模式**构建 Agent，解耦内置工具的硬编码
5. **中间件 + Streaming** 让框架在真实场景中可用
