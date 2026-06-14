# 记忆系统（Memory）

## 是什么

echo-agent 的记忆系统包含三个正交层次，每层解决不同的"记住"问题：

| 层次 | 接口 | 类比 | 解决的问题 |
|------|------|------|-----------|
| **运行时检查点** | `RuntimeStateStore` | 黑匣子 | 进程崩溃后恢复进行中的对话 |
| **历史投影** | `ConversationStore` | 聊天记录 | 用户可见的消息历史投影（驱动 GUI/TUI 历史面板） |
| **长期知识** | `Store` | 笔记本 | 跨会话保留用户偏好、领域知识、任务结果 |

运行时检查点和历史投影针对同一段对话从不同角度切入：检查点保存**完整运行时状态**（消息 + 计划 + 激活技能 + 阻塞原因 + TaskNode DAG），用于重启循环；历史投影是**用户可见**的消息流投影。Store 是正交的长期知识后端。

---

## 运行时检查点：RuntimeStateStore

### 解决什么问题

LLM 的上下文窗口在每次请求结束后就消失了，进程也可能在循环中途崩溃。没有运行时检查点，长任务被中断就需要从头开始；用户想在明天继续昨天的对话也只能重新输入。

`RuntimeStateStore` 在 run 推进过程中持续保存完整的 `AgentCheckpoint`（消息 + 当前计划 + 激活技能 + 阻塞原因 + 时间戳）。下次使用同一 `conversation_id` 启动时，运行时自动恢复先前状态，实现**线程连续性**。

### 工作原理

```
conversation_id: "user-123-chat-5"
                │
                ▼
SqliteRuntimeStateStore (~/.echo-agent/state.db):
{
  "user-123-chat-5": {
    "messages_json":  "...完整消息历史...",
    "current_plan":   "Step 3: draft the haiku",
    "active_skills":  ["doc-writing"],
    "blocked_reason": null,
    "timestamp":      "2026-06-14T...",
  }
}
```

### 使用方式

```rust,no_run
use echo_agent::prelude::*;
use std::sync::Arc;

# async fn demo() -> echo_agent::error::Result<()> {
let state_store = Arc::new(SqliteRuntimeStateStore::open("./state.db").await?);

let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .conversation_id("user-alice-conv-001")  // 恢复键
    .state_store(state_store)
    .build()?;
// 首次运行：每轮收尾时持久化 AgentCheckpoint
// 再次运行（同 conversation_id）：运行时自动恢复先前状态
let _ = agent.execute("你好").await?;
# Ok(())
# }
```

trait 与 `SqliteRuntimeStateStore` 实现位于 `echo-agent/src/state/mod.rs`。

---

## 历史投影：ConversationStore

`ConversationStore` 是消息流的用户可见投影 —— 一行一条 `StoredMessage`，由 `run_core_loop` 收尾时自动写入。GUI/TUI 历史面板渲染的就是它。

- 以 `conversation_id` 为键（与 `RuntimeStateStore` 同键）
- 与 `RuntimeStateStore` 独立 —— 可单独启用、同时启用、都不启用
- 具体实现：`SqliteConversationStore`（`echo-agent/echo-state/src/memory/sqlite_conversation.rs`）

```rust,no_run
use echo_agent::prelude::*;
use std::sync::Arc;

# async fn demo() -> echo_agent::error::Result<()> {
let conv_store = Arc::new(SqliteConversationStore::open("./conversations.db").await?);
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .conversation_id("user-alice-conv-001")
    .conversation_store(conv_store)
    .build()?;
# Ok(())
# }
```

---

## 长期记忆：Store

### 解决什么问题

运行时检查点保存的是消息流，但很多信息不应该以原始对话形式存储，而是需要以结构化方式持久保存，例如：
- 用户偏好（"偏好古典音乐"）
- 领域知识（"项目代号是 OMEGA"）
- 任务成果（"分析结果：斐波那契前10项为..."）

Store 提供 `namespace + key → JSON value` 的 KV 存储，并支持关键词搜索，用于积累和检索**跨会话的知识**。

### Namespace 隔离

Store 使用 namespace（字符串数组）对数据进行逻辑隔离：

```
store.json:
├── ["math_agent", "memories"]   ← math_agent 的专属记忆
├── ["writer_agent", "memories"] ← writer_agent 的专属记忆
└── ["shared", "facts"]          ← 共享知识库
```

同一个物理文件，不同 namespace，数据完全不可互访（除非持有 Store 对象的代码显式跨 namespace 查询）。

