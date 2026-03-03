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

## 三、多轮对话模式（`chat()` 接口）— ✅ 已完成

> **实现时间**：2026-02-28
> **相关文件**：`src/agent/mod.rs`、`src/agent/react_agent.rs`
> **文档**：`docs/zh/13-chat.md`、`docs/en/13-chat.md`
> **示例**：`examples/demo17_chat.rs`

### 实现内容

在 `Agent` trait 和 `ReactAgent` 中新增了 `chat()` / `chat_stream()` 方法：

- **`Agent` trait**：新增 `chat()` 和 `chat_stream()` 方法，带默认实现（回退到 `execute`），对所有现有实现者无破坏性
- **`ReactAgent`**：新增内部方法 `run_chat_direct()`，覆盖 `chat()` / `chat_stream()`
  - 跳过每次调用时的上下文重置，直接追加用户消息
  - 完整支持工具调用、长期记忆（Store）注入、Checkpoint 自动保存
  - `chat_stream()` 在每轮答案生成后也会保存 Checkpoint（同步修复了 `execute_stream` 中缺失 Checkpoint 保存的问题）

### 使用对比

```rust
// 任务 Agent（execute 语义，每次独立）
agent.execute("帮我分析这份报告").await?;
agent.execute("帮我生成代码").await?; // 上一轮的报告内容不在上下文中

// 对话 Agent（chat 语义，持续累积）
agent.chat("你好，我叫张三").await?;
agent.chat("帮我分析这份报告").await?;
agent.chat("把分析结果用英文重写").await?; // 上轮分析结果在上下文中

// 流式多轮对话
let mut stream = agent.chat_stream("下一条消息").await?;

// 重置对话历史
agent.reset();
```

---

## 四、Store 语义搜索（向量检索）— ✅ 已完成

> **实现时间**：2026-02-28
> **相关文件**：`src/memory/embedder.rs`、`src/memory/embedding_store.rs`、`src/memory/store.rs`
> **文档**：`docs/zh/14-semantic-search.md`、`docs/en/14-semantic-search.md`
> **示例**：`examples/demo18_semantic_memory.rs`

### 实现内容

在 `Store` trait 新增两个默认方法（无破坏性变更）：

- `supports_semantic_search() -> bool`：默认 false，`EmbeddingStore` 返回 true
- `semantic_search(namespace, query, limit)`：默认回退到关键词 `search()`，`EmbeddingStore` 执行余弦相似度检索

新增 `Embedder` trait 和 `HttpEmbedder`（OpenAI / Qwen 兼容接口），以及 `EmbeddingStore`（包装任意 `Store` + 向量索引，支持持久化到 JSON 文件）。

`ReactAgent` 所有记忆注入点（`run_react_loop`、`execute_stream`、`chat_stream`）均已升级为调用 `semantic_search()`，透明支持向量检索。新增 `set_memory_store()` 方法同时替换 Store 并重注册记忆工具。

### 架构

```
EmbeddingStore
├── inner: Arc<dyn Store>      ← KV 持久化（FileStore / InMemoryStore）
├── embedder: Arc<dyn Embedder> ← 文本嵌入（HttpEmbedder / MockEmbedder）
└── VecIndex                   ← 内存向量索引（可选持久化 .vecs.json）
```

### 使用对比

```rust
// 默认关键词检索（FileStore）—— 跨语言查询命中率低
store.search(&["alice", "memories"], "music preference", 5).await?;

// 语义检索（EmbeddingStore）—— 跨语言、同义词均可命中
store.semantic_search(&["alice", "memories"], "music preference", 5).await?;

// Agent 集成（自动使用语义检索）
let inner = Arc::new(FileStore::new("~/.echo-agent/store.json")?);
let embedder = Arc::new(HttpEmbedder::from_env());
let store = Arc::new(EmbeddingStore::with_persistence(inner, embedder, "~/.echo-agent/store.vecs.json")?);

let mut agent = ReactAgent::new(config);
agent.set_memory_store(store); // 替换 Store + 重注册工具
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
| 4 | Store 语义搜索（向量检索） | ✅ 完成 | 高 | 长期记忆质量大幅提升 |
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
