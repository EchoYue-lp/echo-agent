# echo-agent Knowledge Base

This directory contains core concepts and technical knowledge documents for the echo-agent project.

---

## Document Index

| Document | Description |
|----------|-------------|
| [Agent Patterns](./agent-patterns.md) | ReAct, Plan-and-Execute, Self-Reflection, LangGraph workflow and other AI Agent core patterns |
| [MCP Protocol](./mcp-protocol.md) | Model Context Protocol specification and echo-agent integration |
| [Skill System](./skill-system.md) | agentskills.io specification, progressive disclosure, code-based/file-based skills |
| [A2A退役与历史恢复](./a2a-protocol.md) | 历史协议资料与固定源码恢复；当前Framework不再提供该能力 |

---

## Core Concepts Quick Reference

### Agent Execution Modes

| Mode | Core Loop | Use Case |
|------|-----------|----------|
| **ReAct** | Thought → Action → Observation | Tool orchestration, open-ended Q&A |
| **Plan-and-Execute** | Plan → Execute (DAG) → Summary | Structured multi-step tasks |
| **Self-Reflection** | Generate → Critique → Refine | High-quality output guarantee |
| **Graph Workflow** | State → Node → State | Multi-agent orchestration |

### Protocol Comparison

| Protocol | Problem Solved | Layer |
|----------|---------------|-------|
| **MCP** | Tool/resource access | Tool layer |
| **A2A（历史）** | 通用协议背景，当前Framework已具名退役 | 历史资料 |
| **OpenAI Functions** | LLM tool calling format | API layer |

### Abstraction Layers

```
┌─────────────────────────────────────────┐
│              Application                 │
├─────────────────────────────────────────┤
│  A2A（历史；当前实现已退役）              │  ← 历史协议背景
├─────────────────────────────────────────┤
│  Graph Workflow / Multi-Agent           │  ← Orchestration
├─────────────────────────────────────────┤
│  Agent (ReAct / Plan-Exec / Reflection) │  ← Agent impl
├─────────────────────────────────────────┤
│  Skills (Code-based / File-based)       │  ← Capability packs
├─────────────────────────────────────────┤
│  Tools + MCP                            │  ← Tool layer
├─────────────────────────────────────────┤
│  LLM Provider (OpenAI / Anthropic / ...) │  ← Foundation
└─────────────────────────────────────────┘
```

---

## Further Reading

### Academic Papers

1. Yao, S., et al. "ReAct: Synergizing Reasoning and Acting in Language Models." ICLR 2023.
2. Wei, J., et al. "Chain-of-Thought Prompting Elicits Reasoning in Large Language Models." NeurIPS 2022.
3. Shinn, N., et al. "Reflexion: Language Agents with Verbal Reinforcement Learning." NeurIPS 2023.
4. Gou, Z., et al. "CRITIC: Large Language Models Can Self-Correct with Tool-Interactive Critiquing." ICLR 2024.

### Technical Specifications

- [LangGraph Documentation](https://langchain-ai.github.io/langgraph/)
- [MCP Specification](https://modelcontextprotocol.io/)
- [agentskills.io Specification](https://agentskills.io/specification)
- [A2A Protocol（外部历史背景，非当前Framework能力）](https://github.com/google/A2A)

### Related Frameworks

- [LangChain](https://www.langchain.com/) - Python/JS Agent framework
- [CrewAI](https://www.crewai.com/) - Multi-agent orchestration framework
- [AutoGen](https://microsoft.github.io/autogen/) - Microsoft multi-agent framework
- [LangGraph](https://langchain-ai.github.io/langgraph/) - Graph workflow framework


当前源码的A2A/LSP具名退役与保留ACP/MCP边界见[ADR0090](../../../adr/0090-a2a-lsp-retirement-contract.md)。
