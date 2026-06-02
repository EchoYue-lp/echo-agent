# 分层记忆系统（Tiered Memory）

> **状态：已实现。**
> `tiered` 模块提供受 Letta 记忆层级启发的四层记忆架构。记忆通过摘要和周期性反思自动向下流转，每一层都使用基于重要性的淘汰策略。

---

## 是什么

`TieredMemory` 是一个多层记忆管理器，按生命周期、访问频率和重要性自动组织记忆。与简单的 `Vec<String>` 摘要不同，每个条目都携带结构化元数据（重要性、时间戳、标签）——支持基于相关性的检索和基于重要性的上下文注入。

四个层次，从最快到最持久：

| 层 | 名称 | 存储 | 生命周期 | 用途 |
|----|------|------|----------|------|
| **Core** | 核心记忆 | 系统提示词 | 永久 | 身份、偏好、目标——始终对代理可见 |
| **ShortTerm** | 短期记忆 | 内存 `Vec` | 分钟级 | 带元数据的近期结构化条目 |
| **Overflow** | 溢出队列 | 内存 `Vec` | 分钟–小时级 | 被淘汰的短期条目，等待异步刷写 |
| **LongTerm** | 长期存储 | `Store`（数据库/文件） | 天–月级 | 可搜索的持久化情景记忆 |

```
用户对话
    │
    ▼
┌──────────────┐    淘汰        ┌──────────────┐   刷写       ┌──────────────┐
│  ShortTerm   │ ─────────────► │  Overflow    │ ───────────► │  LongTerm    │
│  (最多 N 条) │  按重要性      │  (最多 M 条) │   异步       │  (Store)     │
└──────────────┘                └──────────────┘              └──────────────┘
       │                                                          │
       ▼                                                          ▼
┌──────────────┐                                          ┌──────────────┐
│  上下文注入  │                                          │  衰减与修剪  │
│  (提示词)    │                                          │  (周期性)    │
└──────────────┘                                          └──────────────┘
       ▲
       │
┌──────────────┐
│  Core        │  ← 始终注入（系统提示词片段）
│  Memory      │
└──────────────┘
```

---

## MemoryEntry

与简单的 `String` 摘要不同，每个短期条目都是一个带有丰富元数据的 `MemoryEntry`：

```rust
pub struct MemoryEntry {
    pub content: String,          // 记忆内容
    pub importance: f64,          // 1.0–10.0；越高 = 保留越久，注入越靠前
    pub timestamp: DateTime<Utc>, // 条目创建时间
    pub tags: Vec<String>,        // 语义标签，用于关键词检索
    pub source: String,           // 来源："conversation"、"reflection"、"tool_result"、"overflow"
}
```

| 字段 | 类型 | 范围 | 用途 |
|------|------|------|------|
| `content` | `String` | — | 摘要文本（对话轮次、反思、工具结果等） |
| `importance` | `f64` | 1.0–10.0 | 控制淘汰优先级和上下文注入顺序。创建时自动钳位。 |
| `timestamp` | `DateTime<Utc>` | — | 创建时间。用于基于时长的摘要决策。 |
| `tags` | `Vec<String>` | — | 语义标签，用于关键词检索（如 `["rust", "error", "debug"]`） |
| `source` | `String` | — | 条目来源。用于过滤和溯源。 |

### 创建条目

```rust
use echo_core::memory::tiered::MemoryEntry;

// 带完整元数据的结构化条目
let entry = MemoryEntry::new(
    "在 parser.rs 中发现空指针 bug".to_string(),
    8.0,                                              // 高重要性
    vec!["rust".to_string(), "bug".to_string()],      // 标签
    "tool_result".to_string(),                        // 来源
);

// 使用默认值的简单条目（重要性 5.0，无标签，来源 "conversation"）
let simple = MemoryEntry::simple("用户偏好深色主题".to_string());
```

### 关键词匹配

`MemoryEntry::matches_keyword()` 同时搜索 `content` 和 `tags`（不区分大小写）：

```rust
let entry = MemoryEntry::new(
    "Rust 编译错误发生在模块 A".to_string(),
    7.0,
    vec!["rust".to_string(), "error".to_string()],
    "conversation".to_string(),
);

entry.matches_keyword("rust");   // true（标签匹配）
entry.matches_keyword("编译");   // true（内容匹配）
entry.matches_keyword("python"); // false
```

