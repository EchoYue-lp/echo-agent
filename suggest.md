# Echo Agent 优化改进建议

> 基于 LangChain、AutoGen、CrewAI、LlamaIndex 等主流框架对比分析
> 更新时间：2026-02-28

---

## 整体评价

框架架构扎实，模块边界清晰，核心能力已达主流框架水准：

- ✅ ReAct 循环（Thought → Action → Observation）+ Chain-of-Thought
- ✅ 工具系统（内置 + MCP + Skill + 自定义）+ 超时 / 重试 / 并发限流
- ✅ 并行工具调用（`join_all`）
- ✅ 流式输出（`execute_stream` + `AgentEvent`）
- ✅ 生命周期回调（`AgentCallback`）
- ✅ 任务规划与 DAG 执行（Planner 角色 + 拓扑调度 + Mermaid 可视化）
- ✅ 人工介入（审批 / 文本输入，支持 Console / Webhook / WebSocket）
- ✅ SubAgent 编排（Orchestrator / Worker / Planner 三种角色）
- ✅ 双层记忆（Store 长期 KV + Checkpointer 会话持久化）
- ✅ 上下文压缩（滑动窗口 + LLM 摘要 + 混合管道）
- ✅ Skill 系统（内置 + 外部 SKILL.md 加载）
- ✅ MCP 协议客户端（stdio + HTTP SSE）
- ✅ LLM 调用重试（网络错误 / 429 指数退避）
- ✅ 工具错误回传 LLM（`tool_error_feedback`，LLM 自主纠错）
- ✅ 结构化日志（tracing）

---

## 一、结构化输出（Structured Output）— 🔴 高优先级

### 现状

当前 LLM 调用不支持 `response_format`，只能依赖 function calling 获取结构化数据。
OpenAI / Qwen / DeepSeek 均已支持 `response_format: { type: "json_schema", schema: {...}, strict: true }`，
可强制 LLM 按指定 schema 输出，对任务规划阶段的子任务解析、记忆提取等场景非常有价值。

### 建议

在 `llm/types.rs` 新增 `ResponseFormat` 枚举，并在 `chat()` 参数中携带：

```rust
// llm/types.rs
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema {
        json_schema: JsonSchemaSpec,
    },
}

#[derive(Debug, Serialize)]
pub struct JsonSchemaSpec {
    pub name: String,
    pub schema: Value,
    pub strict: bool,
}

// ChatCompletionRequest 新增字段
pub struct ChatCompletionRequest {
    // ...现有字段...
    pub response_format: Option<ResponseFormat>,
}
```

典型使用场景：Planner 规划子任务时强制返回标准 JSON，避免自然语言解析失败：

```rust
let format = ResponseFormat::JsonSchema {
    json_schema: JsonSchemaSpec {
        name: "task_plan".into(),
        schema: json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/Task" }
                }
            }
        }),
        strict: true,
    },
};
```

---

## 二、Mock LLM / 测试基础设施 — 🔴 高优先级

### 现状

`LlmClient` trait 已存在（用于 `SummaryCompressor`），但没有 Mock 实现。
所有测试均依赖真实 LLM API 调用，无法做 CI 自动化，ReAct 循环逻辑缺乏单元测试覆盖。

### 建议

新增 `MockLlmClient`，预设响应序列：

```rust
// llm/mock.rs（新文件）
pub struct MockLlmClient {
    responses: Mutex<VecDeque<ChatCompletionResponse>>,
    call_count: AtomicUsize,
}

impl MockLlmClient {
    /// 预设工具调用序列后跟最终答案
    pub fn with_sequence(responses: Vec<ChatCompletionResponse>) -> Self { ... }

    /// 快捷构造：单次工具调用
    pub fn tool_then_answer(tool: &str, args: Value, answer: &str) -> Self {
        Self::with_sequence(vec![
            ChatCompletionResponse::tool_call(tool, args),
            ChatCompletionResponse::final_answer(answer),
        ])
    }

    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat(&self, _req: ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.responses.lock().await
            .pop_front()
            .ok_or_else(|| ReactError::Llm(LlmError::EmptyResponse))
    }
}
```

对应单元测试示例：

```rust
#[tokio::test]
async fn test_react_calls_tool_and_returns_answer() {
    let mock = Arc::new(MockLlmClient::tool_then_answer(
        "add",
        json!({"a": 3, "b": 4}),
        "3 + 4 = 7",
    ));

    let mut agent = ReactAgent::new_with_llm(
        AgentConfig::new("mock", "test", ""),
        mock.clone(),
    );
    agent.add_tool(Box::new(AddTool));

    let result = agent.execute("3 加 4 等于多少？").await.unwrap();
    assert_eq!(result, "3 + 4 = 7");
    assert_eq!(mock.call_count(), 2); // 第一次返回工具调用，第二次返回答案
}
```

`ReactAgent::new_with_llm(config, llm)` 构造函数仅需暴露为 `pub(crate)` 或 `#[cfg(test)]` 可用。

