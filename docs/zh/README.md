# echo-agent 文档

> **English docs** → [docs/en/README.md](../en/README.md)

---

## 文档索引

### 核心功能

| 文档 | 功能模块 | 核心关键词 |
|------|---------|-----------|
| [01 - ReAct Agent](01-react-agent.md) | 核心执行引擎 | Thought→Action→Observation、CoT、并行工具调用、回调 |
| [02 - 工具系统](02-tools.md) | Tools | Tool trait、ToolManager、超时重试、并发限流 |
| [03 - 记忆系统](03-memory.md) | Memory | Store（长期）、RuntimeStateStore（运行时检查点）、ConversationStore（对话历史） |
| [04 - 上下文压缩](04-compression.md) | Compression | SlidingWindow、Summary、Hybrid、ContextManager |
| [05 - 人工介入](05-human-loop.md) | Human-in-the-Loop | 审批 Guard、Console/Webhook/WebSocket Provider |
| [06 - 多 Agent 编排](06-subagent.md) | SubAgent / Orchestration | Orchestrator/Subagent/Planner、上下文隔离 |
| [07 - Skill 系统](07-skills.md) | Skills | 能力包、系统提示词注入、外部 SKILL.md 加载 |
| [08 - MCP 协议](08-mcp.md) | MCP | stdio/HTTP 传输、工具适配、多服务端管理 |
| [09 - 任务规划](09-tasks.md) | Tasks / DAG | 有向无环图、拓扑排序、循环依赖检测、Mermaid 可视化 |
| [10 - 流式输出](10-streaming.md) | Streaming | execute_stream、AgentEvent、SSE、TTFT |
| [11 - 结构化输出](11-structured-output.md) | Structured Output | ResponseFormat、JsonSchema、extract()、extract_json() |
| [12 - Mock 测试工具](12-mock.md) | Testing | MockLlmClient、MockTool、MockAgent、InMemoryStore |
| [13 - 多轮对话](13-chat.md) | Chat | chat()、chat_stream()、跨轮记忆、reset() |
| [14 - 语义搜索](14-semantic-search.md) | Semantic Search | EmbeddingStore、Embedder、向量索引、余弦相似度 |
| [15 - IM Channels](15-im-channels.md) | IM Channels | QQ Bot / 飞书接入、WebSocket / Webhook、ChannelPlugin、消息路由 |

### 高级功能 (v1.0.0)

| 文档 | 功能模块 | 核心关键词 |
|------|---------|-----------|
| [17 - Graph Workflow](17-graph-workflow.md) | 图工作流 | LangGraph 风格、SharedState、条件边、fan-out/fan-in |
| [18 - Guard 系统](18-guard-system.md) | 护栏系统 | RuleGuard、LlmGuard、输入/输出过滤 |
| [20 - Web 工具](20-web-tools.md) | Web 搜索 / 网页获取 | DuckDuckGo / Brave / Tavily 搜索、HTML→文本 |
| [21 - 常用工具速查](21-common-tools.md) | Tool Guide | Web 搜索、网页抓取、浏览器自动化、数据工具 |
| [22 - 论文检索工具](22-research-tools.md) | Research | ArXiv 搜索、Semantic Scholar、PDF 下载、BibTeX 生成 |
| [23 - Hooks 系统](23-hooks.md) | Hooks | Skills hooks（31 个事件、7 种动作）、Task hooks、Subagent hooks |
| [24 - 评估系统](24-eval-system.md) | Eval | EvalCase、SuccessCriteria、LlmGrader、A/B 对比、回归套件、HTML 报告 |
| [25 - 自进化系统](25-self-improvement.md) | Improve / Evolution | Analyzer、ImprovementLoop、EvalDrivenImprovement、分层记忆、技能自创建、合并/健康/补丁、规则晋升、变更审计 |
| [26 - 多 Agent 模式](26-multi-agent.md) | SubAgent / TeamAgent | 父子委托（Sync/Fork/Teammate）、对等协作（ManagerSubagent/Pipeline/Debate/Swarm） |
| [27 - 追踪系统](27-tracing.md) | Trace | Run、RunEvent（11 种类型）、RunStore、JsonlRunStore、生命周期、密钥脱敏 |
| [28 - 配置参考](28-config-reference.md) | Config | AgentConfig、ReactAgentBuilder、ToolExecutionConfig、TokenBudgetConfig、YAML 配置、Feature Flags |
| [29 - 运行时与任务系统](29-long-running-tasks.md) | Runtime & Tasks | 统一运行时、执行序列化、DAG 编排、ProgressBridge、后台任务、定时调度 |

### 新增功能 (v0.2.1)