启用 `enable_memory=true` 时，Agent 会自动使用 `[agent_name, "memories"]` 作为命名空间。

### 工作原理

Agent 通过三个内置工具操作 Store（无需手动调用 API）：

```
LLM 决定记住某件事
    │
    └─► remember("斐波那契前10项: 1,1,2,3,5,8,13,21,34,55", importance=8)
            │
            └─► store.put(["agent_name", "memories"], uuid, {
                    "content": "斐波那契前10项...",
                    "importance": 8,
                    "created_at": "2026-02-28T..."
                })

LLM 需要检索时
    │
    └─► recall("斐波那契")
            │
            └─► store.search(["agent_name", "memories"], "斐波那契", limit=5)
                    → 关键词匹配（先精确匹配，再词频相关性评分）
                    → 返回最相关的 5 条记忆
```

### 使用方式

```rust,no_run
use echo_agent::prelude::*;

# async fn demo() -> echo_agent::error::Result<()> {
// 方式一：通过 AgentConfig 自动注册 remember/recall/forget 工具
let config = AgentConfig::new("qwen3-max", "my_agent", "你是一个助手")
    .enable_memory(true)
    .memory_path("./store.json");

let mut agent = ReactAgent::new(config);
// LLM 可以自主调用 remember / recall / forget 工具

// 方式二：直接操作 Store API（无需 Agent）
let store = FileStore::new("./store.json")?;

// 写入记忆
store.put(
    &["my_agent", "memories"],
    "fact-001",
    serde_json::json!({ "content": "用户偏好深色主题", "importance": 7 })
).await?;

// 关键词搜索
let results = store.search(&["my_agent", "memories"], "主题", 5).await?;
for item in results {
    let content = item.value["content"].as_str().unwrap_or("");
    println!("[score={:.2}] {}", item.score.unwrap_or(0.0), content);
}

// 精确获取
let item = store.get(&["my_agent", "memories"], "fact-001").await?;

// 删除
store.delete(&["my_agent", "memories"], "fact-001").await?;

// 列出所有 namespace
let namespaces = store.list_namespaces(None).await?;
# Ok(())
# }
```

---

## 三层记忆联动

```
用户第 1 天：
  user: "我叫张三，喜欢古典音乐"
  agent → remember("张三喜欢古典音乐")  ← 存入 Store（跨会话永久保存）
  轮次收尾 → RuntimeStateStore 保存 AgentCheckpoint
            → ConversationStore 保存消息行

第 2 天，相同 conversation_id：
  RuntimeStateStore 恢复：agent 在先前状态上继续运行循环
  user: "推荐一首曲子"
  agent → recall("音乐偏好") → "张三喜欢古典音乐"
  → 推荐巴赫的哥德堡变奏曲

第 3 天，全新 conversation_id：
  RuntimeStateStore: 没有此键 → 全新运行时状态
  user: "推荐一首曲子"
  agent → recall("音乐偏好") → "张三喜欢古典音乐"（Store 还在！）
  → 仍然推荐古典音乐
```

---

## 内存实现（测试用）

```rust,no_run
use echo_agent::prelude::*;

let store = InMemoryStore::new(); // 进程退出后数据丢失
// 测试中需要 RuntimeStateStore / ConversationStore 时，
// 使用 SQLite 实现配合临时文件（参见 `tempfile::NamedTempFile`）或 `:memory:` SQLite URI。
```

---

## 上下文隔离

每个 Agent 都有独立的 Store namespace 和 `conversation_id`：

```
主 Agent    conversation_id = "main-conv-001"     namespace = ["main_agent", "memories"]
SubAgent A  conversation_id = "sub-a-conv-001"    namespace = ["sub_a", "memories"]
SubAgent B  conversation_id = "sub-b-conv-001"    namespace = ["sub_b", "memories"]
```

- SubAgent A 无法读取 SubAgent B 的记忆（不同 namespace）
- SubAgent A 无法看到主 Agent 的运行时状态（不同 `conversation_id`）
- 主 Agent 持有 `Store` / `RuntimeStateStore` 对象，可显式跨 conversation / namespace 读取（用于审计）

---

## conversation_id 与 session_id

- `conversation_id`：持久化的对话标识。同时作为 `RuntimeStateStore`（完整运行时状态）和 `ConversationStore`（历史投影）的键。这是跨进程恢复时设置的字段。
- `session_id`：进程内 run-grouping 标签，不持久化、不参与恢复。

对应示例：`examples/demo14_memory_isolation.rs`

