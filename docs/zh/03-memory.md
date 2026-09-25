# 记忆系统（Memory）

## 是什么

echo-agent 的记忆系统包含三个正交层次，每层解决不同的"记住"问题：

| 层次 | 接口 | 类比 | 解决的问题 |
|------|------|------|-----------|
| **运行时检查点** | `RuntimeStateStore` | 黑匣子 | 进程崩溃后恢复进行中的对话 |
| **历史投影** | `ConversationStore` | 聊天记录 | 用户可见的消息历史投影（驱动 GUI/TUI 历史面板） |
| **长期知识** | `Store` | 笔记本 | 跨会话保留用户偏好、领域知识、任务结果 |

运行时检查点和历史投影针对同一段对话从不同角度切入：检查点保存 ReAct 循环状态（消息 + 激活技能 + 阻塞原因），用于重启循环；历史投影是**用户可见**的消息流投影。版本化任务关系、计划 artifact 与生命周期只属于 canonical task runtime，不进入该检查点。Store 是正交的长期知识后端。

---

## 运行时检查点：RuntimeStateStore

`MemoryScope` 同样是类型化的 framework 值，文档化别名统一通过标准 `scope.parse()` API
解析。

能力档案和用户偏好档案通过稳定的 `echo_agent::profiles` facade 提供，包括
`AgentProfile`、`UserProfile` 和 `ProfileStore`。

### 解决什么问题

LLM 的上下文窗口在每次请求结束后就消失了，进程也可能在循环中途崩溃。没有运行时检查点，长任务被中断就需要从头开始；用户想在明天继续昨天的对话也只能重新输入。

`RuntimeStateStore` 在 run 推进过程中持续保存 `AgentCheckpoint` 的运行时字段（消息 + 激活技能 + 阻塞原因 + 时间戳）。下次使用同一 `conversation_id` 启动时，运行时自动恢复先前状态，实现**线程连续性**。公开的 `current_plan` 字段仍可读取旧 checkpoint，但 ReactAgent 不把它恢复成任务计划，也不会将它写入新 checkpoint。

### 工作原理

```
conversation_id: "user-123-chat-5"
                │
                ▼
FileRuntimeStateStore (./agent-data/runtime_state/_runtime_owners/):
<编码后的-runtime-id>.json
{
  "runtime_state_id": "user-123-chat-5",
  "scope_id": "user-123-chat-5",
  "phase": "active",
  "checkpoint": {
    "messages_json":  "...完整消息历史...",
    "current_plan":   null,
    "active_skills":  ["doc-writing"],
    "blocked_reason": null,
    "timestamp":      "2026-06-14T..."
  }
}
```

### 使用方式

```rust,no_run
use echo_agent::prelude::*;
use echo_agent::state::FileRuntimeStateStore;
use std::sync::Arc;

# async fn demo() -> echo_agent::error::Result<()> {
let state_store = Arc::new(FileRuntimeStateStore::new("./agent-data")?);

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

trait、文件实现和可选 SQLite 实现位于 `echo-agent/src/state/mod.rs`。

---

## 历史投影：ConversationStore

`ConversationStore` 是消息流的用户可见投影，一行一条 `StoredMessage`。框架在压缩前、工具、guard、hook 和终态安全点结算；GUI/TUI 历史面板渲染已提交的结果。

- 以稳定产品 `conversation_id` 为键；`RuntimeStateStore` 为每个 runtime generation
  使用独立 key，并将它持久绑定回该稳定 scope
- durable transcript projection 必须与 `RuntimeStateStore` 配对。可以只启用
  checkpoint，也可以两者都不启用；但只配置 `ConversationStore`、没有支持 revision
  的 `RuntimeStateStore` 时，会在 invocation 产生任何副作用前拒绝接纳。
- 内置实现：无额外依赖的 `FileConversationStore`；启用 `sqlite` feature 后也可使用
  `SqliteConversationStore`。
- `AgentConfig::persistence_settlement_timeout` 和
  `ReactAgentBuilder::persistence_settlement_timeout` 配置每个 managed 安全点的总预算（默认 10 秒，
  不允许零值）。同一 absolute deadline 传到两个 Store；后续恢复使用新预算，但不改变 durable operation identity。

```rust,no_run
use echo_agent::memory::FileConversationStore;
use echo_agent::prelude::*;
use echo_agent::state::FileRuntimeStateStore;
use std::sync::Arc;

