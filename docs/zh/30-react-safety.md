# ReAct 安全机制 —— 循环检测、自适应压缩与 Git 检查点

## 是什么

Agent 在自主执行多步任务时，存在三大风险：

1. **死循环**：Agent 反复执行相同的工具调用，浪费 token 且无法完成任务
2. **上下文溢出**：对话历史超出 token 限制，导致 LLM 请求失败
3. **文件破坏**：Agent 对文件进行错误写入或编辑，无法回滚

echo-agent 提供三层安全机制应对这些风险：

| 机制 | 模块 | 作用 |
|------|------|------|
| **循环检测** | `LoopDetector` | 检测重复调用、连续失败、无进展循环 |
| **自适应压缩** | `AdaptiveCompressor` | 5 级渐进压缩，防止上下文溢出 |
| **Git 检查点** | `git_checkpoint` | 文件变更前自动打 tag，支持回滚 |

---

## 1. 循环检测（Loop Detection）

### 解决什么问题

LLM 驱动的 Agent 可能陷入以下循环模式：

- **完全重复**：用相同参数反复调用同一工具（如反复 `read_file` 同一文件）
- **连续失败**：同一工具连续失败，Agent 不断重试无效操作
- **无进展**：Agent 执行了多轮迭代，但没有任何实质产出（无文件写入、无任务更新）

循环检测器 `LoopDetector` 自动识别这三种模式，及时介入。

### 三种检测策略

#### 策略 1：完全重复检测（Exact Duplicate）

追踪每次工具调用的 `(tool_name, args_json)` 组合。当同一组合被调用超过 `exact_threshold` 次时，判定为死循环，**强制终止** Agent 执行。

```
Agent 调用 read_file({"path": "a.rs"}) → 第 1 次 ✓
Agent 调用 read_file({"path": "a.rs"}) → 第 2 次 ✓
Agent 调用 read_file({"path": "a.rs"}) → 第 3 次 → Break: "Loop detected"
```

#### 策略 2：同工具连续失败（Same-Tool Failure）

追踪每个工具的连续失败次数。当同一工具连续失败 `failure_threshold` 次时，**注入警告消息**引导 Agent 换一种方法（不会强制终止，因为参数可能不同）。

```
shell("bad_cmd_1") → 失败 → streak=1
shell("bad_cmd_2") → 失败 → streak=2
shell("bad_cmd_3") → 失败 → streak=3 → Warn: "failed 3 times, consider a different approach"
```

一旦该工具执行成功，失败计数器自动重置。

#### 策略 3：无进展检测（No-Progress）

追踪自上次"进展"以来的迭代次数。以下工具的成功调用被视为"进展"：

- `edit_file`、`write_file`、`create_file`、`delete_file`
- `create_task`、`update_task`
- `git_commit`
- `shell`

当连续 `no_progress_threshold` 次迭代无任何进展时，注入警告。

### LoopDetectorConfig 配置

```rust
use echo_agent::agent::react::loop_detector::LoopDetectorConfig;

let config = LoopDetectorConfig {
    exact_threshold: 3,         // 完全重复次数上限（默认 3）
    failure_threshold: 3,       // 连续失败次数上限（默认 3）
    no_progress_threshold: 8,   // 无进展迭代次数上限（默认 8）
};
```

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `exact_threshold` | 3 | 同一 `(tool, args)` 组合被调用多少次后触发 Break |
| `failure_threshold` | 3 | 同一工具连续失败多少次后触发 Warn |
| `no_progress_threshold` | 8 | 连续多少轮迭代无进展后触发 Warn |

### 通过 AgentConfig 配置

```rust
use echo_agent::prelude::*;
use echo_agent::agent::react::loop_detector::LoopDetectorConfig;

let config = AgentConfig::new("qwen3-max", "agent", "你是一个助手")
    .enable_tool(true)
    .loop_detector(LoopDetectorConfig {
        exact_threshold: 5,          // 允许最多 5 次完全重复
        failure_threshold: 4,        // 允许最多 4 次连续失败
        no_progress_threshold: 12,   // 允许最多 12 轮无进展
    });

let agent = ReactAgent::new(config);
```

### LoopVerdict 判定结果

`LoopDetector::check()` 返回三种判定：

```rust
pub enum LoopVerdict {
    /// 一切正常，继续执行
    Continue,
    /// 注入警告消息到 Agent 上下文（不终止）
    Warn(String),
    /// 强制终止 Agent 循环
    Break(String),
}
```

**判定优先级**：Exact Duplicate（Break）> Same-Tool Failure（Warn）> No-Progress（Warn）

