# Echo Agent 优化改进建议

> 基于 LangChain、AutoGen、CrewAI、LlamaIndex 等主流框架对比分析
> 更新时间：2026-02-26

---

## 整体评价

框架架构扎实，模块边界清晰，trait 抽象合理：

- ✅ ReAct 循环（think → action → observation）
- ✅ 工具系统（内置 + MCP + 自定义 + Skill）
- ✅ 任务规划与 DAG 执行
- ✅ 人工审批机制
- ✅ 子 Agent 编排
- ✅ 上下文压缩（滑动窗口 + 摘要 + 混合）
- ✅ Skill 系统（内置 + 外部文件加载）
- ✅ MCP 协议集成
- ✅ 异步工具执行（async trait）
- ✅ 结构化日志（tracing）
- ✅ 并行工具调用

---

## 一、流式输出（Streaming）— 🔴 高优先级

这是目前最明显的缺失。主流框架（LangChain、LlamaIndex）都把 streaming 作为核心 API。
当前 `chat()` 是一次性等待完整响应，用户需要等待整个推理过程结束才能看到结果。

**建议：** 在 `llm/client.rs` 中增加 `chat_stream()` 接口，通过解析 Server-Sent Events 按 delta 推送：

```rust
// LLM 层新增流式接口
pub async fn chat_stream(
    client: Arc<Client>,
    model: &str,
    messages: Vec<Message>,
    options: ChatOptions,
) -> Result<impl Stream<Item = Result<String>>> {
    // 解析 SSE，逐 token 推送
}
```

`Agent` trait 增加流式执行入口：

```rust
#[async_trait]
pub trait Agent: Send + Sync {
    async fn execute(&mut self, task: &str) -> Result<String>;

    // 新增：流式执行，逐事件推送
    async fn execute_stream(
        &mut self,
        task: &str,
    ) -> Result<BoxStream<'_, Result<AgentEvent>>>;
}

pub enum AgentEvent {
    Token(String),            // LLM 推理 token
    ToolCall { name: String, args: Value },
    ToolResult { name: String, output: String },
    FinalAnswer(String),
}
```

---

## 二、事件回调系统（Callbacks / Hooks）— 🔴 高优先级

LangChain 的 Callbacks 是最常被开发者使用的可扩展点。目前框架只有 `tracing` 日志，
外部代码无法感知 Agent 的内部事件，无法做实时 UI 进度展示、接入 LangSmith 类监控平台。

**建议：** 新增 `AgentCallback` trait，在 `AgentConfig` 中注册：

```rust
#[async_trait]
pub trait AgentCallback: Send + Sync {
    async fn on_think_start(&self, agent: &str, messages: &[Message]) {}
    async fn on_think_end(&self, agent: &str, steps: &[StepType]) {}
    async fn on_tool_start(&self, agent: &str, tool: &str, args: &Value) {}
    async fn on_tool_end(&self, agent: &str, tool: &str, result: &str) {}
    async fn on_tool_error(&self, agent: &str, tool: &str, err: &ReactError) {}
    async fn on_final_answer(&self, agent: &str, answer: &str) {}
    async fn on_iteration(&self, agent: &str, iteration: usize) {}
}

// AgentConfig 中注册
pub struct AgentConfig {
    // ...现有字段...
    pub callbacks: Vec<Arc<dyn AgentCallback>>,
}
```

使用示例：

```rust
// 自定义进度打印回调
struct ProgressCallback;

#[async_trait]
impl AgentCallback for ProgressCallback {
    async fn on_tool_start(&self, agent: &str, tool: &str, _args: &Value) {
        println!("[{}] 正在调用工具: {}", agent, tool);
    }
    async fn on_final_answer(&self, agent: &str, answer: &str) {
        println!("[{}] 完成: {}", agent, answer);
    }
}
```

---

## 三、LLM 调用重试 + 工具错误回传 LLM — 🔴 高优先级

### 3.1 LLM 调用缺少重试逻辑

Rate limit（429）、临时网络抖动会直接导致任务失败。建议在 `llm/client.rs` 增加带指数退避的重试：

```rust
pub struct RetryConfig {
    pub max_attempts: u32,          // 默认 3
    pub initial_delay_ms: u64,      // 默认 1000
    pub max_delay_ms: u64,          // 默认 30_000
    pub retryable_status: Vec<u16>, // [429, 502, 503, 504]
}
```

### 3.2 工具执行失败应回传给 LLM，而不是直接报错

当前 `react_agent.rs` 中工具执行失败会直接向上传播错误，导致整个 Agent 中断。
主流框架（LangChain、AutoGen）的做法是将错误作为 observation 告知 LLM，让它决策下一步：

```rust
// 当前行为：工具失败 → Agent 直接报错
let result = self.execute_tool(&function_name, &arguments).await?; // ← 直接 ? 传播

// 建议改为：工具失败 → 封装为错误观察，让 LLM 自主恢复
let result = match self.execute_tool(&function_name, &arguments).await {
    Ok(output) => output,
    Err(e) => format!("工具执行失败: {}。请尝试其他方案或换一个工具。", e),
};
self.context.push(Message::tool_result(tool_call_id, function_name, result));
```