---

## TieredMemory 结构体

```rust
pub struct TieredMemory {
    pub core: CoreMemory,                  // 始终注入系统提示词
    pub short_term: Vec<MemoryEntry>,      // 近期结构化条目（最多 N 条）
    pub max_short_term: usize,             // 短期条目上限
    pub long_term: Option<Arc<dyn Store>>, // 可选的持久化存储
    pub overflow_queue: Vec<MemoryEntry>,  // 被淘汰的短期条目
    pub max_overflow: usize,               // 溢出队列上限（默认 100）
}
```

### 构造方式

```rust
use echo_core::memory::tiered::TieredMemory;
use std::sync::Arc;

// 基础构造：5 条短期记忆，2000 字符核心记忆预算
let memory = TieredMemory::new(5, 2000);

// 显式设置溢出上限
let memory = TieredMemory::new(5, 2000)
    .with_overflow_bound(50);

// 挂载长期存储
let store: Arc<dyn Store> = Arc::new(FileStore::new("./store.json")?);
let memory = TieredMemory::new(5, 2000)
    .with_overflow_bound(50)
    .with_store(store);

// 默认值：max_short_term=5, max_core_chars=2000, max_overflow=100
let memory = TieredMemory::default();
```

| 方法 | 用途 |
|------|------|
| `new(max_short_term, max_core_chars)` | 指定短期上限和核心记忆字符预算 |
| `with_overflow_bound(n)` | 设置溢出队列最大条数（默认 100） |
| `with_store(store)` | 挂载长期 `Store` 用于持久化 |

---

## 配置参数

| 参数 | 默认值 | 用途 |
|------|--------|------|
| `max_short_term` | 5 | 短期层最大条目数。超过时，最低重要性的条目被淘汰到溢出队列。 |
| `max_core_chars` | 2000 | 核心记忆块的总字符预算。超过时，最低重要性的块被淘汰。 |
| `max_overflow` | 100 | 溢出队列最大条目数。未挂载 Store 时超过此限制，最低重要性的条目将被永久淘汰并输出警告。 |
| `auto_summarize_threshold` | `max_short_term × 2` | 当 `short_term.len() + overflow_queue.len()` 达到此值时，应触发 LLM 摘要压缩旧条目。 |

### 容量建议

| 场景 | `max_short_term` | `max_overflow` | `max_core_chars` |
|------|------------------|----------------|------------------|
| 短对话 | 3–5 | 50 | 1000 |
| 长时间编码会话 | 10–20 | 100 | 2000 |
| 多代理协作 | 5–10 | 50 | 1500 |
| 研究/论文写作 | 15–30 | 200 | 3000 |

---

## 淘汰策略

淘汰发生在记忆层级的三个位置。三者都使用**基于重要性**的淘汰（而非纯 LRU），确保高价值记忆即使在新条目到来时也能保留。

### 1. 短期淘汰

当 `add_short_term()` 使 `short_term.len()` 超过 `max_short_term` 时，**重要性最低**的条目被移除并推入溢出队列：

```rust
let mut memory = TieredMemory::new(2, 2000);

memory.add_short_term(MemoryEntry::simple("低重要性".to_string()));              // imp 5.0
memory.add_short_term(MemoryEntry::new("高重要性".into(), 9.0, vec![], "conv".into())); // imp 9.0
// short_term: [5.0, 9.0]

memory.add_short_term(MemoryEntry::new("中重要性".into(), 7.0, vec![], "conv".into())); // imp 7.0
// short_term.len() 将为 3 > 上限 2
// 淘汰最低："低重要性" (5.0) → overflow_queue
// short_term: [9.0, 7.0], overflow: [5.0]
```

### 2. 溢出淘汰

当溢出队列达到 `max_overflow` **且未挂载长期存储**时，重要性最低的条目被永久淘汰，并通过 `tracing::warn!` 输出警告：

```
Overflow queue full (max 100), evicted entry (importance=2.0): 用户提到他喜欢...
```

当**已挂载** Store 时，溢出队列允许短暂超出上限——条目将在下次 `flush_overflow()` 调用时刷写到 Store。