# async fn demo() -> echo_agent::error::Result<()> {
let conv_store = Arc::new(FileConversationStore::new("./agent-data")?);
let state_store = Arc::new(FileRuntimeStateStore::new("./agent-data")?);
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .conversation_id("user-alice-conv-001")
    .conversation_store(conv_store)
    .state_store(state_store)
    .build()?;
# Ok(())
# }
```

### 文件后端的异步语义

`FileRuntimeStateStore` 与 `FileConversationStore` 保持 public API 和原有持久化
语义，但文件系统操作不再占用 Tokio runtime 线程。同一 `conversation_id` 的
操作仍严格有序，不同 conversation 可在进程级上限内并发。文件操作一旦被
接纳，即使调用方 future 被取消，owner 仍会完成不可中断的持久化写入；后续
同 conversation 操作会按顺序观察到结果。逻辑 ID 的原始 UTF-8 bytes 会编码为
无碰撞的 ASCII 文件名，文件系统大小写折叠或 Unicode normalization 不会合并
两个 conversation。损坏的 UTF-8/JSON 与文件系统错误仍返回类型化错误，不会
静默降级。

两个 `new(...)` 构造函数仍是同步 bootstrap API：它们会创建并 canonicalize
目录，`FileConversationStore::new` 还会获取 lease、核对已有 manifest。应在进入
延迟敏感的 async 路径前构造，或放进 blocking setup task；只有 async trait 方法
使用进程级文件操作 owner。

ownership 与并发设计见 [ADR 0004](../adr/0004-async-file-store-ownership.md)。
SQLite 仍是框架可选后端，并与文件后端实现相同的原子投影、deadline 和 retirement 合同。

投影不再使用 read-modify-replace，而是 prepare/apply/ack：runtime checkpoint 先保存
完整 pending batch；`ConversationStore` 原子返回 `Applied` 或 `AlreadyApplied`；随后
checkpoint CAS 推进 cursor 并清除 pending。dispatch 后 timeout 会保留 durable
`Deferred` debt，warm admission 与 cold recovery 都必须在模型执行前结算。
`TranscriptProjectionSettlement` 总是在 invocation terminal 前出现。详见
[ADR 0056](../adr/0056-durable-transcript-projection-settlement.md)。

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

Agent 记忆统一使用 `["agent", "memories"]` 命名空间。

### 工作原理

未安装 layer manager 时，Agent 只提供 `recall` 和 `search_memory`；两者只返回已批准
记忆。原始 KV 值与 Draft 仍可由直接持有 Store 的调用方检查，但不会进入 Agent 工具结果。
安装 manager 后，`remember` 创建经过 journal 的 Draft，`forget` 由 manager 结算：

```
LLM 决定记住某件事
    │
    └─► remember("斐波那契前10项: 1,1,2,3,5,8,13,21,34,55", importance=8)
            │
            └─► manager.write_memory(["agent", "memories"], uuid, Draft)
                    → 调用方审阅并激活精确 proposal

LLM 需要检索时
    │
    └─► recall("斐波那契")
            │
            └─► MemoryRecaller 搜索 ["agent", "memories"]
                    → 只返回已批准的 Active 或 Archived 记忆
```

`install_memory_layer_manager`、`set_memory_store`、`install_memory_store` 均返回
`Result`。同步安装遇到忙碌 context 时不会发布部分配置；要更换 manager 所属的 Store，
必须更换 manager。

### 使用方式

```rust,no_run
use echo_agent::prelude::*;

# async fn demo() -> echo_agent::error::Result<()> {
// 方式一：AgentConfig 注册已批准记忆的 recall/search 工具
let config = AgentConfig::new("qwen3-max", "my_agent", "你是一个助手")
    .enable_memory(true)
    .memory_path("./store.json");

let mut agent = ReactAgent::new(config);
// 安装 MemoryLayerManager 后才启用经过 journal 的 remember / forget。

// 方式二：直接操作 Store API（无需 Agent）
let store = FileStore::new("./store.json")?;

// 写入记忆
store.put(
    &["my_agent", "raw_notes"],
    "fact-001",
    serde_json::json!({ "content": "用户偏好深色主题", "importance": 7 })
).await?;

