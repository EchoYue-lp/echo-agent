# echo-agent 知识库

本目录包含 echo-agent 项目涉及的核心概念和技术知识文档。

---

## 文档索引

| 文档 | 描述 |
|------|------|
| [Agent 模式](agent-patterns.md) | ReAct、Plan-and-Execute、Self-Reflection、LangGraph 工作流等 AI Agent 核心模式 |
| [MCP 协议](mcp-protocol.md) | Model Context Protocol 规范与 echo-agent 集成实现 |
| [Skill 系统](skill-system.md) | agentskills.io 规范、渐进式披露、代码型/文件型技能 |
| [A2A 协议](a2a-protocol.md) | Agent-to-Agent 协议、Agent Card、任务状态机 |

---

## 核心概念速查

### Agent 执行模式

| 模式 | 核心循环 | 适用场景 |
|------|----------|----------|
| **ReAct** | Thought → Action → Observation | 工具编排、开放式问答 |
| **Plan-and-Execute** | Plan → Execute (DAG) → Summary | 结构化多步任务 |
| **Self-Reflection** | Generate → Critique → Refine | 高质量输出保证 |
| **Graph Workflow** | State → Node → State | 多 Agent 协作编排 |

### 协议对比

| 协议 | 解决问题 | 层级 |
|------|----------|------|
| **MCP** | 工具/资源访问 | Tool 层 |
| **A2A** | Agent 间通信 | Agent 层 |
| **OpenAI Functions** | LLM 工具调用格式 | API 层 |

### 抽象层级

```
┌─────────────────────────────────────────┐
│              Application                 │
├─────────────────────────────────────────┤
│  A2A Protocol (Agent-to-Agent)          │  ← Agent 互操作
├─────────────────────────────────────────┤
│  Graph Workflow / Multi-Agent           │  ← 编排层
├─────────────────────────────────────────┤
│  Agent (ReAct / Plan-Exec / Reflection) │  ← Agent 实现
├─────────────────────────────────────────┤
│  Skills (Code-based / File-based)       │  ← 能力包
├─────────────────────────────────────────┤
│  Tools + MCP                            │  ← 工具层
├─────────────────────────────────────────┤
│  LLM Provider (OpenAI / Anthropic / ...) │  ← 基础层
└─────────────────────────────────────────┘
```

---

## 扩展阅读

### 学术论文

1. Yao, S., et al. "ReAct: Synergizing Reasoning and Acting in Language Models." ICLR 2023.
2. Wei, J., et al. "Chain-of-Thought Prompting Elicits Reasoning in Large Language Models." NeurIPS 2022.
3. Shinn, N., et al. "Reflexion: Language Agents with Verbal Reinforcement Learning." NeurIPS 2023.
4. Gou, Z., et al. "CRITIC: Large Language Models Can Self-Correct with Tool-Interactive Critiquing." ICLR 2024.

### 技术规范

- [LangGraph Documentation](https://langchain-ai.github.io/langgraph/)
- [MCP Specification](https://modelcontextprotocol.io/)
- [agentskills.io Specification](https://agentskills.io/specification)
- [A2A Protocol](https://github.com/google/A2A)

### 相关框架

- [LangChain](https://www.langchain.com/) - Python/JS Agent 框架
- [CrewAI](https://www.crewai.com/) - 多 Agent 协作框架
- [AutoGen](https://microsoft.github.io/autogen/) - 微软多 Agent 框架
- [LangGraph](https://langchain-ai.github.io/langgraph/) - 图工作流框架