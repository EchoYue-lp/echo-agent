# 上下文压缩（Context Compression）

## 是什么

LLM 的上下文窗口（Context Window）是有限的。当对话历史积累到一定长度时，如果直接发送全部消息，会超出 token 限制导致请求失败，或因 token 数量激增导致推理变慢、成本激增。

上下文压缩系统在每次调用 LLM 前自动检查当前消息历史的 token 用量，超限时按照配置的策略压缩，保留最有价值的信息。

压缩只改变发送给模型的活动上下文。配置 `ConversationStore` 后，框架会在任何
活动历史替换之前持久化用户可见 transcript，并把后续压缩窗口中的新增后缀合并进
完整记录；`RuntimeStateStore` checkpoint 则继续保存较小的恢复视图。因此摘要不会
反向覆盖磁盘上的完整会话。

---

## 解决什么问题

- **长对话支持**：处理数十轮以上的对话，不因上下文过长而崩溃
- **成本控制**：token 越少，API 费用越低
- **速度优化**：更短的上下文意味着更快的推理速度
- **自动透明**：压缩过程对 Agent 执行逻辑完全透明，无需手动干预

---

## 压缩策略

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

内置提示词生成的是可继续执行的语义检查点，而不是流水账式复述。结构化输出分别
保存当前目标、工作状态、决策、文件、错误、偏好、不可违反的约束、精确关键事实和
下一步。摘要与近期原文随后还会经过 token 上限适配，固定的保留条数不会再让最终
模型输入停留在有效预算之外。

```rust
use echo_agent::prelude::*;
use echo_agent::llm::OpenAiClient;
use std::sync::Arc;

let llm = Arc::new(OpenAiClient::from_env("qwen3-max")?);

// 使用内置摘要提示词
SummaryCompressor::new(llm.clone(), 6)
//                                 ↑ 保留最新 6 条消息不摘要

// 使用自定义摘要提示词
SummaryCompressor::with_prompt(
    llm.clone(),
    6,
    |messages| format!("请用 3 句话总结以下 {} 条对话：", messages.len()),
)
```

### 设计依据

本实现参考了成熟 coding agent 收敛出的共同模式：