### 代码示例

```rust
use echo_agent::agent::react::loop_detector::{LoopDetector, LoopDetectorConfig};

let mut detector = LoopDetector::new(LoopDetectorConfig::default());

// 记录每次工具调用
detector.record_tool_call("read_file", r#"{"path":"a.rs"}"#, true);
detector.record_tool_call("read_file", r#"{"path":"b.rs"}"#, true);
detector.record_iteration();

// 检查结果
match detector.check() {
    LoopVerdict::Continue => println!("正常"),
    LoopVerdict::Warn(msg) => println!("警告: {}", msg),
    LoopVerdict::Break(msg) => println!("终止: {}", msg),
}

// 开始新任务时重置所有状态
detector.reset();
```

---

## 2. 自适应压缩（Adaptive Compression）

### 解决什么问题

与 `04-compression.md` 中介绍的 `SlidingWindowCompressor` / `SummaryCompressor` 不同，`AdaptiveCompressor`（位于 `echo-state` crate）提供**渐进式多级压缩**：从最轻量的输出裁剪到最激进的紧急压缩，逐级升级，只在必要时才使用更昂贵的策略。

### 压缩级别

| 级别 | 名称 | 触发阈值（默认） | 策略 | 是否需要 LLM |
|------|------|-------------------|------|-------------|
| L1 Snip | **Snip** | 80,000 tokens | 裁剪超大工具输出（超过 `l1_max_output_tokens` 的截断） | 否 |
| L1 Fold | **Fold** | （Snip 后执行） | 折叠连续的工具结果，保留最新 N 条 | 否 |
| L2 | **Micro** | 100,000 tokens | 截取工具输出的首尾各 N 行，删除中间部分 | 否 |
| L3 | **Collapse** | 120,000 tokens | 移除较早的消息，仅保留最近 N 条 + system 消息 | 否 |
| L4 | **Compact** | 150,000 tokens | 调用 LLM 对旧消息进行完整摘要（通过 `.with_llm()` 启用） | 是（可选） |
| L5 | **Reactive** | 紧急 | 仅保留 system 提示 + 最近 3 条消息 | 否 |

### 压缩流程

```
compress(messages, current_tokens, target_tokens):
    │
    ├─ tokens > L1 阈值且 > target？ → 裁剪超大工具输出（Snip）
    │                                → 折叠连续工具结果（Fold）
    │
    ├─ tokens > L2 阈值且 > target？ → 截取首尾行（Micro）
    │
    ├─ tokens > L3 阈值且 > target？ → 移除旧消息（Collapse）
    │
    ├─ tokens > L4 阈值且 > target 且配置了 LLM？ → LLM 摘要（Compact）
    │
    └─ tokens > L4 阈值且 > 2×target？ → 紧急模式（Reactive）

注意：L4 需要通过 .with_llm() 配置 LLM 客户端。未配置时，
      AdaptiveCompressor 跳过 L4 并降级到 L5。
      AdaptiveCompressor 同时实现了 ContextCompressor trait，
      可直接通过 ContextManager::builder().compressor() 集成。
```

### AdaptiveCompressionConfig 配置

```rust
use echo_state::compression::levels::AdaptiveCompressionConfig;

let config = AdaptiveCompressionConfig {
    l1_snip_threshold_tokens: 80_000,      // L1 Snip 触发阈值
    l1_max_output_tokens: 4_000,           // 单个工具输出最大 token 数
    l1_fold_consecutive_tools: true,       // L1 Fold: 折叠连续工具结果
    l1_fold_keep_latest: 2,               // L1 Fold: 每组保留最新 N 条
    l2_micro_threshold_tokens: 100_000,    // L2 触发阈值
    l2_keep_lines: 50,                     // 截取首尾各多少行
    l3_collapse_threshold_tokens: 120_000, // L3 触发阈值
    l3_keep_recent: 10,                    // 保留最近多少条消息
    l4_compact_threshold_tokens: 150_000,  // L4/L5 触发阈值
    l4_keep_recent: 6,                     // L4 保留消息数
};
```

### 代码示例

```rust
use echo_state::compression::levels::{AdaptiveCompressor, AdaptiveCompressionConfig};
use echo_core::llm::types::{Message, MessageContent, Role};

// 不带 LLM（L4 被跳过）：
let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig::default());

// 带 LLM（L4 启用）：
// let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig::default()).with_llm(llm);

let mut messages: Vec<Message> = vec![
    Message::system("你是一个助手"),
    Message::user("请分析这份报告..."),
    Message::assistant("好的，让我来..."),
    // ... 更多消息
];

let result = compressor.compress_in_place(
    &mut messages,
    130_000, // current_tokens: 当前估算 token 数
    80_000,  // target_tokens: 目标 token 数
);

println!("压缩前: {} tokens", result.tokens_before);
println!("压缩后: {} tokens", result.tokens_after);
println!("应用的级别: {:?}", result.levels_applied);
// 输出: ["L1:Snip", "L1:Fold", "L2:Micro", "L3:Collapse"]
```

