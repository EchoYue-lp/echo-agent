# AI Agent 核心模式

> **重要说明**：Plan 是投影到单一 TaskRun graph 的版本化 artifact，不是独立运行时或 Agent 类型：
> - **任务规划**：通过 `.enable_tasks()` 启用 `task_create`、`task_update`、`task_list`
> - **自我审查**：使用 `ReviewTool` 工具
> 
> 这与业界最佳实践（Hermes、Claude Code、LangGraph）保持一致，其中反思和规划是可组合的工具能力，而不是单独的 Agent 类型。

本文档介绍 echo-agent 实现的核心 AI Agent 模式，这些模式来自学术界和工业界的前沿研究。

---

## 目录

1. [ReAct 模式](#react-模式)
2. [Plan-and-Execute 模式](#plan-and-execute-模式)
3. [Self-Reflection 模式](#self-reflection-模式)
4. [LangGraph 工作流模式](#langgraph-工作流模式)
5. [模式对比与选型](#模式对比与选型)

---

## ReAct 模式

### 概念起源

ReAct (Reasoning + Acting) 由 Yao et al. 在 2022 年提出 [1]，将**推理**（Reasoning）和**行动**（Acting）交织在一起，形成 Thought → Action → Observation 的循环。

### 核心思想

```
┌─────────────────────────────────────────────────────────────┐
│                     ReAct Loop                               │
│                                                              │
│   User Query                                                │
│       │                                                      │
│       ▼                                                      │
│   ┌─────────────┐     ┌─────────────┐     ┌──────────────┐  │
│   │   Thought   │────▶│   Action    │────▶│  Observation │  │
│   │  (推理思考) │     │ (工具调用)  │     │  (执行结果)  │  │
│   └─────────────┘     └─────────────┘     └──────────────┘  │
│          ▲                                        │          │
│          └────────────────────────────────────────┘          │
│                                                              │
│   终止条件: 生成 Final Answer 或达到最大迭代次数             │
└─────────────────────────────────────────────────────────────┘
```

### echo-agent 实现

```rust
// src/agents/react/run.rs
pub async fn run_react_loop(&mut self, task: &str) -> Result<String> {
    loop {
        // 1. Think: 调用 LLM 生成下一步
        let response = self.think().await?;
        
        match response.step_type {
            StepType::FinalAnswer(answer) => {
                // 达到终止条件
                return Ok(answer);
            }
            StepType::ToolCalls(calls) => {
                // 2. Act: 执行工具
                for call in calls {
                    let result = self.tool_manager.execute(&call).await?;
                    // 3. Observe: 将结果加入上下文
                    self.context.push_tool_result(call.id, result);
                }
            }
        }
        
        // 检查最大迭代
        if iterations >= self.config.max_iterations {
            return Err(ReactError::MaxIterationsExceeded);
        }
    }
}
```

### Chain-of-Thought (CoT) 增强

echo-agent 默认启用 CoT，在工具调用前要求 LLM 先用自然语言描述思路：

```rust
const COT_INSTRUCTION: &str = "在调用工具之前，先用文字简述你的分析思路和执行计划。";
```

### 适用场景

| 场景 | 适合程度 | 原因 |
|------|----------|------|
| 多工具编排 | ★★★★★ | 动态选择工具，灵活组合 |
| 开放式问答 | ★★★★★ | 思维链可追溯，可调试 |
| 精确计算 | ★★★☆☆ | 可能因推理错误导致结果偏差 |
| 长流程任务 | ★★☆☆☆ | 迭代成本高，容易超 token |

### 参考文献

[1] Yao, S., et al. "ReAct: Synergizing Reasoning and Acting in Language Models." ICLR 2023.

---

## Plan-and-Execute 模式

### 概念起源

Plan-and-Execute 由 Wei et al. 提出 [2]，将复杂任务分解为显式的**规划**（Planning）和**执行**（Execution）两个阶段。

### 核心思想

```
┌─────────────────────────────────────────────────────────────────────┐
│                  Plan-and-Execute Pipeline                           │
│                                                                      │
│   Phase 1: Planning                                                 │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  Task → [Planner] → Plan { step_1, step_2, ..., step_n }   │   │
│   └─────────────────────────────────────────────────────────────┘   │
│                              │                                       │
│                              ▼                                       │
│   Phase 2: Execution (DAG)                                          │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │                                                              │   │
│   │     step_1 ──┬──▶ step_2 ──┬──▶ step_4 ──▶ Summary        │   │
│   │              │              │                               │   │
│   │              └──▶ step_3 ──┘                               │   │
│   │                                                              │   │
│   │  [TaskManager] 按拓扑序调度，独立步骤并行执行               │   │
│   └─────────────────────────────────────────────────────────────┘   │
│                                                                      │
│   Phase 3: Replanning (on failure)                                  │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  step_2 失败 → 识别下游受影响步骤 → 重新规划子图            │   │
│   └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

### echo-agent 实现

```rust
// src/agents/plan_execute/mod.rs

// Planner trait: 将任务转换为计划
pub trait Planner: Send + Sync {
    fn plan<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<Plan>>;
}

// Executor trait: 执行单个步骤
pub trait Executor: Send + Sync {
    fn execute_step<'a>(
        &'a mut self,
        step_description: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<String>>;
}

// Plan-and-Execute Agent
pub struct PlanExecuteAgent<P: Planner, E: Executor> {
    planner: P,
    executor: E,
    max_replans: usize,
    execution_mode: ExecutionMode, // Sequential | Parallel
}
```

### Plan → Task DAG 转换

```rust
impl Plan {
    pub fn to_task_dag(&self) -> Vec<Task> {
        self.steps.iter().enumerate().map(|(i, step)| {
            Task {
                id: format!("plan_step_{}", i),
                description: step.description.clone(),
                dependencies: step.dependencies.iter()
                    .map(|d| format!("plan_step_{}", d))
                    .collect(),
                ..Default::default()
            }
        }).collect()
    }
}
```

### 增量重规划

当某个步骤失败时，仅重新规划受影响的下游子图，而非整个计划：

```rust
// 使用 downstream_steps_recursive 识别受影响范围
let affected: Vec<usize> = plan.downstream_steps_recursive(failed_step_idx);

// 只移除受影响的任务，保留已成功的
for id in affected {
    task_manager.delete_task(&format!("plan_step_{}", id));
}

// 重新生成子计划
let new_plan = planner.plan(&replan_prompt).await?;
```

### 适用场景

| 场景 | 适合程度 | 原因 |
|------|----------|------|
| 结构化任务 | ★★★★★ | 显式步骤，可预测 |
| 多步骤流水线 | ★★★★★ | DAG 调度，可并行 |
| 需要失败恢复 | ★★★★☆ | 支持增量重规划 |
| 开放探索任务 | ★★☆☆☆ | 难以预先规划 |

### 参考文献

[2] Wei, J., et al. "Chain-of-Thought Prompting Elicits Reasoning in Large Language Models." NeurIPS 2022.

---

## Self-Reflection 模式

### 概念起源

Self-Reflection 受到 Reflexion [3] 和 CRITIC [4] 启发，通过"生成 → 评估 → 修正"闭环提升输出质量。

### 核心思想

```
┌─────────────────────────────────────────────────────────────────────┐
│                   Self-Reflection Loop                               │
│                                                                      │
│   ┌──────────────┐                                                  │
│   │    Actor     │ ─── 生成初始输出 ──────────────────────────┐     │
│   │   (生成器)   │                                           │     │
│   └──────────────┘                                           │     │
│                                                              ▼     │
│   ┌──────────────┐     ┌─────────────────┐                  │     │
│   │   Evaluator  │ ──▶ │ score < threshold?│                  │     │
│   │   (评估器)   │     └────────┬────────┘                  │     │
│   └──────────────┘              │                           │     │
│                    ┌────────────┴────────────┐              │     │
│                    │                         │              │     │
│                   Yes                       No             │     │
│                    │                         │              │     │
│                    ▼                         ▼              │     │
│   ┌──────────────┐              ┌──────────────────┐       │     │
│   │   Reflector  │              │   返回最终结果   │       │     │
│   │   (反思器)   │              └──────────────────┘       │     │
│   └──────────────┘                                         │     │
│          │                                                 │     │
│          │ 反思文本 + 修正建议                              │     │
│          ▼                                                 │     │
│   ┌──────────────┐                                         │     │
│   │   Refiner    │ ◀───────────────────────────────────────┘     │
│   │   (修正器)   │                                                  │
│   └──────────────┘                                                  │
│          │                                                          │
│          └──────────▶ 重新进入评估 ◀───────────────────────────────┘
│                                                                      │
│   Episodic Memory: 跨任务经验存储，避免重复错误                    │
└─────────────────────────────────────────────────────────────────────┘
```

### echo-agent 实现

```rust
// src/agents/self_reflection/mod.rs

pub struct SelfReflectionAgent<C: Critic> {
    generator: Box<dyn Agent>,        // Actor
    critic: C,                         // Evaluator
    refinement_prompt_builder: Box<dyn RefinementPromptBuilder>, // Reflector
    reflection_prompt_builder: Box<dyn ReflectionPromptBuilder>, // Refiner
    episodic_memory: Vec<ReflectionExperience>, // 跨任务经验
}

// Critic trait: 评估输出质量
pub trait Critic: Send + Sync {
    fn critique<'a>(
        &'a self,
        task: &'a str,
        output: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<Critique>>;
}

pub struct Critique {
    pub score: f64,           // 0.0 - 10.0
    pub passed: bool,         // score >= threshold
    pub feedback: String,     // 改进建议
    pub suggestions: Vec<String>,
}
```

### 情景记忆 (Episodic Memory)

```rust
pub struct ReflectionExperience {
    pub lesson: String,       // 从错误中学到的教训
    pub error_pattern: String,// 识别的错误模式
    pub use_count: usize,     // 引用次数（用于淘汰）
}

// 新任务时注入历史经验
fn build_memory_context(&self) -> String {
    self.episodic_memory.iter()
        .map(|exp| format!("- {}", exp.lesson))
        .collect::<Vec<_>>()
        .join("\n")
}
```

### LLM-based Critic

```rust
// src/agents/self_reflection/llm_critic.rs
pub struct LlmCritic {
    model: String,
    pass_threshold: f64,
}

impl Critic for LlmCritic {
    async fn critique(&self, task: &str, output: &str, context: &str) -> Result<Critique> {
        let prompt = format!(
            "评估以下回答的质量（0-10分）：\n任务: {}\n回答: {}\n\n请给出评分和改进建议。",
            task, output
        );
        // LLM 调用并解析结构化输出
    }
}
```

### 适用场景

| 场景 | 适合程度 | 原因 |
|------|----------|------|
| 高质量写作 | ★★★★★ | 多轮打磨，质量可控 |
| 代码生成 | ★★★★☆ | 可检测逻辑错误 |
| 事实核查 | ★★★★☆ | 评估器可验证事实 |
| 简单问答 | ★☆☆☆☆ | 过度设计，成本高 |

### 参考文献

[3] Shinn, N., et al. "Reflexion: Language Agents with Verbal Reinforcement Learning." NeurIPS 2023.

[4] Gou, Z., et al. "CRITIC: Large Language Models Can Self-Correct with Tool-Interactive Critiquing." ICLR 2024.

---

## LangGraph 工作流模式

### 概念起源

LangGraph [5] 是 LangChain 团队提出的有向图工作流框架，将 Agent 编排建模为**状态机 + 图结构**。

### 核心概念

```
┌─────────────────────────────────────────────────────────────────────┐
│                    LangGraph-style Workflow                          │
│                                                                      │
│   SharedState: 节点间共享的 KV store + 结构化消息历史              │
│                                                                      │
│   ┌─────────┐     ┌─────────┐     ┌─────────┐                      │
│   │ Node A  │────▶│ Node B  │────▶│ Node C  │                      │
│   │ (Agent) │     │(Function)│    │ (Agent) │                      │
│   └─────────┘     └─────────┘     └─────────┘                      │
│        │                                                │            │
│        │           ┌─────────┐                         │            │
│        └──────────▶│ Node D  │◀────────────────────────┘            │
│                    │(Condition)│    (条件边)                         │
│                    └─────────┘                                      │
│                         │                                           │
│                    ┌────┴────┐                                     │
│                    ▼         ▼                                     │
│               ┌────────┐ ┌────────┐                                │
│               │Node E  │ │Node F  │  (并行 fan-out)               │
│               └────────┘ └────────┘                                │
│                    │         │                                     │
│                    └────┬────┘                                     │
│                         ▼                                           │
│                    ┌────────┐                                       │
│                    │  END   │  (fan-in)                            │
│                    └────────┘                                       │
└─────────────────────────────────────────────────────────────────────┘
```

### echo-agent 实现

```rust
// src/workflow/mod.rs

// 共享状态
pub struct SharedState {
    data: HashMap<String, Value>,
    messages: Vec<Message>,
}

// 图定义
pub struct Graph {
    name: String,
    nodes: HashMap<String, Node>,
    edges: Vec<Edge>,
    entry: String,
    finish: Vec<String>,
}

// 节点类型
pub enum Node {
    Agent(SharedAgent),
    Function(BoxedFutureFn),
}

// 边类型
pub enum Edge {
    Simple { from: String, to: String },
    Conditional { from: String, condition: ConditionFn, branches: HashMap<String, String> },
}
```

### GraphBuilder DSL

```rust
let graph = GraphBuilder::new("pipeline")
    // Agent 节点
    .add_agent_node("researcher", researcher)
        .input_key("task")
        .output_key("research")
    // Function 节点
    .add_function_node("transform", |state| Box::pin(async move {
        let data: String = state.get("research").unwrap_or_default();
        state.set("report", format!("Report: {}", data));
        Ok(())
    }))
    // 条件边
    .add_conditional_edge("researcher", |state| {
        let complexity: f64 = state.get("complexity").unwrap_or(0.5);
        if complexity > 0.7 { "detailed_analysis" } else { "summary" }
    })
    // 并行 fan-out
    .add_edge("transform", "review")
    .add_edge("transform", "format")
    // fan-in
    .add_edge("review", "final")
    .add_edge("format", "final")
    .set_entry("researcher")
    .set_finish("final")
    .build()?;
```

### 工作流类型

| 类型 | 说明 | echo-agent 实现 |
|------|------|-----------------|
| Sequential | 顺序执行，前一步输出是后一步输入 | `SequentialWorkflow` |
| Concurrent | 所有 Agent 并行执行，合并结果 | `ConcurrentWorkflow` |
| DAG | 按拓扑序执行，独立节点自动并行 | `DagWorkflow` |
| Graph | LangGraph 风格，支持条件边、循环 | `Graph` |

### 声明式 YAML 工作流

```yaml
name: content_pipeline
nodes:
  - name: researcher
    type: agent
    model: qwen3-max
    system_prompt: "You are a research assistant"
    input_key: task
    output_key: research
    
  - name: writer
    type: agent
    model: qwen3-max
    system_prompt: "You are a writer"
    input_key: research
    output_key: draft
    
  - name: reviewer
    type: agent
    model: qwen3-max
    system_prompt: "You are an editor"
    input_key: draft
    output_key: final

edges:
  - from: researcher
    to: writer
  - from: writer
    to: reviewer

entry: researcher
finish: [reviewer]
```

```rust
let graph = Graph::from_yaml("workflow.yaml")?;
let result = graph.run(state).await?;
```

### 参考文献

[5] LangGraph Documentation. https://langchain-ai.github.io/langgraph/

---

## 模式对比与选型

### 决策树

```
任务类型？
│
├─ 开放式问答/工具编排 ──▶ ReAct
│   └─ 需要灵活选择工具和路径
│
├─ 结构化多步骤任务 ──▶ Plan-and-Execute
│   └─ 步骤可预定义，需要失败恢复
│
├─ 高质量输出要求 ──▶ Self-Reflection
│   └─ 需要多轮打磨和质量把关
│
└─ 复杂 Agent 编排 ──▶ Graph Workflow
    └─ 多 Agent 协作，条件分支，并行
```

### 复杂度对比

| 模式 | Token 成本 | 延迟 | 可调试性 | 适用复杂度 |
|------|-----------|------|----------|-----------|
| ReAct | 中 | 中 | ★★★★☆ | 中等 |
| Plan-and-Execute | 高 | 高 | ★★★★★ | 高 |
| Self-Reflection | 很高 | 很高 | ★★★☆☆ | 中高 |
| Graph Workflow | 可变 | 可变 | ★★★★★ | 很高 |

### 组合使用

模式可以组合：

```rust
// Self-Reflection 作为 Plan-and-Execute 的 Executor
let reflective_executor = ReflectiveExecutor::new(
    SelfReflectionAgent::new("step_agent", generator, critic)
);

let agent = PlanExecuteAgent::new("planner", planner, reflective_executor);
```

---

## 总结

echo-agent 实现了现代 AI Agent 的四大核心模式：

1. **ReAct** - 动态推理与行动，适合工具编排
2. **Plan-and-Execute** - 显式规划与执行，适合结构化任务
3. **Self-Reflection** - 质量闭环，适合高要求输出
4. **Graph Workflow** - 复杂编排，适合多 Agent 协作

选择合适的模式是构建高效 Agent 的关键。