| 文档 | 功能模块 | 核心关键词 |
|------|---------|-----------|
| [30 - ReAct 安全机制](30-react-safety.md) | Loop Detection / Adaptive Compression | 循环检测、5 级自适应压缩、Git Checkpoint |
| [31 - LSP 集成](31-lsp-integration.md) | LSP | Language Server Protocol、代码导航、诊断、rust-analyzer |
| [32 - 插件系统](32-plugin-system.md) | Plugin | PluginManifest、PluginRegistry、PluginScope、生命周期管理 |
| [33 - Headless 模式](33-headless-mode.md) | Headless | 非交互执行、CI/CD 集成、JSON 输出、exit_code |
| [34 - Git 隔离](34-git-isolation.md) | Git Worktree / Checkpoint | 并行子代理隔离、worktree 管理、文件操作回滚 |
| [35 - 流水线](35-pipelines.md) | Data Pipeline / Writing Pipeline | 代码优先可复现分析、写作质量循环 |
| [36 - 数据质量与统计](36-data-quality-statistics.md) | Data Quality / Statistics | 数据画像、异常检测、描述统计、相关性分析 |
| [37 - 代码搜索](37-code-search.md) | Code Search | Ripgrep、结构化输出、glob/type 过滤、50KB 上限 |
| [38 - Agent 工厂与模式](38-factory-modes.md) | Agent Factory / Mode Engine / Prompt Templates | 模式切换、本地化、模板渲染 |
| [40 - Context 系统](40-context-system.md) | Context System | ContextAssembler、ContextBudgeter、ContextSelector、优先级排序、预算感知 |

### 入门指南

| 文档 | 说明 |
|------|------|
| [快速入门](getting-started.md) | 从零开始构建你的第一个 Agent |
| [Skill 创作指南](skill-authoring.md) | 创建自定义 Code-based 和 File-based Skill |

### 安全

| 文档 | 功能模块 | 核心关键词 |
|------|---------|-----------|
| [安全指南](security.md) | Security | 安全模型、沙箱配置、密钥管理、MCP 信任边界 |
| [工具权限系统](tool-permissions.md) | Tool Permissions | ToolPermission、PermissionMode、RuleRegistry、deny-first、ToolRiskClassifier |

### 知识库

参见 [知识库](../knowledge/zh/README.md) 获取深入概念讲解：
- [Agent 模式](../knowledge/zh/agent-patterns.md) — ReAct、Plan-and-Execute、Self-Reflection、Graph Workflow
- [MCP 协议](../knowledge/zh/mcp-protocol.md) — Model Context Protocol 规范
- [Skill 系统设计](../knowledge/zh/skill-system.md) — agentskills.io 规范对齐
- [A2A 协议](../knowledge/zh/a2a-protocol.md) — Agent-to-Agent 通信

---

## 快速上手

### 单次任务模式（`execute`）

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("qwen3-max", "assistant", "你是一个有帮助的助手");
    let mut agent = ReactAgent::new(config);
    let answer = agent.execute("你好，介绍一下自己").await?;
    println!("{}", answer);
    Ok(())
}
```

### 多轮对话模式（`chat`）

`chat()` 在现有上下文上追加消息，天然支持多轮连续对话；
`execute()` 每次都会重置上下文，适合独立的单次任务。

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("qwen3-max", "assistant", "你是一个有帮助的助手");
    let mut agent = ReactAgent::new(config);

    let r1 = agent.chat("你好，我叫小明，是一名 Rust 程序员。").await?;
    println!("Agent: {r1}");

    let r2 = agent.chat("你还记得我的名字吗？").await?;
    println!("Agent: {r2}"); // Agent 能记住上下文中的"小明"

    agent.reset(); // 清除历史，开启新会话
    Ok(())
}
```

---

## 架构总览

```
┌─────────────────────────────────────────────────────────┐
│                      用户 / 应用                          │
└────────────────────────┬────────────────────────────────┘
                         │ execute() / execute_stream()   （单次任务，每次重置上下文）
                         │ chat()    / chat_stream()      （多轮对话，保留历史）
┌────────────────────────▼────────────────────────────────┐
│                    ReactAgent                            │
│                                                         │
│  ┌──────────────┐  ┌────────────┐  ┌─────────────────┐  │
│  │ContextManager│  │ToolManager │  │  SkillManager   │  │
│  │ (压缩/历史)  │  │(注册/执行) │  │ (Skill 元数据)  │  │
│  └──────────────┘  └────────────┘  └─────────────────┘  │
│                                                         │
│  ┌──────────────┐  ┌────────────┐  ┌─────────────────┐  │
│  │  RuntimeState│  │   Store    │  │HumanApprovalMgr │  │
│  │ (运行时恢复) │  │(长期记忆)  │  │  (审批 Guard)   │  │
│  └──────────────┘  └────────────┘  └─────────────────┘  │
│                                                         │
│  ┌──────────────────────────────────────────────────┐   │
│  │              SubAgent 注册表                      │   │
│  └──────────────────────────────────────────────────┘   │
└────────────────────────┬────────────────────────────────┘
                         │ HTTP (OpenAI API)
┌────────────────────────▼────────────────────────────────┐
│                  LLM Provider                            │
└─────────────────────────────────────────────────────────┘
```

---

## 功能矩阵