### 与上下文管理的集成

在 Agent 层面，压缩通过 `compress_threshold_ratio` 配置自动触发：

```rust
let config = AgentConfig::new("qwen3-max", "agent", "你是一个助手")
    .token_limit(100_000)              // 上下文 token 上限
    .compress_threshold_ratio(0.2);    // 剩余空间不足 20% 时触发压缩

let agent = ReactAgent::new(config);
```

当可用 token 比例低于 `compress_threshold_ratio` 时，Agent 在调用 `llm.chat()` 前自动触发压缩。

### 各级别详细说明

**L1 Snip** — 对超过 `l1_max_output_tokens` 的 Tool 消息输出进行截断，保留前 N 个 token 对应的字符（使用字符边界安全切片，避免 UTF-8 panic），附加 `[output truncated]` 提示。

**L1 Fold** — 折叠连续的工具结果消息。保留最新 `l1_fold_keep_latest` 条消息，将较旧的替换为一条 `[L1 fold: N consecutive tool results collapsed]` 用户消息。当 `l1_fold_consecutive_tools` 为 true 时，在 Snip 后执行。

**L2 Micro** — 对 Tool 消息输出按行截取：保留首 `l2_keep_lines` 行和尾 `l2_keep_lines` 行，中间部分替换为 `[N lines truncated]`。

**L3 Collapse** — 保留所有 System 消息和最近 `l3_keep_recent` 条消息，移除中间消息，插入一条 `[Context compressed: N older messages removed]` 的 System 消息作为说明。

**L4 Compact** — 对旧消息进行完整 LLM 摘要。仅当通过 `AdaptiveCompressor::with_llm(llm)` 配置了 LLM 客户端时才激活。LLM 调用失败时，优雅降级到 L5。

**L5 Reactive** — 紧急模式，仅保留 System 消息和最近 3 条消息。插入 `[Emergency compression: context was critically large]` 提示。仅在 token 数超过 `l4_compact_threshold_tokens` 且超过目标值 2 倍时触发。

---

## 3. Git 检查点（Git Checkpoint）

### 解决什么问题

Agent 在执行文件写入、编辑、删除操作时，可能产生意外结果（如误删代码、写入错误内容）。如果项目已纳入 Git 管理，echo-agent 可以在每次文件变更前自动创建轻量级 tag，提供回滚能力。

### 工作原理

```
文件变更操作（create_file / write_file / edit_file / delete_file）
    │
    ├─ 1. 检测目标文件是否在 Git 仓库中
    │     └─ 不在 → 跳过检查点（非 Git 项目无影响）
    │
    ├─ 2. 获取当前 HEAD commit hash
    │
    ├─ 3. 创建轻量级 tag：echo-checkpoint/{timestamp}
    │
    └─ 4. 执行文件变更
```

### 核心 API

```rust
use echo_tools::git_checkpoint::{
    create_checkpoint,
    rollback_to_checkpoint,
    cleanup_old_checkpoints,
};
use std::path::Path;

// 创建检查点（在文件变更前调用）
let tag = create_checkpoint(Path::new("src/main.rs"));
// 返回: Some("echo-checkpoint/1717200000") 或 None（非 Git 仓库）

// 回滚到检查点
let success = rollback_to_checkpoint(
    Path::new("src/main.rs"),
    "echo-checkpoint/1717200000",
);

// 清理旧检查点（保留最近 N 个 tag）
cleanup_old_checkpoints(Path::new("src/main.rs"), 10);
```

### 关键特性

- **自动检测 Git 根目录**：从目标文件向上查找 `.git` 目录，自动定位仓库根目录
- **轻量级 tag**：使用 `git tag` 而非 commit，无额外提交开销
- **安全回滚**：使用 `git checkout <tag> -- .` 恢复文件到检查点状态（不影响 Git 历史）
- **自动清理**：`cleanup_old_checkpoints` 按创建时间倒序排列，删除超出保留数的旧 tag

### 注意事项