### 3. 长期衰减与修剪

长期记忆使用指数衰减逐步降低旧的、未访问条目的有效重要性：

```text
effective_score = importance × e^(-λ × days_since_access)
其中 λ = 0.05（半衰期 ≈ 14 天）
```

`effective_score < 1.0` 的条目成为修剪候选：

```rust
// 识别修剪候选
let candidates = memory.prune_candidates(&items);

// 按衰减后的重要性排序并截断
let mut items = store.search(&["memories", "short_term"], "", 100).await?;
TieredMemory::rank_by_importance(&mut items, 20);
```

| 距上次访问天数 | 衰减因子 (λ=0.05) | 原始重要性 8.0 → 有效值 |
|----------------|-------------------|-------------------------|
| 0 | 1.000 | 8.00 |
| 7 | 0.705 | 5.64 |
| 14 | 0.497 | 3.97 |
| 30 | 0.223 | 1.78 |
| 60 | 0.050 | 0.40 ← 修剪候选 |
| 90 | 0.011 | 0.09 ← 修剪候选 |

---

## 溢出处理与刷写

溢出队列充当短期记忆和长期存储之间的缓冲区。调用 `flush_overflow()` 持久化条目：

```rust
// 将溢出条目刷写到长期存储
let flushed = memory.flush_overflow().await;
println!("已将 {} 条记忆持久化到长期存储", flushed);
```

### 行为矩阵

| 挂载 Store？ | 队列已满？ | 行为 |
|-------------|-----------|------|
| 否 | 否 | 条目留在有界的溢出队列中 |
| 否 | 是 | 最低重要性条目被淘汰并输出警告 |
| 是 | 否 | 条目保留直到下次刷写 |
| 是 | 是 | 队列短暂超出上限；刷写排空所有条目 |

每个刷写的条目写入 Store 的 `["memories", "short_term"]` 命名空间，key 为 `short_term_{uuid}`：

```json
{
  "content": "用户偏好 Rust 而非 Python 做后端服务",
  "importance": 7.0,
  "timestamp": "2026-06-02T10:30:00Z",
  "tags": ["rust", "preferences"],
  "source": "conversation"
}
```

---

## 上下文注入

`build_context_injection()` 将 Core + ShortTerm 记忆组装为字符串，注入到代理的系统提示词中。短期条目按**重要性降序**排列，而非 FIFO：

```rust
if let Some(injection) = memory.build_context_injection() {
    // 注入到系统提示词
    system_prompt.push_str(&injection);
}
```

输出示例：

```text
## Core Memory
- user_name: 张三
- project_goal: 构建 Rust Agent 框架

## Recent Context
1. 关键发现：parser.rs 中存在空指针 bug
2. 用户正在用 tokio 开发 Agent 框架
3. 之前关于 Rust 错误处理的对话
```

条目 #1 排在最前面，因为它的重要性为 9.0，尽管它是最后添加的。

---

## 检索

### 短期检索

按关键词搜索短期条目（同时匹配内容和标签）：

```rust
let results = memory.recall("rust", 5);
for entry in results {
    println!("[imp={:.1}] {}", entry.importance, entry.content);
}
// 结果按重要性降序排列，最多返回 5 条
```

### 长期检索

通过 Store 的关键词搜索接口查询长期存储：

```rust
if let Some(items) = memory.recall_from_long_term("解析器 bug", 10).await {
    for item in items {
        println!("[score={:.2}] {:?}", item.score, item.value["content"]);
    }
}
// 未挂载长期存储时返回 None
```

---

## 自动摘要

`TieredMemory` 跟踪待处理条目总数（短期 + 溢出），当达到阈值时表示需要 LLM 摘要压缩：

```rust
// 检查是否需要摘要
if memory.needs_summarization() {
    let entries = &memory.short_term;
    // 将旧条目发送给 LLM 进行压缩
    let summary = llm_summarize(entries).await;
    // 用压缩后的摘要替换旧条目
    memory.short_term.clear();
    memory.overflow_queue.clear();
    memory.add_short_term(MemoryEntry::new(
        summary,
        7.0,
        vec!["summary".to_string()],
        "reflection".to_string(),
    ));
}
```