// 关键词搜索
let results = store.search(&["my_agent", "raw_notes"], "主题", 5).await?;
for item in results {
    let content = item.value["content"].as_str().unwrap_or("");
    println!("[score={:.2}] {}", item.score.unwrap_or(0.0), content);
}

// 精确获取
let item = store.get(&["my_agent", "raw_notes"], "fact-001").await?;

// 删除
store.delete(&["my_agent", "raw_notes"], "fact-001").await?;

// 列出所有 namespace
let namespaces = store.list_namespaces(None).await?;
# Ok(())
# }
```

### 已审阅的 Typed Memory

`MemoryLayerManager` 拥有框架中带来源证据的长期记忆。压缩前 LLM 抽取、被压缩
消息提取、memory trigger、分层 `remember` 与可选的 Background Review
持久化都先在统一的 `["agent", "memories"]` namespace 写入 `Draft`。
`MemoryMeta.provenance` 保存原文片段及 user、assistant 或 tool 来源角色；
`L3Promotion`、`AutoExtracted` 等 source 只表示生成机制，不等于可信来源或批准。
缺少精确用户证据的“用户偏好”不能激活或召回；部分自动提取入口会在写入 Draft 前
直接拒绝。含密钥或指令式内容的证据不会持久化。

调用方先用 `MemoryLayerManager::preview_activation(key)` 审阅 Draft，再携带
`MemoryApproval` 调用 `activate_draft(proposal, approval)`。proposal 绑定内容、
metadata 与 operation journal generation；即使发生 A→B→A，过期批准仍被拒绝。
取消或结果不明的激活由同一 manager 在重启后对账。只有已批准的 Active 或
Archived typed memory 可进入自动 context 与 Store/分层 `recall`/`search_memory`；
已批准的 Hot 记忆晋升后仍进入轮次上下文。
Draft、Superseded 和缺少 provenance 的旧记录仍可检查，但不会注入模型。
参见可执行的[分层记忆示例](../../echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs)
与 [ADR 0070](../adr/0070-memory-provenance-and-recall-authority.md)。

---

## 跨对话记忆联动

```
用户第 1 天：
  user: "我叫张三，喜欢古典音乐"
  已安装 manager 的 agent → remember("张三喜欢古典音乐")  ← Store 中的 Draft
  调用方 → preview_activation + activate_draft  ← 审阅并批准
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
// FileConversationStore 可直接使用临时目录，不需要额外 feature。
// SQLite 实现仍可在启用 `sqlite` feature 后使用。
```

---

## 上下文隔离

底层 Store API 允许复用方给不同 Agent 指定独立 namespace；
`conversation_id` 则分别隔离运行时与对话投影：

```
主 Agent    conversation_id = "main-conv-001"     namespace = ["main_agent", "memories"]
Subagent A  conversation_id = "sub-a-conv-001"    namespace = ["sub_a", "memories"]
Subagent B  conversation_id = "sub-b-conv-001"    namespace = ["sub_b", "memories"]
```

- 在此调用方自定布局中，Subagent A 无法读取 Subagent B 的记忆（不同 namespace）
- Subagent A 无法看到主 Agent 的运行时状态（不同 `conversation_id`）
- 主 Agent 持有 `Store` / `RuntimeStateStore` 对象，可显式跨 conversation / namespace 读取（用于审计）

这个例子不是 `MemoryLayerManager` 的布局。其暖层在注入的 Store 内固定使用
`WARM_NAMESPACE = ["agent", "memories"]`；若不同 Agent 的分层记忆必须隔离，
复用方需提供各自的 Store 实例或自行建立隔离边界。

---

## conversation_id 与 session_id

- `conversation_id`：持久化的对话标识。同时作为 `RuntimeStateStore`（完整运行时状态）和 `ConversationStore`（历史投影）的键。这是跨进程恢复时设置的字段。
- `session_id`：进程内 run-grouping 标签，不持久化、不参与恢复。

---

## 类型化与分层记忆（自进化）

本文档介绍的是底层的三种 Store（长期 `Store`、运行时 `RuntimeStateStore`、对话 `ConversationStore`）。
若你需要**带元数据的结构化记忆**（类型、置信度、来源）和**热/暖两层管理（Archived 留在暖层）、写入触发、审查/GC、技能自创建**等运行时演化能力，
参见 [25 - 自进化系统](./25-self-improvement.md)（`evolution` 模块）。