- OpenAI Codex 只用摘要、保留的用户材料和重新构造的 initial/world context 替换
  活动历史，同时持久化独立 compaction item，并明确提示连续压缩是有损的。实现见
  固定版本的 [Codex `compact.rs`](https://github.com/openai/codex/blob/53eaa297e595fc98df0f33d4c63686a7014d7c9a/codex-rs/core/src/compact.rs)。
- Claude Code 提供压缩前 hook，压缩后恢复 session/plan 等状态，并持续修复工具结果
  和 resume 边界。依据见官方 [Claude Code CHANGELOG](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)。
- OpenCode 无条件保护最新用户轮次，更旧的原始工具结果共享 40K token 预算，并且
  只有预计至少释放 20K token 时才清理；其近期压缩尾部同样按 token 而非固定轮数
  选择。实现见官方 [OpenCode compaction 源码](https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/session/compaction.ts)。
- Pi 将单个工具结果限制为 2,000 行或 50 KiB，整体压缩保留约 20K recent token，
  且绝不在 tool call/result 中间切断。依据见官方 [Pi compaction 文档](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/compaction.md)
  和 [工具截断源码](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/truncate.ts)。

Echo Agent 据此保持四个权威边界：`ConversationStore` 保存完整展示/审计 transcript，
`ContextManager` 保存有界活动上下文，`CanonicalContext` 与 projection 重建权威规则和
状态，仓库文件等外部知识继续由工具按需取回。

主动折叠工具轨迹时，应使用混合策略而不是固定轮数：至少保护最新用户轮次；更旧
原始结果使用随模型窗口缩放的总 token 预算；收益太小时不改写历史；tool call/result
始终成组保留或成组折叠。

---

### 3. IncrementalSummaryCompressor（增量摘要压缩）

> **新增于 v0.2.2。** 复用历史摘要的增量 LLM 摘要压缩器。

与 `SummaryCompressor` 每次全量重新摘要不同，`IncrementalSummaryCompressor` 维护上次的摘要文本，后续压缩时只发送 `[上次摘要] + [新增消息]` 给 LLM。对于需要多次压缩的长对话，可显著降低 LLM 成本和延迟。

```rust
use echo_agent::compression::compressor::IncrementalSummaryCompressor;
use std::sync::Arc;

let compressor = IncrementalSummaryCompressor::new(llm, 6);
// 第一次压缩：摘要全部旧消息（与 SummaryCompressor 相同）
// 第二次压缩：只发送上次摘要 + 新增消息
// 第三次压缩：同上，更便宜

// 查看或重置存储的摘要：
println!("当前摘要: {:?}", compressor.current_summary());
compressor.reset();
```

**优点**：长对话多次压缩时成本大幅降低。

**缺点**：需要维护内部状态（`Mutex` 保护）；逻辑略复杂。

---

### 4. HybridCompressor（混合管道）

**原理**：将多个压缩策略串联为管道，前一策略的输出作为后一策略的输入。

**典型用法**：先用滑动窗口快速裁剪，再对剩余过长部分用摘要精细压缩。

```rust
use echo_agent::prelude::*;

let compressor = HybridCompressor::builder()
    .stage(SlidingWindowCompressor::new(30))        // 第一阶段：保留最新 30 条
    .stage(SummaryCompressor::new(llm, 8))          // 第二阶段：摘要
    .build();
```

**短路优化**（v0.2.2 新增）：默认启用。当某阶段执行后 token 数已降至阈值以下，跳过后续阶段，避免不必要的 LLM 调用。

```rust
// 禁用短路（始终执行所有阶段）
let compressor = HybridCompressor::builder()
    .stage(SlidingWindowCompressor::new(30))
    .stage(SummaryCompressor::new(llm, 8))
    .short_circuit(false)
    .build();
```

---

### 5. AdaptiveCompressor（自适应压缩）

> **新增于 v0.2.1，v0.2.2 增强。** 自动根据上下文长度选择压缩级别。

`AdaptiveCompressor` 实现多级渐进式压缩策略，当上下文超过 token 阈值时自动升级压缩强度：

| 级别 | 名称 | 策略 | 触发阈值 | LLM? |
|------|------|------|---------|------|
| L1 | **Snip** | 移除超过 token 上限的工具输出 | `l1_snip_threshold_tokens` (80k) | 否 |
| L1 | **Fold** | 折叠连续的工具结果，保留最新 N 条 | Snip 后执行 | 否 |
| L2 | **Micro** | 截断工具输出，保留首尾各 N 行 | `l2_micro_threshold_tokens` (100k) | 否 |
| L3 | **Collapse** | 丢弃较早消息，保留系统提示 + 最近 N 条 | `l3_collapse_threshold_tokens` (120k) | 否 |
| L4 | **Auto Compact** | LLM 全文摘要 | `l4_compact_threshold_tokens` (150k) | 是（可选） |
| L5 | **Reactive** | 紧急模式：仅保留系统提示 + 最近 3 条消息 | 超过 L4 阈值 + 2×target | 否 |

**v0.2.2 变更：**
- L4 现在**内置支持**，通过 `.with_llm()` 启用 — 无需外部集成
- L1 新增**工具折叠**（`l1_fold_consecutive_tools`），自动折叠连续的工具消息
- `AdaptiveCompressor` 现在实现了 `ContextCompressor` trait — 可直接通过 `ContextManager::builder().compressor()` 使用

### 配置

```rust
use echo_agent::compression::levels::{AdaptiveCompressor, AdaptiveCompressionConfig};

let config = AdaptiveCompressionConfig {
    l1_snip_threshold_tokens: 80_000,
    l1_max_output_tokens: 4_000,        // Snip 时单条输出最大 token
    l1_fold_consecutive_tools: true,     // 折叠连续工具结果（新增）
    l1_fold_keep_latest: 2,             // 每组保留最新 N 条工具结果（新增）
    l2_micro_threshold_tokens: 100_000,
    l2_keep_lines: 50,                   // Micro 时保留首尾行数
    l3_collapse_threshold_tokens: 120_000,
    l3_keep_recent: 10,                  // Collapse 时保留最近消息数
    l4_compact_threshold_tokens: 150_000,
    l4_keep_recent: 6,                   // Compact 时保留最近消息数
};

// 不带 LLM：L4 被跳过，直接降级到 L5
let compressor = AdaptiveCompressor::new(config.clone());

// 带 LLM：L4 自动摘要被启用
let compressor = AdaptiveCompressor::new(config).with_llm(llm);
```

### 工作原理

```
Token 数:     0 ──── 80k ──── 100k ──── 120k ──── 150k ──── ∞
               │       │        │         │         │        │
               │ 无压缩 │  Snip  │  Micro  │Collapse │Compact │
               │       │+Fold   │截断首尾 │丢弃旧消息│LLM 摘要│
               │       │裁剪长  │         │         │        │
               │       │输出    │         │         │        │
```

### 与 ContextManager 集成

`AdaptiveCompressor` 实现了 `ContextCompressor`，可通过 `ContextManager::builder()` 直接集成（v0.2.2 新增）：

```rust
use echo_agent::compression::ContextManager;
use echo_agent::compression::levels::{AdaptiveCompressor, AdaptiveCompressionConfig};

let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig::default())
    .with_llm(llm); // 可选：启用 L4

let mut ctx = ContextManager::builder(token_limit)
    .compressor(compressor) // 现在可直接使用（之前需要 Box::new()）
    .with_system("系统提示词".to_string())
    .build();

// prepare() 会自动检测 token 并触发压缩
let result = ctx.prepare(None).await?;
// result.messages — 压缩后的消息列表
// result.compressed — 压缩统计信息（如果有）
```

### 底层 API（不通过 ContextManager 直接使用）

对于高级用例，`compress_in_place()` 直接原地修改消息：

```rust
let mut messages = vec![/* ... */];
let current_tokens = 50_000;
let target_tokens = 30_000;
let result = compressor.compress_in_place(&mut messages, current_tokens, target_tokens);
println!("应用的级别: {:?}", result.levels_applied);
```

详见 [demo53_adaptive_compression.rs](../../echo-agent-learning/tests/example_contracts/demo53_adaptive_compression.rs)。

---

## 与 Agent 集成

### 自动压缩（推荐）

配置 `AgentConfig::token_limit` 和压缩器，框架自动在每次 LLM 调用前检查并压缩：

```rust
let config = AgentConfig::new("qwen3-max", "agent", "你是一个助手")
    .token_limit(4096); // 超过 4096 token 时自动压缩

let mut agent = ReactAgent::new(config);

// 安装压缩器（默认没有，需手动设置）
agent.set_compressor(SlidingWindowCompressor::new(20)).await;

// 此后所有 execute() 调用都受到自动压缩保护
let answer = agent.execute("...").await?;
```

或使用更推荐的 Builder 模式：

```rust
use echo_agent::prelude::*;

let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .name("agent")
    .system_prompt("你是一个助手")
    .token_limit(4096)
    .build()?;

agent.set_compressor(SlidingWindowCompressor::new(20)).await;
```

### 手动触发压缩

```rust
// 使用指定压缩器强制压缩
let compressor = SlidingWindowCompressor::new(10);
let stats = agent.force_compress_with(&compressor).await?;

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
let result = ctx.prepare(None).await?;

println!("压缩后消息数: {}", result.messages.len());
```

---

## 压缩指标

> **新增于 v0.2.2。** 压缩事件的累积可观测性。

`ContextManager` 在其生命周期内跟踪压缩统计：

```rust
let metrics = ctx.compression_metrics();

println!("总压缩次数: {}", metrics.total_compressions);
println!("节省 token: {}", metrics.total_tokens_saved());
println!("压缩比率: {:.1}%", metrics.compression_ratio() * 100.0);
println!("使用的策略: {:?}", metrics.strategies_used);

// 人类可读的报告：
println!("{}", metrics.report());
// → "CompressionMetrics: 5 compressions, 12340 tokens saved (35.2%), 48 messages evicted, strategies: [SlidingWindow(3), Adaptive(2)]"

// 重置指标：
ctx.reset_compression_metrics();
```

指标在以下方法中自动记录：
- `prepare()`（自动压缩）
- `force_compress()`
- `force_compress_with()`

每次压缩事件还会发出 `tracing` 日志（`info` 级别），包含字段：`compressor`、`before_messages`、`after_messages`、`before_tokens`、`after_tokens`、`evicted`、`saved_tokens`、`elapsed_ms`。

---

## Token 估算

### 内置 Tokenizer

| 类型 | 算法 | 精度 |
|------|------|------|
| `HeuristicTokenizer` | ASCII 权重 1，CJK 权重 2，总量 / 4 | 中等（CJK/英文混合推荐） |
| `SimpleTokenizer` | `字节数 / 4 + 1` | 低（向后兼容） |

### CalibratedTokenizer（v0.2.2 新增）

`CalibratedTokenizer` 包装任意基础 Tokenizer，通过从实际 API 响应数据学习，逐步提升估算精度：

```rust
use echo_agent::tokenizer::{CalibratedTokenizer, HeuristicTokenizer, Tokenizer};
use std::sync::Arc;

let base = Arc::new(HeuristicTokenizer);
let calibrated = CalibratedTokenizer::new(base);

// 像其他 Tokenizer 一样使用
let tokens = calibrated.count_tokens("some text");

// LLM API 返回实际 token 数后，反馈校准：
calibrated.calibrate(tokens, api_usage.prompt_tokens);

// 校准因子通过指数移动平均（EMA）逐步收敛
println!("校准因子: {:.3}", calibrated.calibration_factor());
println!("样本数: {}", calibrated.sample_count());

// 与 ContextManager 配合使用：
let ctx = ContextManager::builder(4096)
    .tokenizer(Arc::new(calibrated))
    .build();
```

---

## 压缩时机

```
调用 ctx.prepare() 时：
    │
    ├─ 估算当前 token 数（通过配置的 Tokenizer）
    │
    ├─ 若 token_estimate() ≤ token_limit → 直接返回，不压缩
    │
    └─ 若 token_estimate() > token_limit → 调用 compressor.compress()
           ├─ SlidingWindow：直接截断（纳秒级）
           ├─ Summary/IncrementalSummary：调用 LLM 生成摘要（秒级，有成本）
           ├─ Hybrid：依次执行管道阶段（低于阈值时短路跳过后续）
           └─ Adaptive：按需升级 L1→L2→L3→L4→L5
    │
    └─ 记录指标 + 发出 tracing 日志事件
```

---

## 最佳实践

| 场景 | 推荐策略 |
|------|---------|
| 聊天机器人（历史不重要） | `SlidingWindowCompressor(20~50)` |
| 任务执行 Agent（历史有价值） | `SummaryCompressor` 或 `Hybrid` |
| 高频调用、成本敏感 | `SlidingWindowCompressor` |
| 长对话、需要多次压缩 | `IncrementalSummaryCompressor` |
| 长文档分析 | `HybridCompressor`（先滑动窗口，再摘要） |
| 工具密集型工作流 | `AdaptiveCompressor`（自动升级 + L1 工具折叠） |
| 测试环境 | `SlidingWindowCompressor(5)` + `token_limit: 100` |

对应示例：`echo-agent-learning/examples/demo05_compressor.rs`、`echo-agent-learning/tests/example_contracts/demo53_adaptive_compression.rs`

---

## 自定义压缩策略

`ContextCompressor` 是唯一的扩展点。框架围绕它提供两条路径：

```text
你想做什么？                          怎么做
────────────────────────────────────────────────────────
只改摘要提示词的措辞/语言/关注点     →  SummaryCompressor::with_prompt(llm, n, |msgs| ...)
降低重复压缩的 LLM 成本              →  IncrementalSummaryCompressor
修改压缩逻辑本身（消息过滤、回退       →  impl ContextCompressor
  策略、摘要放置位置、增量摘要等）
快速从一个 async fn 生成压缩器        →  #[compressor] 过程宏
```

### 自定义摘要提示词

如果你认可 `SummaryCompressor` 的分割/回退/组装逻辑，只是想改发给 LLM 的摘要指令，用 `with_prompt`：

```rust
use echo_agent::compression::compressor::SummaryCompressor;

// 英文摘要
let compressor = SummaryCompressor::with_prompt(
    llm,
    6,
    |messages| format!("Summarize the following {} messages in English", messages.len()),
);
```

### 完全自定义压缩逻辑

当 `SummaryCompressor` 的行为不满足需求（如：消息过滤、增量摘要、摘要不放入 system 消息、
不同的失败回退策略、基于 token 预算的动态分割等），直接实现 `ContextCompressor`：

```rust
use echo_agent::compression::{ContextCompressor, CompressionInput, CompressionOutput};
use echo_agent::error::Result;
use echo_agent::llm::types::Message;
use futures::future::BoxFuture;

/// 只保留用户消息的压缩器（示例）
struct UserOnlyCompressor { keep: usize }

impl ContextCompressor for UserOnlyCompressor {
    fn name(&self) -> &'static str { "UserOnly" } // 可选：用于指标追踪

    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        Box::pin(async move {
            let (system, conv): (Vec<_>, Vec<_>) = input.messages
                .into_iter()
                .partition(|m| m.role == "system");
            let user_msgs: Vec<_> = conv.into_iter()
                .filter(|m| m.role == "user")
                .collect();
            let keep = self.keep.min(user_msgs.len());
            let evicted = user_msgs[..user_msgs.len() - keep].to_vec();
            let kept = user_msgs[user_msgs.len() - keep..].to_vec();
            let mut messages = system;
            messages.extend(kept);
            Ok(CompressionOutput { messages, evicted })
        })
    }
}
```

实现 `ContextCompressor` 时，你可以调用 `default_summary_prompt(messages)` 复用内置的中文摘要模板：

```rust
use echo_agent::compression::compressor::default_summary_prompt;

let prompt = default_summary_prompt(&messages);
// prompt 是完整的摘要指令字符串，可直接发给 LLM
```

### `#[compressor]` 过程宏

从 async fn 快速生成 `ContextCompressor` 实现，无需手写 struct：

```rust
use echo_agent::compression::{CompressionInput, CompressionOutput};
use echo_agent::error::Result;
use echo_agent_macros::compressor;

#[compressor]
async fn tail_only(input: CompressionInput) -> Result<CompressionOutput> {
    let keep = 10.min(input.messages.len());
    let evicted = input.messages[..input.messages.len() - keep].to_vec();
    let messages = input.messages[input.messages.len() - keep..].to_vec();
    Ok(CompressionOutput { messages, evicted })
}
// 自动生成: struct TailOnlyCompressor; impl ContextCompressor for TailOnlyCompressor { ... }
```

### 架构总览

```text
ContextCompressor (唯一的压缩策略扩展点)
 ├── SlidingWindowCompressor       (独立实现，无外部依赖)
 ├── SummaryCompressor             (LLM 摘要 + 失败回退)
 │     ├── new()                   (使用 default_summary_prompt)
 │     └── with_prompt()           (使用自定义闭包)
 ├── IncrementalSummaryCompressor  (增量 LLM 摘要，复用上次结果)
 ├── HybridCompressor              (管道串联 + 短路优化)
 └── AdaptiveCompressor            (5 级自动升级，可选 LLM for L4)
       ├── L1: Snip + Fold         (截断/折叠工具输出)
       ├── L2: Micro               (保留首尾 N 行)
       ├── L3: Collapse            (丢弃旧消息，保留最近)
       ├── L4: Compact             (通过 .with_llm() 启用 LLM 摘要)
       └── L5: Reactive            (紧急：系统提示 + 最近 3 条)
```
