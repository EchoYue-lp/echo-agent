# 结构化输出（Structured Output）

## 是什么

结构化输出向 LLM 发送格式提示；启用严格 JSON Schema 时，框架还会在结构化提取成功前本地校验返回值。开发者无需再用正则表达式或字符串解析提取数据，合格输出可以直接反序列化为 Rust 结构体。

echo-agent 通过三层 API 支持结构化输出：

```
ResponseFormat（类型）
    └─ chat() / stream_chat()       ← 底层 LLM 请求
         └─ AgentConfig::response_format() ← Agent 全局配置
              └─ ReactAgent::extract_json() / extract::<T>()  ← 便捷方法
```

---

## 解决什么问题

### 传统方式的痛点

```
LLM 返回："人物：李明，年龄：34，职业：软件工程师"
↓
开发者需要：正则 / 字符串分割 / 自己写解析逻辑
↓
脆弱、容易出错、格式稍变就崩溃
```

### 结构化输出的方式

```
定义 JSON Schema → 发送 Provider 提示 → 本地校验严格模式结果
↓
{"name":"李明","age":34,"occupation":"软件工程师"}
↓
直接 serde_json::from_str::<Person>() → 强类型结构体
```

结构化输出解决了：
- **信息提取**：从非结构化文本中抽取字段（人名、时间、金额）
- **分类标注**：情感分析、意图识别，输出枚举值
- **格式转换**：将自然语言描述转为程序可消费的数据结构
- **批量提取**：从长文本中提取数组型数据（事件列表、商品清单）

---

## 核心类型

```rust
/// 响应格式控制
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// 默认自由文本
    Text,
    /// 强制输出合法 JSON（不校验结构）
    JsonObject,
    /// 按指定 JSON Schema 严格输出
    JsonSchema { json_schema: JsonSchemaSpec },
}

pub struct JsonSchemaSpec {
    pub name: String,             // Schema 名称
    pub schema: serde_json::Value, // 标准 JSON Schema
    pub strict: bool,              // 是否严格遵守（默认 true）
}
```

`ResponseFormat::json_schema()` 是快速构建的快捷方法：

```rust
let fmt = ResponseFormat::json_schema(
    "person",            // schema 名称
    json!({              // JSON Schema 定义
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age":  { "type": "integer" }
        },
        "required": ["name", "age"],
        "additionalProperties": false
    }),
);
```

`JsonSchema` 的 `strict: true` 会在本地编译并执行 Schema 校验。非法 Schema 在一次性 LLM 请求前被拒绝；JSON 语法错误与严格 Schema 不匹配共用配置的有界修复重试预算，耗尽后返回类型化错误。`strict: false` 仅向 Provider 发送提示；`JsonObject` 只检查 JSON 语法。ADR 0079 规定在模型调用前拒绝外部 HTTP/文件 `$ref`，可将定义打包为本地 `$defs`。

---

## 使用方式

### 方式一：`extract_json()` — 返回 `serde_json::Value`

适合需要动态处理 JSON 字段、或不需要强类型绑定的场景：

```rust
use echo_agent::prelude::*;
use serde_json::json;

let llm_config = LlmConfig::for_provider(
    "openai",
    "https://api.openai.com/v1",
    std::env::var("OPENAI_API_KEY")?,
    "gpt-5.5",
    LlmApiProtocol::Responses,
)?;
let agent = ReactAgentBuilder::new()
    .llm_config(llm_config)
    .name("extractor")
    .system_prompt("你是一个信息提取助手")
    .disable_cot()
    .build()?;

let schema = ResponseFormat::json_schema(
    "person",
    json!({
        "type": "object",
        "properties": {
            "name":       { "type": "string" },
            "age":        { "type": "integer" },
            "occupation": { "type": "string" }
        },
        "required": ["name", "age", "occupation"],
        "additionalProperties": false
    }),
);

let value = agent.extract_json(
    "李明，34岁，是一名软件工程师。",
    schema,
).await?;

println!("{}", value["name"]);       // "李明"
println!("{}", value["age"]);        // 34
println!("{}", value["occupation"]); // "软件工程师"
```

### 方式二：`extract::<T>()` — 直接反序列化为 Rust 结构体

最常用的方式，类型安全，编译期检查：

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct SentimentResult {
    sentiment:  String,    // "positive" | "negative" | "neutral"
    confidence: f64,       // 0.0 ~ 1.0
    keywords:   Vec<String>,
    summary:    String,
}

let schema = ResponseFormat::json_schema(
    "sentiment_result",
    json!({
        "type": "object",
        "properties": {
            "sentiment":  { "type": "string", "enum": ["positive", "negative", "neutral"] },
            "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
            "keywords":   { "type": "array", "items": { "type": "string" } },
            "summary":    { "type": "string" }
        },
        "required": ["sentiment", "confidence", "keywords", "summary"],
        "additionalProperties": false
    }),
);

