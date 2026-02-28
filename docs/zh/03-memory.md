# 记忆系统（Memory）

## 是什么

echo-agent 的记忆系统分为两个相互独立的层次，分别解决不同粒度的"记住"问题：

| 层次 | 接口 | 类比 | 解决的问题 |
|------|------|------|-----------|
| **短期记忆** | `Checkpointer` | 录音机 | 同一会话中断后可续接对话 |
| **长期记忆** | `Store` | 笔记本 | 跨会话保留领域知识和用户偏好 |

这一设计直接对应 LangGraph 的 `Checkpointer`（短期）和 `Store`（长期）两层架构。

---

## 短期记忆：Checkpointer

### 解决什么问题

LLM 的上下文窗口在每次请求结束后就消失了。如果 Agent 在处理长任务时被中断，或者用户想在明天继续昨天的对话，没有 Checkpointer 就需要从头开始。

Checkpointer 在每轮对话结束后自动将完整消息历史保存到磁盘（或内存），下次使用同一 `session_id` 启动时自动恢复，实现**对话连续性**。

### 工作原理

```
session_id: "user-123-chat-5"
                │
                ▼
checkpoints.json:
{
  "user-123-chat-5": {
    "session_id": "user-123-chat-5",
    "messages": [
      { "role": "system",    "content": "你是一个助手" },
      { "role": "user",      "content": "帮我写一首诗" },
      { "role": "assistant", "content": "..." },
      { "role": "user",      "content": "改成七言绝句" }
    ]
  }
}
```

### 使用方式

```rust
use echo_agent::prelude::*;

// 方式一：通过 AgentConfig 自动管理（推荐）
let config = AgentConfig::new("qwen3-max", "assistant", "你是一个助手")
    .session_id("user-alice-session-1")      // 指定会话 ID
    .checkpointer_path("./checkpoints.json"); // 持久化文件路径

let mut agent = ReactAgent::new(config);
// 首次运行：保存会话历史到文件
// 再次运行（同 session_id）：自动恢复上次的对话历史
let _ = agent.execute("你好").await?;

// 方式二：手动操作 Checkpointer（用于审计、跨 Agent 读取等）
let cp = FileCheckpointer::new("./checkpoints.json")?;

// 读取某个会话的历史
if let Some(checkpoint) = cp.get("user-alice-session-1").await? {
    println!("历史消息数: {}", checkpoint.messages.len());
}

// 列出所有会话
let sessions = cp.list_sessions().await?;
println!("所有会话: {:?}", sessions);

// 删除某个会话
cp.delete_session("user-alice-session-1").await?;
```

---

## 长期记忆：Store

### 解决什么问题

Checkpointer 保存的是"对话过程"（消息流），但很多信息不应该以对话形式存储，而是需要以结构化的方式持久保存，例如：
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

```rust
use echo_agent::prelude::*;

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
```

---

## 两层记忆对比

```
用户第 1 天：
  user: "我叫张三，喜欢古典音乐"
  agent → remember("张三喜欢古典音乐")  ← 存入 Store（跨会话永久保存）
  session 结束 → Checkpointer 保存对话历史

第 2 天，同一会话继续：
  Checkpointer 恢复：agent 知道昨天说了什么（"帮我写一首诗" 等历史消息）
  user: "推荐一首曲子"
  agent → recall("音乐偏好") → "张三喜欢古典音乐"
  → 推荐巴赫的哥德堡变奏曲

第 3 天，全新会话：
  Checkpointer: 没有此 session_id → 空的消息历史（不知道第 1 天说了什么）
  user: "推荐一首曲子"
  agent → recall("音乐偏好") → "张三喜欢古典音乐"（Store 还在！）
  → 仍然推荐古典音乐
```

---

## 内存实现（测试用）

```rust
use echo_agent::prelude::*;

// 内存版 Checkpointer（进程退出后数据丢失，适合测试）
let cp = InMemoryCheckpointer::new();

// 内存版 Store（适合测试）
let store = InMemoryStore::new();
```

---

## 上下文隔离

每个 Agent 都有独立的 Store namespace 和 Checkpointer session_id：

```
主 Agent    session_id = "main-001"     namespace = ["main_agent", "memories"]
SubAgent A  session_id = "sub-a-001"    namespace = ["sub_a", "memories"]
SubAgent B  session_id = "sub-b-001"    namespace = ["sub_b", "memories"]
```

- SubAgent A 无法读取 SubAgent B 的记忆（不同 namespace）
- SubAgent A 无法看到主 Agent 的对话历史（不同 session_id）
- 主 Agent 持有 `Store` 和 `Checkpointer` 对象，可以显式跨 namespace / session 读取（用于审计）

对应示例：`examples/demo14_memory_isolation.rs`
