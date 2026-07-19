# 多 Agent 编排 — SubAgent 与 TeamAgent

## 概述

echo-agent 提供两种多 Agent 模式：

1. **SubAgent** — 父子委托，3 种执行模式（Sync、Fork、Teammate）
2. **TeamAgent** — 对等协作，4 种策略（ManagerSubagent、Pipeline、Debate、Swarm）

两者都在 `subagent` feature flag 下。

```
┌─────────────────────────────────────────────────────────┐
│                   多 Agent 模式                           │
│                                                         │
│  ┌─────────────────────┐  ┌──────────────────────────┐  │
│  │      SubAgent        │  │       TeamAgent          │  │
│  │  (父 → 子)           │  │  (对等 ↔ 对等)            │  │
│  │                      │  │                          │  │
│  │  • Sync（阻塞）       │  │  • ManagerSubagent         │  │
│  │  • Fork（独立）       │  │  • Pipeline              │  │
│  │  • Teammate（邮箱）   │  │  • Debate                │  │
│  │                      │  │  • Swarm                 │  │
│  └─────────────────────┘  └──────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

---

## Feature 开关

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["subagent"] }
```

---

## SubAgent — 父子委托

SubAgent 是较简单的模式：父 Agent 将任务委派给子 Agent。

### 执行模式

| 模式 | 上下文继承 | 通信方式 | 使用场景 |
|------|-----------|---------|----------|
| **Sync** | 无（通过 mutex 共享状态） | 返回值 | 简单委派，阻塞等待 |
| **Fork** | 系统提示词 + 工具 + 近期历史 | 返回值 | 需要父上下文的独立子任务 |
| **Teammate** | 无 | 邮箱（async mpsc） | 并行独立工作 |

### 注册

```rust
use echo_agent::prelude::*;
use echo_agent::agent::subagent::SubagentBuilder;

let parent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("你是协调者")
    .enable_subagent()
    .subagent(
        SubagentBuilder::new("code-explorer")
            .description("探索和读取代码文件")
            .model("qwen3-max")
            .system_prompt("你是代码探索专家")
            .build()
    )
    .subagent(
        SubagentBuilder::new("web-researcher")
            .description("在网上搜索信息")
            .model("qwen3-max")
            .system_prompt("你是网络研究专家")
            .build()
    )
    .build()?;
```

### 调度工具

调用 `enable_subagent()` 后，父 Agent 自动获得 `agent_dispatch` 工具。LLM 可以调用它来委派任务：

```
用户: "读取 src/main.rs 并在网上查找相关文档"
  → Agent 调用 agent_dispatch("code-explorer", "读取 src/main.rs")
  → Agent 调用 agent_dispatch("web-researcher", "查找 src/main.rs 中模式的文档")
  → Agent 综合结果
```

---

## TeamAgent — 对等协作

TeamAgent 是高级模式：多个 Agent 作为对等节点在策略驱动下协作。

### 团队角色

| 角色 | 职责 |
|------|------|
| **Leader** | 分解任务、分配工作、综合结果 |
| **Subagent** | 执行分配的子任务 |
| **Reviewer** | 验证输出（可选） |

### 四种协作策略

#### 1. ManagerSubagent（默认）

管理者分解任务，分发给工作者，综合结果。

```rust
use echo_agent::agent::subagent::team::{TeamAgent, TeamAgentBuilder, TeamStrategy};

let team = TeamAgentBuilder::new()
    .model("qwen3-max")
    .strategy(TeamStrategy::ManagerSubagent)
    .member("researcher", "搜索相关信息", TeamRole::Subagent)
    .member("analyst", "分析发现", TeamRole::Subagent)
    .member("writer", "撰写最终报告", TeamRole::Subagent)
    .build()?;

let result = team.execute("写一份关于 Rust 异步模式的报告").await?;
```

#### 2. Pipeline

Agent 按顺序执行：每个 Agent 的输出成为下一个 Agent 的输入。

```rust
let team = TeamAgentBuilder::new()
    .model("qwen3-max")
    .strategy(TeamStrategy::Pipeline(vec![
        "researcher".into(),
        "analyst".into(),
        "writer".into(),
    ]))
    .member("researcher", "研究主题", TeamRole::Subagent)
    .member("analyst", "分析研究结果", TeamRole::Subagent)
    .member("writer", "撰写最终输出", TeamRole::Subagent)
    .build()?;
```

#### 3. Debate

多个 Agent 独立提出方案，由评判者选择最佳方案。

```rust
let team = TeamAgentBuilder::new()
    .model("qwen3-max")
    .strategy(TeamStrategy::Debate {
        judge: "judge".into(),
        debaters: vec!["architect-a".into(), "architect-b".into()],
    })
    .member("judge", "评估方案并选择最佳", TeamRole::Reviewer)
    .member("architect-a", "提出架构方案 A", TeamRole::Subagent)
    .member("architect-b", "提出架构方案 B", TeamRole::Subagent)
    .build()?;
```

#### 4. Swarm

工作按模块/文件分配给多个 Agent，由 reducer 合并发现。

```rust
let team = TeamAgentBuilder::new()
    .model("qwen3-max")
    .strategy(TeamStrategy::Swarm {
        batch_size: 3,
        reducer: "synthesizer".into(),
    })
    .member("subagent-1", "分析 src/agent/ 下的文件", TeamRole::Subagent)
    .member("subagent-2", "分析 src/tools/ 下的文件", TeamRole::Subagent)
    .member("subagent-3", "分析 src/memory/ 下的文件", TeamRole::Subagent)
    .member("synthesizer", "合并所有发现为报告", TeamRole::Reviewer)
    .build()?;
```

---

## SubAgent vs TeamAgent

| 维度 | SubAgent | TeamAgent |
|------|----------|-----------|
| 关系 | 父子 | 对等 |
| 方向 | 单向调度 | 双向协作 |
| 上下文 | 隔离（不共享） | 隔离（邮箱通信） |
| 协调方式 | 父决定 | 策略驱动 |
| 复杂度 | 简单 | 高级 |
| 使用场景 | 工具式委派 | 复杂多步工作流 |
| Feature flag | `subagent` | `subagent` |

### 何时使用哪个

- **SubAgent**：需要将特定任务委派给专业 Agent 时（类似调用工具）。父 Agent 清楚知道要问什么。
- **TeamAgent**：需要 Agent 协作完成复杂任务时，涉及分解、并行执行或辩论。

---

## 邮箱通信

TeamAgent 成员通过异步邮箱通信（tokio::sync::mpsc）：

```rust
pub enum MessageKind {
    TaskAssigned { task_id: String, task: String },
    TaskResult { task_id: String, result: String },
    Query { question: String },
    QueryResponse { answer: String },
    Status { status: String },
    Cancelled { reason: String },
}
```

每个 `TeamMember` 获得一个可配置容量的 `Mailbox`（默认：64 条消息）。

---

另见：[06 - SubAgent 编排](./06-subagent.md) 了解原始 SubAgent 文档。