---

## 四、异步化人工审批 — 🟡 中等优先级

`execute_tool` 中直接调用 `std::io::stdin().read_line()`，这是同步阻塞调用，
**会占用整个 tokio 工作线程**，在高并发场景下会导致运行时饥饿。

```rust
// 当前问题代码（react_agent.rs）
std::io::stdin().read_line(&mut user_input)?; // ← 阻塞 tokio 线程！
```

**短期修复：** 用 `tokio::io` 替换：

```rust
use tokio::io::{AsyncBufReadExt, BufReader};

let stdin = tokio::io::stdin();
let mut reader = BufReader::new(stdin);
let mut user_input = String::new();
reader.read_line(&mut user_input).await?;
```

**长期方案：** 抽象为 `ApprovalProvider` trait，支持 WebSocket 推送、HTTP 回调等多种审批渠道：

```rust
#[async_trait]
pub trait ApprovalProvider: Send + Sync {
    async fn request_approval(
        &self,
        tool_name: &str,
        args: &Value,
    ) -> Result<ApprovalDecision>;
}

pub enum ApprovalDecision {
    Approved,
    Rejected { reason: Option<String> },
    Timeout,
}

// 内置实现
pub struct ConsoleApproval;   // 当前行为：控制台 y/n
pub struct WebhookApproval { pub url: String }  // HTTP 回调
```

---

## 五、工具超时控制 — 🟡 中等优先级

工具执行目前没有超时机制，MCP 工具或网络工具挂起会导致整个 Agent 无限期等待。

**建议：** 在 `ToolManager::execute_tool` 中统一加 timeout：

```rust
// tools/mod.rs 新增配置
pub struct ToolExecutionConfig {
    pub timeout_ms: u64,    // 默认 30_000
    pub retry_on_fail: bool,
    pub max_retries: u32,
}

// 执行时包裹 tokio::time::timeout
tokio::time::timeout(
    Duration::from_millis(config.timeout_ms),
    tool.execute(params),
)
.await
.map_err(|_| ToolError::Timeout(tool_name.to_string()))?
```

同时 `ToolError` 增加 `Timeout` 变体：

```rust
pub enum ToolError {
    // ...现有变体...
    Timeout(String),  // 工具执行超时
}
```

---

## 六、多轮对话支持 — 🟡 中等优先级

目前 `run_direct()` 每次都调用 `reset_messages()`，导致每次 `execute()` 都是全新对话，
无法支持连续多轮交互（如 Chat Agent 场景）。

**建议：** 在 `Agent` trait 增加多轮对话接口：

```rust
#[async_trait]
pub trait Agent: Send + Sync {
    // 单次任务执行（当前行为，内部重置历史）
    async fn execute(&mut self, task: &str) -> Result<String>;

    // 多轮对话：不重置历史，保留上下文（新增）
    async fn chat(&mut self, message: &str) -> Result<String>;

    // 显式清除历史（新增）
    fn clear_history(&mut self);
}
```

`ReactAgent` 对应实现：

```rust
async fn chat(&mut self, message: &str) -> Result<String> {
    // 不调用 reset_messages()，直接追加消息
    self.context.push(Message::user(message.to_string()));
    self.run_react_loop().await
}
```

---

## 七、结构化输出支持 — 🟡 中等优先级

当前 LLM 只支持 function calling，但主流 API 都支持 `response_format: json_schema`，
可强制 LLM 按指定 schema 返回，对任务规划阶段的结构化数据提取非常有价值。

**建议：** 在 `chat()` 接口增加 `response_format` 参数：

```rust
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema {
        name: String,
        schema: Value,
        strict: bool,
    },
}

pub async fn chat(
    client: Arc<Client>,
    model: &str,
    messages: Vec<Message>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    stream: Option<bool>,
    tools: Option<Vec<Value>>,
    tool_choice: Option<Value>,
    response_format: Option<ResponseFormat>, // 新增
) -> Result<ChatCompletionResponse>
```

---

## 八、Mock LLM / 测试基础设施 — 🟡 中等优先级

目前没有任何单元测试基础设施，所有测试都依赖真实 LLM API 调用，无法做 CI 自动化。
LangChain、LlamaIndex 都提供 FakeLLM 用于测试。

**建议：** 将 LLM 调用抽象为 trait，提供 Mock 实现：

```rust
// llm/mod.rs：新增 LlmClient trait
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse>;
}

// 生产实现（当前逻辑）
pub struct OpenAiClient { client: Arc<reqwest::Client>, model: String }

// 测试用 Mock 实现
pub struct MockLlmClient {
    responses: Mutex<VecDeque<ChatResponse>>,
}

impl MockLlmClient {
    // 预设工具调用响应
    pub fn with_tool_call(tool: &str, args: Value) -> Self { ... }
    // 预设最终答案响应
    pub fn with_final_answer(answer: &str) -> Self { ... }
    // 预设响应序列
    pub fn with_sequence(responses: Vec<ChatResponse>) -> Self { ... }
}
```