---

## 三、多轮对话模式（`chat()` 接口）— 🟡 中等优先级

### 现状

`execute()` 内部每次都调用 `reset_messages()` 重置上下文，是"单次任务"语义。
虽然 `session_id + Checkpointer` 可以跨进程续接，但在**同一进程内**无法做"连续聊天"——
每轮对话都从空白开始，适合任务 Agent 但不适合对话 Agent（Chatbot）场景。

### 建议

在 `Agent` trait 和 `ReactAgent` 中新增 `chat()` 方法，不重置历史、持续累积上下文：

```rust
// agent/mod.rs
#[async_trait]
pub trait Agent: Send + Sync {
    async fn execute(&mut self, task: &str) -> Result<String>; // 已有：单次任务，内部重置
    async fn chat(&mut self, message: &str) -> Result<String>; // 新增：多轮对话，保留历史
    async fn execute_stream(&mut self, task: &str) -> Result<BoxStream<'_, Result<AgentEvent>>>; // 已有
    async fn chat_stream(&mut self, message: &str) -> Result<BoxStream<'_, Result<AgentEvent>>>; // 新增
}

// react_agent.rs 实现
async fn chat(&mut self, message: &str) -> Result<String> {
    // 不调用 reset_messages()，直接追加用户消息
    self.context.push(Message::user(message.to_string()));
    self.run_react_loop().await
}
```

使用场景对比：

```rust
// 任务 Agent（当前 execute 语义，每次独立）
agent.execute("帮我分析这份报告").await?;
agent.execute("帮我生成代码").await?; // 上一轮的报告内容不在上下文中

// 对话 Agent（新 chat 语义，持续累积）
agent.chat("你好，我叫张三").await?;
agent.chat("帮我分析这份报告").await?;
agent.chat("把分析结果用英文重写").await?; // 上轮分析结果在上下文中
```

---

## 四、Store 语义搜索（向量检索）— 🟡 中等优先级

### 现状

`Store::search()` 实现是关键词匹配（字符串包含 + 词频评分），对于语义相似但用词不同的查询效果差：

```
存储：{"content": "用户喜好：古典音乐"}
查询：recall("music preference")  ← 英文查询，中文内容，命中为 0
```

### 建议

**方案 A（短期，无外部依赖）**：
扩展现有关键词匹配，加入简单的双语 / 归一化处理（Unicode 标准化、停用词过滤、ngram 索引）。

**方案 B（中期，可选功能）**：
为 `Store` trait 新增可选的 embedding 接口，配合本地嵌入模型（如 `fastembed-rs`）或远程 API：

```rust
// memory/store.rs
#[async_trait]
pub trait Store: Send + Sync {
    // ...现有方法...

    /// 是否支持语义搜索（默认 false，关键词搜索）
    fn supports_semantic_search(&self) -> bool { false }

    /// 语义搜索（仅在 supports_semantic_search() == true 时有效）
    async fn semantic_search(
        &self,
        namespace: &[&str],
        query: &str,
        limit: usize,
    ) -> Result<Vec<StoreItem>> {
        // 默认 fallback 到关键词搜索
        self.search(namespace, query, limit).await
    }
}

// 新增：向量 Store 实现
pub struct VectorStore {
    inner: FileStore,
    embedder: Arc<dyn Embedder>,
    index: Arc<RwLock<VectorIndex>>,
}
```

---

## 五、Agent 编排模式扩展 — 🟡 中等优先级

### 现状

当前仅支持 Orchestrator-Worker 模式（一对多分派）。复杂业务中还需要：

- **Pipeline（流水线）**：A 的输出作为 B 的输入，顺序处理
- **FanOut-FanIn（扇出聚合）**：将同一任务并发分配给多个 Worker，聚合结果
- **Race（竞争执行）**：多个 Agent 并发执行同一任务，取最快/质量最好的结果

### 建议

新增 `AgentPipeline` 工具类（不修改现有代码，作为上层封装）：

```rust
// agent/pipeline.rs（新文件）
pub struct AgentPipeline;

impl AgentPipeline {
    /// 顺序管道：前一个 Agent 的输出作为下一个的输入
    pub async fn sequential(
        agents: &mut [Box<dyn Agent>],
        initial_input: &str,
    ) -> Result<String> {
        let mut input = initial_input.to_string();
        for agent in agents.iter_mut() {
            input = agent.execute(&input).await?;
        }
        Ok(input)
    }

    /// 并行扇出 + 聚合：所有 Agent 并行执行同一任务
    pub async fn fan_out(
        agents: &mut [Box<dyn Agent>],
        task: &str,
    ) -> Result<Vec<String>> {
        // 无法同时持有多个 &mut，需要 Arc<AsyncMutex>
        todo!("需要 agents: Vec<Arc<AsyncMutex<Box<dyn Agent>>>>")
    }

    /// 竞争执行：取第一个成功完成的结果
    pub async fn race(
        agents: Vec<Arc<AsyncMutex<Box<dyn Agent>>>>,
        task: &str,
    ) -> Result<String> { ... }
}
```