| 方法 | 返回值 | 用途 |
|------|--------|------|
| `auto_summarize_threshold()` | `max_short_term × 2` | 触发摘要的条目数阈值 |
| `needs_summarization()` | `bool` | 当前 `short_term + overflow >= 阈值` 是否成立 |
| `total_pending_entries()` | `usize` | 当前 `short_term.len() + overflow_queue.len()` |

---

## 与 Agent 记忆工具的集成

`TieredMemory` 通过 `Store` trait 与代理内置的 `remember` / `recall` / `forget` 工具集成。当挂载了长期存储时，溢出条目自动持久化，并可通过 `recall` 搜索。

### 数据流

```
Agent 调用 remember("用户偏好深色主题", importance=7)
    │
    ├─► CoreMemory.upsert() 如果高重要性 (≥ 8.0)
    │       → 始终在系统提示词中可见
    │
    └─► TieredMemory.add_short_term()
            │
            ├─► 短期层有空位 → 保留在工作上下文中
            │
            └─► 短期层已满 → 淘汰到溢出队列
                    │
                    └─► flush_overflow() → Store.put(["memories", "short_term"], ...)
                            │
                            └─► 可通过 recall("深色主题") 搜索
```

### Agent 配置

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new("qwen3-max", "assistant", "你是一个有帮助的助手")
    .enable_memory(true)
    .memory_path("./store.json");

let mut agent = ReactAgent::new(config);
// Agent 现在可以使用 remember/recall/forget 工具
// TieredMemory 自动管理记忆层级
```

---

## 完整示例

```rust
use echo_core::memory::tiered::{TieredMemory, MemoryEntry};
use echo_core::memory::core_memory::CoreMemoryBlock;
use std::sync::Arc;

// 1. 创建包含所有层的分层记忆
let store: Arc<dyn echo_core::memory::Store> =
    Arc::new(echo_state::FileStore::new("./store.json").unwrap());

let mut memory = TieredMemory::new(5, 2000)
    .with_overflow_bound(50)
    .with_store(store);

// 2. 设置核心记忆（始终可见）
memory.core.upsert(
    CoreMemoryBlock::new("user", "user_name", "张三")
        .with_importance(9.0)
);
memory.core.upsert(
    CoreMemoryBlock::new("proj", "project", "Rust Agent 框架")
        .with_importance(7.0)
);

// 3. 添加结构化短期记忆
memory.add_short_term(MemoryEntry::new(
    "用户报告 parser.rs 中存在空指针 bug".to_string(),
    9.0,
    vec!["rust".to_string(), "bug".to_string()],
    "conversation".to_string(),
));
memory.add_short_term(MemoryEntry::new(
    "讨论了异步 Rust 中的错误处理模式".to_string(),
    7.0,
    vec!["rust".to_string(), "async".to_string()],
    "conversation".to_string(),
));
memory.add_short_term(MemoryEntry::simple(
    "用户询问了记忆层级配置".to_string(),
));

// 4. 构建上下文注入（按重要性排序）
if let Some(ctx) = memory.build_context_injection() {
    println!("=== 注入系统提示词 ===");
    println!("{}", ctx);
}

// 5. 按关键词检索
let rust_entries = memory.recall("rust", 10);
println!("\n=== 检索 'rust' ===");
for e in rust_entries {
    println!("[imp={:.1}] {}", e.importance, e.content);
}

// 6. 检查是否需要摘要
if memory.needs_summarization() {
    println!("\n需要摘要！当前 {} 条待处理条目", memory.total_pending_entries());
}

// 7. 将溢出刷写到长期存储
tokio::runtime::Runtime::new().unwrap().block_on(async {
    let flushed = memory.flush_overflow().await;
    println!("\n已将 {} 条记忆刷写到长期存储", flushed);

    // 8. 从长期存储检索
    if let Some(items) = memory.recall_from_long_term("bug", 5).await {
        for item in items {
            println!("[长期] {:?}", item.value["content"]);
        }
    }
});
```

---

## 从扁平记忆迁移

如果你现有的 Agent 使用扁平的 `Vec<String>` 或裸 `Checkpointer` 做记忆，以下是迁移到 `TieredMemory` 的方法。

### 迁移前：扁平记忆

```rust
// 旧方式：字符串摘要列表
let mut memories: Vec<String> = vec![];
memories.push("用户喜欢 Rust".to_string());
memories.push("项目使用 tokio".to_string());
// 没有重要性、没有淘汰、没有持久化
```

### 迁移后：分层记忆

```rust
use echo_core::memory::tiered::{TieredMemory, MemoryEntry};