| 功能 | API / 配置字段 | 默认值 |
|------|---------------|--------|
| 单次任务执行 | `execute()` / `execute_stream()` | — |
| **多轮对话** | **`chat()` / `chat_stream()`** | — |
| 工具调用 | `enable_tool` | `true` |
| DAG 任务规划 | `enable_task` | `false` |
| SubAgent 编排 | `enable_subagent` | `false` |
| 长期记忆 (Store) | `enable_memory` | `false` |
| 人工介入 | `enable_human_in_loop` | `false` |
| Chain-of-Thought 提示词 | `enable_cot` | `true` |
| 上下文压缩 | 通过 `set_compressor()` | 无 |
| 线程持久化 / 恢复 | `conversation_id` + `state_store`（`RuntimeStateStore`） | 无 |
| transcript/history 投影 | `conversation_id` + `ConversationStore` | 无 |

---

## 示例文件

所有示例按 `验收样例`、`条件验收样例`、`教学示例` 三类维护。
完整分层清单和维护规则见 `examples/README.md`。

| 示例 | 演示功能 |
|------|---------|
| `examples/demo01_tools.rs` | 基础工具注册与调用 |
| `examples/demo02_tasks.rs` | DAG 任务规划 |
| `examples/demo03_approval.rs` | 人工审批 |
| `examples/demo04_subagent.rs` | SubAgent 编排 |
| `examples/demo05_compressor.rs` | 上下文压缩 |
| `examples/demo06_mcp.rs` | MCP 协议集成 |
| `examples/demo07_skills.rs` | Skill 系统 |
| `examples/demo08_external_skills.rs` | 外部 SKILL.md 加载 |
| `examples/demo09_file_shell.rs` | 文件和 Shell 工具 |
| `examples/demo10_streaming.rs` | 流式输出 |
| `examples/demo11_callbacks.rs` | 生命周期回调 |
| `examples/demo12_resilience.rs` | 容错与重试 |
| `examples/demo13_tool_execution.rs` | 工具执行配置 |
| `examples/demo14_memory_isolation.rs` | 记忆隔离与上下文隔离 |
| `examples/demo15_structured_output.rs` | 结构化输出（extract / JSON Schema） |
| `examples/demo16_testing.rs` | Mock 测试基础设施（零真实 LLM 调用） |
| `examples/demo17_chat.rs` | 多轮对话（chat / chat_stream / reset） |
| `examples/demo18_semantic_memory.rs` | Store 语义搜索（EmbeddingStore / 向量检索） |
| `examples/demo19_guard.rs` | Guard 系统（规则 / LLM 内容过滤） |
| `examples/demo20_audit.rs` | 审计日志 |
| `examples/demo21_handoff.rs` | Agent 切换（Handoff） |
| `examples/demo22_plan_execute.rs` | Plan-and-Execute 规划执行 |
| `examples/demo23_a2a.rs` | A2A 协议 |
| `examples/demo24_topology.rs` | 多 Agent 拓扑可视化 |
| `examples/demo25_macros.rs` | 宏系统综合展示 |
| `examples/demo26_provider_factory.rs` | 动态 LLM 工厂 |
| `examples/demo27_sqlite_memory.rs` | SQLite 持久化记忆 |
| `examples/demo28_workflow.rs` | 工作流管道 |
| `examples/demo29_sandbox.rs` | 沙箱执行 |
| `examples/demo30_mcp_server.rs` | MCP 服务端模式 |
| `examples/demo31_memory_tools.rs` | 记忆工具注入 |
| `examples/demo32_token_budget.rs` | Token 预算控制 |
| `examples/demo33_retry_policy.rs` | 统一重试策略 |
| `examples/demo34_workflow_stream.rs` | 工作流流式输出 |
| `examples/demo35_dynamic_tools.rs` | 动态工具管理 |
| `examples/demo36_multimodal.rs` | 多模态消息 |
| `examples/demo37_declarative_workflow.rs` | YAML/JSON 声明式工作流 |
| `examples/demo38_im_channels.rs` | IM 频道集成 |
| `examples/demo39_workflow.rs` | 图工作流引擎 |
| `examples/demo40_snapshot.rs` | 快照与回滚 |
| `examples/demo41_web_tools.rs` | Web 搜索与页面获取 |
| `examples/demo42_playwright_mcp.rs` | Playwright MCP 浏览器自动化 |
| `examples/demo43_data_tools.rs` | 数据工具（Excel / CSV / Word / 文本） |
| `examples/demo44_code_laboratory.rs` | 代码执行助手 |
| `examples/demo45_customer_service.rs` | 智能客服 |
| `examples/demo46_data_analyst.rs` | 数据分析助手 |
| `examples/demo47_enterprise.rs` | 企业工作流自动化 |
| `examples/demo48_personal_assistant.rs` | 个人智能助理 |
| `examples/demo49_research_agent.rs` | 研究与报告助手 |
| `examples/demo50_eval.rs` | 评估系统：用例、标准、约束、轨迹回放、触发准确率、HTML 报告 |
| `examples/demo51_self_improvement.rs` | 自进化：Analyzer 失败检测、Curator 技能生命周期、TrajectorySaver 微调数据 |