---

## 六、`thiserror` 重构错误类型 — 🟢 低优先级

### 现状

`error.rs` 约 354 行，包含大量手写的 `Display` 实现和 `From` 转换样板代码。

### 建议

使用 `thiserror` crate 消除样板：

```toml
# Cargo.toml
[dependencies]
thiserror = "2"
```

```rust
// 改造前（手写 ~20 行）：
impl fmt::Display for LlmError { ... }
impl std::error::Error for LlmError {}
impl From<LlmError> for ReactError { ... }

// 改造后（3 行）：
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("网络请求失败: {0}")]
    NetworkError(String),

    #[error("API 错误 (状态码 {status}): {message}")]
    ApiError { status: u16, message: String },

    #[error("响应格式无效: {0}")]
    InvalidResponse(String),

    #[error("LLM 返回空响应")]
    EmptyResponse,
}

#[derive(Debug, thiserror::Error)]
pub enum ReactError {
    #[error(transparent)]
    Llm(#[from] LlmError),   // 自动生成 From<LlmError> for ReactError

    #[error(transparent)]
    Tool(#[from] ToolError),
    // ...
}
```

预计可将 `error.rs` 从 354 行压缩到约 120 行，且语义更清晰。

---

## 七、工具结果缓存 — 🟢 低优先级

### 现状

每次调用幂等工具（天气查询、搜索、文件读取）都会重新执行，同一任务循环内可能重复调用相同参数的工具。

### 建议

在 `Tool` trait 新增可选的缓存声明，`ToolManager` 自动缓存结果：

```rust
pub trait Tool: Send + Sync {
    // ...现有方法...

    /// 是否对相同参数的调用结果进行缓存（默认 false）
    fn cache_ttl(&self) -> Option<Duration> { None }
}

// ToolManager 内部维护缓存
struct CacheEntry {
    result: String,
    expires_at: Instant,
}

// 执行前检查缓存 key = (tool_name, params_hash)
```

---

## 八、可观测性增强（Tracing / Span）— 🟢 低优先级

### 现状

已有 `tracing` 结构化日志，但日志是"扁平"的，无法形成调用链。
对于多 Agent 编排场景，无法追踪"主 Agent → SubAgent A → 工具 X"这条完整的执行路径。

### 建议

为每次 `execute()` 创建一个 `tracing::Span`，工具调用和 SubAgent 分派作为子 Span：

```rust
// react_agent.rs
pub async fn execute(&mut self, task: &str) -> Result<String> {
    let span = tracing::info_span!(
        "agent.execute",
        agent = %self.config.agent_name,
        task = %task,
    );
    let _guard = span.enter();
    // ...现有逻辑...
}
```

这样接入 Jaeger / Zipkin / OTLP 后即可看到完整的多 Agent 调用树。

---

## 优先级汇总（截至 2026-02-28）

| # | 改进项 | 优先级 | 复杂度 | 预期收益 |
|---|--------|:------:|:------:|--------|
| 1 | 结构化输出（`response_format`） | 🔴 高 | 低 | 提升 Planner / 数据提取可靠性 |
| 2 | Mock LLM / 测试基础设施 | 🔴 高 | 中 | 支持 CI / 单元测试 |
| 3 | 多轮对话模式（`chat()` 接口） | 🟡 中 | 低 | 支持 Chatbot 场景 |
| 4 | Store 语义搜索（向量检索） | 🟡 中 | 高 | 长期记忆质量大幅提升 |
| 5 | Agent 编排模式扩展 | 🟡 中 | 中 | Pipeline / FanOut / Race 场景 |
| 6 | `thiserror` 重构 | 🟢 低 | 低 | error.rs 代码量减少 ~65% |
| 7 | 工具结果缓存 | 🟢 低 | 低 | 减少重复工具调用 |
| 8 | Tracing Span 调用链 | 🟢 低 | 低 | 多 Agent 可观测性 |

---

## 已完成项（自 2026-02-26 起）

以下建议均已实现，记录以供参考：

| 原建议 | 完成状态 |
|--------|---------|
| 流式输出 | ✅ `execute_stream()` + `AgentEvent` |
| 事件回调系统 | ✅ `AgentCallback` trait（on_think/on_tool/on_final_answer 等） |
| LLM 调用重试 | ✅ `is_retryable_llm_error` + 指数退避，可配 `llm_retry_delay_ms` |
| 工具错误回传 LLM | ✅ `tool_error_feedback` 配置（默认开启） |
| 人工审批异步化 | ✅ `HumanLoopProvider` trait + Console / Webhook / WebSocket |
| 工具超时控制 | ✅ `ToolExecutionConfig`（timeout/retry/concurrency） |
| 记忆分层（L1 + L2） | ✅ `ContextManager`（工作记忆）+ `Store`（语义记忆）+ `Checkpointer`（会话历史） |