let mut memory = TieredMemory::new(10, 2000).with_overflow_bound(50);

// 将现有字符串作为简单条目迁移
for s in &old_memories {
    memory.add_short_term(MemoryEntry::simple(s.clone()));
}

// 或创建带重要性的结构化条目
memory.add_short_term(MemoryEntry::new(
    "用户喜欢 Rust".to_string(),
    7.0,
    vec!["preferences".to_string(), "rust".to_string()],
    "migration".to_string(),
));
```

### 迁移清单

| 步骤 | 操作 |
|------|------|
| 1 | 用 `TieredMemory::new(max_short_term, max_core_chars)` 替换 `Vec<String>` |
| 2 | 将每个已有字符串包装为 `MemoryEntry::simple()` 或带适当重要性的 `MemoryEntry::new()` |
| 3 | 将 `max_short_term` 设为大约与旧列表相同的大小 |
| 4 | 根据想缓冲多少被淘汰条目设置 `with_overflow_bound()` |
| 5 | 如需跨会话持久化，通过 `with_store()` 挂载 `Store` |
| 6 | 用 `build_context_injection()` 替换手动上下文构建 |
| 7 | 用 `recall()` 和 `recall_from_long_term()` 替换手动关键词搜索 |
| 8 | 在 Agent 循环中添加周期性 `flush_overflow()` 调用 |

### 向后兼容

`add_short_term_simple()` 提供 `Vec<String>` 上 `push()` 的直接替代：

```rust
// 迁移前
memories.push("摘要文本".to_string());

// 迁移后
memory.add_short_term_simple("摘要文本".to_string());
// 创建 importance=5.0、无标签、source="conversation" 的 MemoryEntry
```

---

## 架构总览

```
┌─────────────────────────────────────────────────────────────────┐
│                        TieredMemory                              │
│                                                                  │
│  ┌────────────────────────────────────────────────────────┐     │
│  │  核心记忆 (CoreMemory)                                  │     │
│  │  • 固定块，始终在系统提示词中                            │     │
│  │  • 字符预算有界 (max_core_chars)                         │     │
│  │  • 淘汰：移除最低重要性的块                              │     │
│  └────────────────────────────────────────────────────────┘     │
│                                                                  │
│  ┌──────────────────────┐    ┌──────────────────────┐           │
│  │  短期记忆             │    │  溢出队列             │           │
│  │  Vec<MemoryEntry>    │───►│  Vec<MemoryEntry>    │           │
│  │  • max_short_term 上限│    │  • max_overflow 上限  │           │
│  │  • 基于重要性淘汰     │    │  • 基于重要性淘汰     │           │
│  │  • 按重要性排序注入   │    │    （无 Store 时）    │           │
│  │    上下文             │    │  • 异步刷写到长期存储 │           │
│  └──────────────────────┘    └──────────┬───────────┘           │
│                                         │                       │
│                                         ▼                       │
│  ┌────────────────────────────────────────────────────────┐     │
│  │  长期存储 (Option<Arc<dyn Store>>)                      │     │
│  │  • 持久化 KV 存储                                       │     │
│  │  • 命名空间：["memories", "short_term"]                  │     │
│  │  • 通过 recall_from_long_term() 关键词搜索               │     │
│  │  • 基于衰减的修剪（λ=0.05，半衰期 ≈ 14 天）             │     │
│  └────────────────────────────────────────────────────────┘     │
└─────────────────────────────────────────────────────────────────┘
```

---

## 参见

- [03-memory.md](03-memory.md) — Checkpointer 与 Store 基础
- [04-compression.md](04-compression.md) — 上下文窗口压缩
- [19-self-reflection.md](19-self-reflection.md) — 反思驱动的记忆更新
- [28-config-reference.md](28-config-reference.md) — 完整配置参考