这样可以对 ReAct 循环逻辑做不依赖网络的单元测试：

```rust
#[tokio::test]
async fn test_react_loop_calls_tool_then_answers() {
    let mock_llm = MockLlmClient::with_sequence(vec![
        ChatResponse::tool_call("weather", json!({"city": "Beijing"})),
        ChatResponse::final_answer("北京今天晴，25°C"),
    ]);

    let mut agent = ReactAgent::new_with_llm(config, Arc::new(mock_llm));
    agent.add_tool(Box::new(WeatherTool));

    let result = agent.execute("北京天气如何？").await.unwrap();
    assert_eq!(result, "北京今天晴，25°C");
}
```

---

## 九、用 `thiserror` 简化错误代码 — 🟢 低优先级

`error.rs` 有 312 行，包含大量样板代码（手写 `Display` + `From` 实现）。
使用 `thiserror` 可大幅简化，且语义更清晰：

**当前写法（每个变体需要手写多个 impl）：**

```rust
impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::NetworkError(msg) => write!(f, "Network error: {}", msg),
            // ... 逐一手写
        }
    }
}
impl std::error::Error for LlmError {}
```

**改用 `thiserror` 后（一个 derive 搞定）：**

```rust
// Cargo.toml 新增：thiserror = "1"

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("API error (status {status}): {message}")]
    ApiError { status: u16, message: String },

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Empty response from LLM")]
    EmptyResponse,

    #[error("Serialization error: {0}")]
    SerializationError(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ReactError {
    #[error("LLM Error: {0}")]
    Llm(#[from] LlmError),  // #[from] 自动生成 From impl

    #[error("Tool Error: {0}")]
    Tool(#[from] ToolError),

    // ...
}
```

预计可将 `error.rs` 从 312 行压缩到约 100 行。

---

## 十、功能性扩展建议

### 10.1 记忆分层（Memory Hierarchy）

目前 `ContextManager` 只是短期记忆（当前对话历史）。建议增加：

| 层级 | 名称 | 描述 | 实现方案 |
|------|------|------|----------|
| L1 | 工作记忆 | 当前对话历史 | 已有 `ContextManager` |
| L2 | 语义记忆 | 跨对话的 key-value 知识 | `sled` 或内存 `HashMap` |
| L3 | 向量记忆 | 长期知识检索（RAG） | `qdrant-client` / `lancedb` |

### 10.2 Agent 编排模式扩展

目前 `enable_subagent` 只支持 Orchestrator-Worker 模式，可以补充：

```rust
pub enum OrchestrationPattern {
    // 当前已有：Orchestrator 调度
    Orchestrator,
    // 新增：顺序管道（A 输出 → B 输入）
    Pipeline(Vec<Box<dyn Agent>>),
    // 新增：并行扇出 + 汇总
    FanOutFanIn {
        workers: Vec<Box<dyn Agent>>,
        reducer: Box<dyn Agent>,
    },
    // 新增：竞争执行，取最快结果
    Race(Vec<Box<dyn Agent>>),
}
```

### 10.3 工具执行结果缓存

对于幂等工具（如天气查询、搜索），可以缓存结果避免重复调用：

```rust
pub trait Tool: Send + Sync {
    // 新增：声明工具是否幂等（可缓存）
    fn is_idempotent(&self) -> bool { false }
    fn cache_ttl(&self) -> Option<Duration> { None }
    // ...
}
```

---

## 优先级汇总

| # | 改进项 | 优先级 | 实现复杂度 | 预期收益 |
|---|--------|:------:|:--------:|--------|
| 1 | 流式输出 | 🔴 高 | 中 | 大幅提升用户体验 |
| 2 | 事件回调系统 | 🔴 高 | 低 | 可观测性、监控集成 |
| 3 | LLM 重试 + 工具错误回传 LLM | 🔴 高 | 低 | 大幅提升鲁棒性 |
| 4 | 人工审批异步化 | 🟡 中 | 低 | 修复运行时阻塞问题 |
| 5 | 工具超时控制 | 🟡 中 | 低 | 防止挂起 |
| 6 | 多轮对话支持 | 🟡 中 | 低 | 支持 Chat 场景 |
| 7 | 结构化输出 | 🟡 中 | 低 | 提升 Planning 可靠性 |
| 8 | Mock LLM / 测试基础设施 | 🟡 中 | 中 | 支持 CI / 单元测试 |
| 9 | `thiserror` 重构 | 🟢 低 | 低 | 代码量减少 ~60% |
| 10 | 记忆分层 / RAG | 🟢 低 | 高 | 长期知识积累 |
| 11 | Agent 编排模式扩展 | 🟢 低 | 高 | 更复杂的协作场景 |

**建议优先攻坚前三项**：流式输出、回调系统、工具失败错误回传 LLM——这三项对用户体验和 Agent 鲁棒性影响最大，且实现成本相对较低。