let review = "这款手机真的太棒了！性能强劲，续航持久，强烈推荐！";
let result: SentimentResult = agent.extract(review, schema).await?;

println!("情感: {}", result.sentiment);    // "positive"
println!("置信: {:.0}%", result.confidence * 100.0); // "95%"
```

### 方式三：`ReactAgentBuilder::response_format()` — 全局配置

格式随运行快照进入每次主 ReAct JSON 请求。模型事实必须明确支持结构化输出；未知或不支持的模型在调用前拒绝。严格 JSON Schema 还会在文本或 `final_answer` 工具结果成为成功终态前由框架本地校验；JSON 或 Schema 错误可在独立预算内有界修正，耗尽则失败。`execute_typed()` 再将合格值反序列化为请求的 Rust 类型：

```rust
let llm_config = LlmConfig::for_provider(
    "openai",
    "https://api.openai.com/v1",
    std::env::var("OPENAI_API_KEY")?,
    "gpt-5.5",
    LlmApiProtocol::Responses,
)?;
let mut agent = ReactAgentBuilder::new()
    .llm_config(llm_config)
    .name("translator")
    .system_prompt("你是一个翻译助手")
    .response_format(ResponseFormat::json_schema(
        "translation_result",
        json!({
            "type": "object",
            "properties": {
                "original":    { "type": "string" },
                "translation": { "type": "string" },
                "language":    { "type": "string" }
            },
            "required": ["original", "translation", "language"],
            "additionalProperties": false
        }),
    ))
    .disable_cot()
    .build()?;

let v: serde_json::Value = agent.execute_typed("人工智能正在改变世界。").await?;
println!("translation: {}", v["translation"]);
```

### 方式四：数组 / 嵌套结构

从长文本中批量提取多条记录：

```rust
#[derive(Debug, Deserialize)]
struct EventList {
    events: Vec<HistoryEvent>,
}

#[derive(Debug, Deserialize)]
struct HistoryEvent {
    year: i32,
    description: String,
    significance: String,
}

let schema = ResponseFormat::json_schema(
    "event_list",
    json!({
        "type": "object",
        "properties": {
            "events": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "year":         { "type": "integer" },
                        "description":  { "type": "string" },
                        "significance": { "type": "string" }
                    },
                    "required": ["year", "description", "significance"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["events"],
        "additionalProperties": false
    }),
);

let result: EventList = agent.extract(long_text, schema).await?;
for event in &result.events {
    println!("[{}] {} — {}", event.year, event.description, event.significance);
}
```

---

## 三种模式对比

| 模式 | 适用场景 | 结构校验 |
|------|---------|---------|
| `ResponseFormat::Text` | 默认，自由问答 | 无 |
| `ResponseFormat::JsonObject` | 输出任意 JSON，不需要固定字段 | 仅验证合法 JSON |
| `ResponseFormat::JsonSchema` | 固定字段提取/分类/转换 | `strict: true` 本地校验；`strict: false` 仅 Provider 提示 |

---

## 与工具调用的关系

`extract_json()` / `extract()` **不走 ReAct 循环**，也不触发工具调用：

```
extract_json(prompt, schema)
    │
    └─ chat()，携带 response_format
         LLM 输出 JSON 文本
         解析并校验严格 Schema；语法错误或不匹配时在配置上限内修复重试
```

若需要"先用工具查询数据，再以结构化格式输出结果"，推荐两阶段模式：

```rust
// 阶段 1：ReAct Agent 用工具完成数据收集
let raw_answer = agent.execute("查询最近3天的销售数据并汇总").await?;

// 阶段 2：单次结构化提取
let report: SalesReport = extractor_agent.extract(&raw_answer, schema).await?;
```

---

## 注意事项

1. **模型兼容性**：JSON 格式需要新鲜且明确的结构化输出能力事实。Provider 支持有助于生成合格输出；即使 Provider 忽略提示，本地严格校验也会拒绝不匹配值。显式 `Text` 会按未约束的默认请求发送。
2. **`additionalProperties: false`**：JSON Schema 中建议始终设置此项，防止模型输出多余字段
3. **与 CoT 的关系**：结构化提取场景通常不需要推理链，建议 `.enable_cot(false)` 以减少干扰
4. **temperature**：`extract_json()` 内部固定使用 `temperature=0.0`，保证输出稳定；`AgentConfig::response_format()` 方式仍使用 Agent 默认温度
5. **Schema 引用**：严格校验支持 `$defs` 等本地引用；外部 HTTP/文件 `$ref` 会在模型调用前拒绝。请将外部定义打包进传入的 Schema。

---

## 完整示例

对应示例：`echo-agent-learning/examples/demo15_structured_output.rs`

```bash
cargo run -p echo-agent-learning --example demo15_structured_output
```
