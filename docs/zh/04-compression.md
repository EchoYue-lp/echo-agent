# 上下文压缩（Context Compression）

## 是什么

LLM 的上下文窗口（Context Window）是有限的。当对话历史积累到一定长度时，如果直接发送全部消息，会超出 token 限制导致请求失败，或因 token 数量激增导致推理变慢、成本激增。

上下文压缩系统在每次调用 LLM 前自动检查当前消息历史的 token 用量，超限时按照配置的策略压缩，保留最有价值的信息。

---

## 解决什么问题

- **长对话支持**：处理数十轮以上的对话，不因上下文过长而崩溃
- **成本控制**：token 越少，API 费用越低
- **速度优化**：更短的上下文意味着更快的推理速度
- **自动透明**：压缩过程对 Agent 执行逻辑完全透明，无需手动干预

---

## 三种压缩策略

### 1. SlidingWindowCompressor（滑动窗口）

**原理**：保留最新的 N 条消息，丢弃最早的消息。

**优点**：无需 LLM 调用，速度极快，零成本。

**缺点**：早期对话内容完全丢失，无摘要保留。

```rust
use echo_agent::prelude::*;

SlidingWindowCompressor::new(20) // 保留最新 20 条消息
```

适用场景：对话轮次多但历史不重要，或对成本敏感。

---

### 2. SummaryCompressor（LLM 摘要压缩）

**原理**：将较旧的消息（超出保留窗口的部分）发送给 LLM 生成摘要，摘要作为一条新的 system 消息插入上下文。

**优点**：历史信息以摘要形式保留，不完全丢失。

**缺点**：压缩时需要额外的 LLM 调用（有成本）。

```rust
use echo_agent::prelude::*;
use echo_agent::llm::DefaultLlmClient;
use reqwest::Client;
use std::sync::Arc;

let llm = Arc::new(DefaultLlmClient::new(Arc::new(Client::new()), "qwen3-max"));

// 使用内置摘要提示词
SummaryCompressor::new(llm.clone(), DefaultSummaryPrompt, 6)
//                                                         ↑
//                                        保留最新 6 条消息不摘要

// 使用自定义摘要提示词
SummaryCompressor::new(
    llm.clone(),
    FnSummaryPrompt(|messages| {
        format!("请用 3 句话总结以下 {} 条对话：", messages.len())
    }),
    6,
)
```

---

### 3. HybridCompressor（混合管道）

**原理**：将多个压缩策略串联为管道，前一策略的输出作为后一策略的输入。

**典型用法**：先用滑动窗口快速裁剪，再对剩余过长部分用摘要精细压缩。

```rust
use echo_agent::prelude::*;

let compressor = HybridCompressor::builder()
    .stage(SlidingWindowCompressor::new(30))        // 第一阶段：保留最新 30 条
    .stage(SummaryCompressor::new(llm, DefaultSummaryPrompt, 8)) // 第二阶段：摘要
    .build();
```

---

## 与 Agent 集成

### 自动压缩（推荐）

配置 `AgentConfig::token_limit` 和压缩器，框架自动在每次 LLM 调用前检查并压缩：

```rust
let config = AgentConfig::new("qwen3-max", "agent", "你是一个助手")
    .token_limit(4096); // 超过 4096 token 时自动压缩

let mut agent = ReactAgent::new(config);

// 安装压缩器（默认没有，需手动设置）
agent.set_compressor(SlidingWindowCompressor::new(20));

// 此后所有 execute() 调用都受到自动压缩保护
let answer = agent.execute("...").await?;
```

### 手动触发压缩

```rust
// 使用已安装的压缩器强制压缩
let stats = agent.force_compress_with(
    &SlidingWindowCompressor::new(10)
).await?;

println!(
    "压缩前 {} 条 / {} token → 压缩后 {} 条 / {} token（裁剪 {} 条）",
    stats.before_count, stats.before_tokens,
    stats.after_count,  stats.after_tokens,
    stats.evicted
);
```

---

## 直接使用 ContextManager

不通过 Agent，直接使用 `ContextManager` 管理上下文：

```rust
use echo_agent::prelude::*;
use echo_agent::llm::types::Message;

// 构建带压缩器的上下文管理器
let mut ctx = ContextManager::builder(2000) // token 上限 2000
    .compressor(SlidingWindowCompressor::new(10))
    .build();

ctx.push(Message::system("你是一个助手".to_string()));
for i in 0..30 {
    ctx.push(Message::user(format!("问题 {}", i)));
    ctx.push(Message::assistant(format!("回答 {}", i)));
}

println!("压缩前 token: {}", ctx.token_estimate());

// prepare() 触发自动压缩，返回可发送给 LLM 的消息列表
let messages = ctx.prepare(None).await?;

println!("压缩后消息数: {}", messages.len());
```

---

## 压缩时机

```
调用 ctx.prepare() 时：
    │
    ├─ 估算当前 token 数（字符数 / 4，粗略估算）
    │
    ├─ 若 token_estimate() ≤ token_limit → 直接返回，不压缩
    │
    └─ 若 token_estimate() > token_limit → 调用 compressor.compress()
           ├─ SlidingWindow：直接截断（纳秒级）
           └─ Summary：调用 LLM 生成摘要（秒级，有成本）
```

---

## 最佳实践

| 场景 | 推荐策略 |
|------|---------|
| 聊天机器人（历史不重要） | `SlidingWindowCompressor(20~50)` |
| 任务执行 Agent（历史有价值） | `SummaryCompressor` 或 `Hybrid` |
| 高频调用、成本敏感 | `SlidingWindowCompressor` |
| 长文档分析 | `HybridCompressor`（先滑动窗口，再摘要） |
| 测试环境 | `SlidingWindowCompressor(5)` + `token_limit: 100` |

对应示例：`examples/demo05_compressor.rs`
