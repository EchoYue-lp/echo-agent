# Self-Reflection Agent —— 质量闭环

## 是什么

Self-Reflection Agent 在 ReAct 基础上引入"评估 → 反思 → 修正"闭环，通过语言反馈让 Agent 从错误中学习。

```
Actor（生成）→ Evaluator（评估）→ Reflector（反思）→ 修正 → 循环
                      ↓                              ↓
                Episodic Memory（跨任务经验存储）
```

---

## 解决什么问题

单次 LLM 响应可能存在：
- **质量不稳定**：不同运行结果质量差异大
- **无自我纠错**：无法检测和修复自己的错误
- **无学习能力**：跨任务重复犯相同错误

Self-Reflection 通过添加质量门控和迭代改进解决这些问题。

---

## 三组件模型

```rust
use echo_agent::agent::self_reflection::{SelfReflectionAgent, LlmCritic};

// 1. 生成器（Actor）：产生初始输出
let generator = ReactAgentBuilder::simple("qwen3-max", "技术文档撰写者")?;

// 2. 评估器（Critic）：打分并提供反馈
let critic = LlmCritic::new("qwen3-max")
    .with_pass_threshold(8.0);  // 0-10 分制，>= 8.0 通过

// 3. 反思 Agent：编排整个循环
let mut agent = SelfReflectionAgent::new("reflection_agent", generator, critic)
    .max_reflections(3);        // 最多 3 轮修正
```

---

## 执行流程

```
execute(task)
    │
    ├─ 1. 从过往经验构建记忆上下文
    │
    ├─ 2. 生成初始响应
    │      generator.execute(enhanced_task)
    │
    └─ Loop（max_reflections 次）:
          │
          ├─ 评估：critic.critique(task, answer)
          │     ├─ score: 0.0 - 10.0
          │     ├─ passed: score >= threshold
          │     └─ feedback: 改进建议
          │
          ├─ 若通过 → 存储成功经验 → 返回答案
          │
          └─ 若失败：
                ├─ 反思：分析哪里出错了
                ├─ 修正：生成改进后的答案
                ├─ 提取经验教训
                └─ 继续循环
```

---

## 评估器类型

### StaticCritic

简单的规则评估：

```rust
use echo_agent::agent::self_reflection::StaticCritic;

let critic = StaticCritic::always_pass();  // 总是通过（测试用）
let critic = StaticCritic::always_fail();  // 总是失败（测试用）
let critic = StaticCritic::with_threshold(7.0, |output| {
    // 自定义评分函数
    if output.len() > 100 { 7.5 } else { 5.0 }
});
```

### LlmCritic

基于 LLM 的语义评估：

```rust
use echo_agent::agent::self_reflection::LlmCritic;

let critic = LlmCritic::new("qwen3-max")
    .with_pass_threshold(8.0)
    .with_prompt(|task, output| format!(
        "评估以下回答（0-10 分）：\n任务: {}\n回答: {}\n\n请给出评分和改进建议。",
        task, output
    ));
```

### CompositeCritic

组合多个评估器：

```rust
use echo_agent::agent::self_reflection::{CompositeCritic, CompositeStrategy};

let critic = CompositeCritic::new()
    .add_critic(Box::new(length_critic))
    .add_critic(Box::new(llm_critic))
    .with_strategy(CompositeStrategy::AllMustPass);  // 或 AnyCanPass
```

---

## 情景记忆

Agent 跨任务存储学到的教训：

```rust
pub struct ReflectionExperience {
    pub lesson: String,        // 学到的教训
    pub error_pattern: String, // 识别的错误模式
    pub use_count: usize,      // 引用次数（用于淘汰）
}
```

新任务到达时，相关历史经验注入上下文：

```
原始任务: "解释 Rust 所有权"

增强任务: "解释 Rust 所有权

参考以下过往教训：
1. 讨论所有权时必须提及借用检查器
2. 移动语义需要包含代码示例
3. 避免混淆 '生命周期' 和 '作用域'"
```

---

## 使用示例

```rust
use echo_agent::prelude::*;
use echo_agent::agent::self_reflection::{SelfReflectionAgent, LlmCritic};

#[tokio::main]
async fn main() -> Result<()> {
    let generator = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("writer")
        .system_prompt("你是一个技术文档撰写专家。")
        .build()?;

    let critic = LlmCritic::new("qwen3-max")
        .with_pass_threshold(8.0);

    let mut agent = SelfReflectionAgent::new("reflection_agent", generator, critic)
        .max_reflections(3)
        .pass_threshold(7.0);

    let result = agent
        .execute("清晰准确地解释 Rust 所有权。")
        .await?;

    println!("最终结果:\n{}", result);
    Ok(())
}
```

---

## 流式事件

```rust
let mut stream = agent.execute_stream(task).await?;

while let Some(event) = stream.next().await {
    match event? {
        AgentEvent::ReflectionStart { iteration } => {
            println!("开始第 {} 轮反思", iteration + 1);
        }
        AgentEvent::CritiqueGenerated { score, passed, feedback } => {
            println!("评分: {:.1}/10 ({})", score, if passed { "通过" } else { "未通过" });
        }
        AgentEvent::Refining { iteration } => {
            println!("正在修正答案...");
        }
        AgentEvent::ReflectionEnd { iteration, score, passed } => {
            println!("第 {} 轮完成: {:.1} 分", iteration + 1, score);
        }
        AgentEvent::FinalAnswer(answer) => {
            println!("最终: {}", answer);
            break;
        }
        _ => {}
    }
}
```

---

## ReflectiveExecutor

将 Self-Reflection 作为 Plan-and-Execute 的执行器：

```rust
use echo_agent::agent::plan_execute::{PlanExecuteAgent, LlmPlanner};
use echo_agent::agent::self_reflection::{SelfReflectionAgent, LlmCritic, ReflectiveExecutor};

let generator = ReactAgentBuilder::simple("qwen3-max", "步骤执行器")?;
let critic = LlmCritic::new("qwen3-max");
let reflective_agent = SelfReflectionAgent::new("reflective", generator, critic);

let executor = ReflectiveExecutor::new(reflective_agent);
let planner = LlmPlanner::new("qwen3-max");

let mut agent = PlanExecuteAgent::new("plan_agent", planner, executor);
```

每个计划步骤都会经过质量闭环。

---

## 最佳实践

1. **设置合理阈值**：7.0-8.0 通常较好，太高会导致无限循环
2. **限制反思次数**：2-3 轮通常足够
3. **使用领域特定评估器**：为特定任务编写自定义评分函数
4. **监控情景记忆**：过大或包含陈旧教训时清理

---

## 适用场景

| 场景 | 适合程度 | 原因 |
|------|----------|------|
| 高质量写作 | ★★★★★ | 多轮打磨 |
| 代码生成 | ★★★★☆ | 可检测逻辑错误 |
| 事实核查 | ★★★★☆ | 评估器可验证事实 |
| 简单问答 | ★☆☆☆☆ | 过度设计，成本高 |

对应示例：`examples/demo20_audit.rs`