- 仅在文件位于 Git 仓库内时生效，非 Git 项目自动跳过
- tag 命名格式为 `echo-checkpoint/{unix_timestamp}`，不会与用户自定义 tag 冲突
- 回滚操作恢复的是工作区文件，不会修改 Git 分支或 HEAD 指针
- 建议在长任务结束后调用 `cleanup_old_checkpoints` 避免 tag 堆积

---

## 4. 综合配置示例

```rust
use echo_agent::prelude::*;
use echo_agent::agent::react::loop_detector::LoopDetectorConfig;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new(
        "qwen3-max",
        "safe_agent",
        "你是一个安全的代码助手。",
    )
    // 基础配置
    .enable_tool(true)
    .max_iterations(30)
    .token_limit(100_000)
    // 循环检测 — 自定义阈值
    .loop_detector(LoopDetectorConfig {
        exact_threshold: 4,
        failure_threshold: 3,
        no_progress_threshold: 10,
    })
    // 压缩 — 剩余空间不足 25% 时主动压缩
    .compress_threshold_ratio(0.25)
    // 文件安全 — 编辑前必须先读取
    .force_read_before_edit(true);

    let mut agent = ReactAgent::new(config);

    let answer = agent
        .execute("重构 src/utils.rs 中的 parse_date 函数")
        .await?;
    println!("{}", answer);
    Ok(())
}
```

---

## 5. 配置参考表

### 循环检测（LoopDetectorConfig）

| 参数 | 类型 | 默认值 | 配置方式 | 说明 |
|------|------|--------|----------|------|
| `exact_threshold` | `usize` | 3 | `AgentConfig::loop_detector()` | 完全重复检测阈值 |
| `failure_threshold` | `usize` | 3 | `AgentConfig::loop_detector()` | 连续失败检测阈值 |
| `no_progress_threshold` | `usize` | 8 | `AgentConfig::loop_detector()` | 无进展检测阈值 |

### 自适应压缩（AdaptiveCompressionConfig）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `l1_snip_threshold_tokens` | `usize` | 80,000 | L1 Snip 触发阈值 |
| `l1_max_output_tokens` | `usize` | 4,000 | 单个工具输出最大 token 数 |
| `l1_fold_consecutive_tools` | `bool` | true | L1 Fold: 折叠连续工具结果 |
| `l1_fold_keep_latest` | `usize` | 2 | L1 Fold: 每组保留最新 N 条 |
| `l2_micro_threshold_tokens` | `usize` | 100,000 | L2 Micro 触发阈值 |
| `l2_keep_lines` | `usize` | 50 | 截取首尾各多少行 |
| `l3_collapse_threshold_tokens` | `usize` | 120,000 | L3 Collapse 触发阈值 |
| `l3_keep_recent` | `usize` | 10 | Collapse 保留最近消息数 |
| `l4_compact_threshold_tokens` | `usize` | 150,000 | L4/L5 触发阈值 |
| `l4_keep_recent` | `usize` | 6 | Compact 保留最近消息数 |

### Agent 级安全配置

| 参数 | 类型 | 默认值 | 配置方式 | 说明 |
|------|------|--------|----------|------|
| `token_limit` | `usize` | `usize::MAX` | `AgentConfig::token_limit()` | 上下文 token 上限 |
| `compress_threshold_ratio` | `f64` | 0.2 | `AgentConfig::compress_threshold_ratio()` | 剩余空间低于此比例时触发压缩 |
| `max_iterations` | `usize` | 10 | `AgentConfig::max_iterations()` | 最大迭代次数（防无限循环） |
| `force_read_before_edit` | `bool` | false | `AgentConfig::force_read_before_edit()` | 编辑文件前必须先用 `read_file` 读取 |
| `max_tool_output_tokens` | `Option<usize>` | None | `AgentConfig::max_tool_output_tokens()` | 单个工具输出 token 上限，超限自动截断 |

### Git 检查点 API

| 函数 | 参数 | 返回值 | 说明 |
|------|------|--------|------|
| `create_checkpoint` | `file_path: &Path` | `Option<String>` | 创建 tag，返回 tag 名 |
| `rollback_to_checkpoint` | `file_path: &Path, tag: &str` | `bool` | 回滚到指定 tag |
| `cleanup_old_checkpoints` | `file_path: &Path, keep: usize` | `()` | 保留最近 N 个 tag，删除其余 |

---

## 相关文档

- [ReAct Agent](01-react-agent.md) — 核心执行引擎与迭代流程
- [上下文压缩](04-compression.md) — SlidingWindow / Summary / Hybrid 压缩策略
- [工具系统](02-tools.md) — 工具注册、执行与权限控制
- [配置参考](28-config-reference.md) — 完整配置参数一览
