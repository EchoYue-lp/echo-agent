# 自进化系统 — 分析、批判、进化

## 概述

自进化系统分析已完成的 Agent 运行，检测失败模式，生成改进建议，并迭代提升 Agent 性能。它将评估、轨迹分析、提示词重新生成和后台学习整合为统一系统。

```
运行 Agent → 收集轨迹 → 分析失败 → 生成建议
                                        ↓
                                    人工审查
                                        ↓
                                    应用变更 → 重新评估 → 循环
```

---

## 解决的问题

随着任务变难和边缘情况累积，Agent 性能会逐渐退化。没有系统化改进：

- **失败模式重复**：Agent 不读文件就写入、过度重试失败工具、遗漏明显工具
- **无反馈循环**：过去的失败不影响未来行为
- **无经验记忆**：每次会话从零开始
- **技能老化**：技能过时却未被发现

---

## 安全模型

所有建议都需要人工审查。系统不会自动：
- 修改核心运行时代码
- 放松安全策略
- 更改权限规则
- 发布或部署任何内容

---

## 流水线架构

```
┌─────────────────────────────────────────────────────────┐
│                    自进化流水线                            │
│                                                         │
│  ┌─────────────┐  ┌─────────────┐  ┌────────────────┐  │
│  │  EvalRunner  │  │  Analyzer   │  │PromptGenerator │  │
│  │  (评估)      │  │ (检测)      │  │  (改进)        │  │
│  └──────┬──────┘  └──────┬──────┘  └───────┬────────┘  │
│         │                │                  │            │
│  ┌──────▼────────────────▼──────────────────▼────────┐  │
│  │              ImprovementLoop                        │  │
│  │  评估 → 批判 → 建议 → 重新评估 → 追踪最优          │  │
│  └───────────────────────────────────────────────────┘  │
│                                                         │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │
│  │BackgroundRev │  │   Curator    │  │TrajectorySaver│  │
│  │(从对话中学习) │  │(技能生命周期) │  │(微调数据)     │  │
│  └──────────────┘  └──────────────┘  └──────────────┘  │
└─────────────────────────────────────────────────────────┘
```

---

## Analyzer — 失败检测

`Analyzer` 检查已完成的运行轨迹，检测常见失败模式：

```rust
use echo_agent::improve::Analyzer;

let critique = Analyzer::analyze(&run);
println!("{}", critique.format_report());
```

输出：
```
运行: run_abc123
成功: false (得分: 0.65)

发现的问题:
  - 先写后读: write 被调用 2 次，之前未调用 read_file
  - 过度重试: shell 被重试 4 次

建议:
  [提示词] tools: 添加指令: '编辑文件前务必先用 read_file 读取'
  [策略] force_read_before_edit: true — 原因: Agent 先写后读 2 次
```

### 检测的问题类型

| 问题 | 检测逻辑 | 建议 |
|------|----------|------|
| `WriteWithoutRead` | 对同一文件先调用写工具再调用读工具 | 添加先读后写提示词 |
| `ExcessiveRetries` | 同一工具错误 > 2 次 | 添加"尝试不同方法"指令 |
| `ToolErrorPattern` | 同一工具反复失败 | 为该工具生成评估用例 |
| `ContextOverflow` | 触发了上下文压缩 | 建议上下文感知提示 |
| `MissingTool` | 未使用预期工具 | 建议添加工具指令 |
| `ExcessiveToolCalls` | 一次运行 > 20 次工具调用 | 添加效率指令 |

---

## ImprovementLoop — 迭代改进

循环运行评估、批判失败、生成建议并追踪改进：

```rust
use echo_agent::improve::ImprovementLoop;

let loop_runner = ImprovementLoop {
    max_iterations: 5,
    improvement_threshold: 0.95,  // 测试得分 >= 95% 时停止
    holdout_ratio: 0.4,           // 40% 用于测试，60% 用于训练
};

let result = loop_runner.run(&cases, agent_factory, &run_store).await;

println!("最优得分: {:.2}，第 {} 次迭代", result.best_score, result.best_iteration);
for iter in &result.iterations {
    println!("  迭代 {}: 训练={:.2}, 测试={:.2}, {} 条建议",
        iter.iteration, iter.train_score, iter.eval_report.avg_score,
        iter.suggestions.len());
}
```

### 工作原理

1. **分层拆分**：按标准类型（TestPass、OutputContains 等）拆分用例，防止过拟合
2. **训练评估**：在训练集上运行 Agent
3. **批判**：加载失败运行的轨迹，用 `Analyzer` 分析
4. **生成建议**：收集并去重所有批判中的建议
5. **测试评估**：在留出集上运行 Agent（盲测——生成器看不到测试分数）
6. **追踪最优**：记录各迭代的最佳测试分数
7. **提前停止**：达到 `improvement_threshold` 时提前停止

---

## SelfEvolution — 一键启动

`SelfEvolution` 引擎是最简单的入口——一个 `.enable()` 开启全部功能：

```rust
use echo_agent::improve::SelfEvolution;

let result = SelfEvolution::new()
    .with_eval_cases(cases)
    .with_run_store(run_store)
    .max_iterations(5)
    .with_report_dir("./eval_reports")
    .enable()
    .run(|| create_agent())
    .await;

if let Some(result) = result {
    println!("最优: {:.2} (第 {} 次迭代)", result.best_score, result.best_iteration);
    // HTML 报告生成在 ./eval_reports/
    // - iter_0.html, iter_1.html, ..., final.html
}
```

---

## PromptGenerator — LLM 驱动的提示词改进

使用 LLM 基于失败分析生成改进的系统提示词：

```rust
use echo_agent::improve::PromptGenerator;

let generator = PromptGenerator::new();

let improved_prompt = generator.generate_improved_prompt(
    &agent,
    current_system_prompt,
    &critiques,
    "代码编辑",
).await;

println!("改进后的提示词:\n{}", improved_prompt);
```

---

## BackgroundReviewer — 从对话中学习

每次对话轮次后，启动后台任务提取记忆和技能更新：

```rust
use echo_agent::evolution::{BackgroundReviewer, BackgroundReviewConfig};

let reviewer = BackgroundReviewer::new(
    BackgroundReviewConfig {
        enabled: true,
        max_iterations: 8,
        review_memory: true,   // 提取用户偏好
        review_skills: true,   // 提取可复用模式
    },
    llm_client,
    Some(memory_store),
    Some(run_store),
);

// 审查已完成的运行
let outcome = reviewer.review(&run).await?;
println!("操作: {:?}", outcome.actions);
// 例如: ["已审查记忆", "建议更新技能"]
```

### 关注的信号

**记忆信号**：
- 用户人设、偏好、个人细节
- 对 Agent 行为的期望

**技能信号**：
- 用户纠正（"不要这样做"、"太啰嗦"）
- 非平凡的技术或解决方案
- 过时或缺失的技能

**忽略的内容**：
- 环境失败（缺少二进制文件）
- 瞬态错误
- 一次性任务叙述

---

## Curator — 技能生命周期管理

自动管理技能生命周期：Active → Stale → Archived

```rust
use echo_agent::improve::{Curator, CuratorConfig};

let curator = Curator::new(
    CuratorConfig {
        stale_days: 30,    // 30 天未使用 → Stale
        archive_days: 90,  // 90 天未使用 → Archived
        enabled: true,
    },
    "~/.echo-agent/curator_state.json",
);

// 使用时注册技能
curator.touch_skill("my-code-review", true)?;

// 固定重要技能（免于自动转换）
curator.pin_skill("critical-skill")?;

// 应用自动转换
let transitions = curator.apply_transitions()?;
for (name, from, to) in &transitions {
    println!("{name}: {from:?} → {to:?}");
}
```

---

## TrajectorySaver — 微调数据

将已完成的运行转换为 ShareGPT 格式的 JSONL，用于模型微调：

```rust
use echo_agent::improve::TrajectorySaver;

let saver = TrajectorySaver::default_dir()?;

// 保存已完成的运行
saver.save(&run, "qwen3-max").await?;

// 列出保存的轨迹
let entries = saver.list(Some("2026-05-29")).await?;
for entry in &entries {
    println!("{}: {} 轮, {} token, {} 次工具调用",
        entry.id, entry.conversations.len(),
        entry.token_usage, entry.tool_call_count);
}
```

---

## 使用场景

| 组件 | 何时使用 | 频率 |
|------|----------|------|
| `Analyzer` | 每次运行失败后 | 每次运行 |
| `ImprovementLoop` | 调优提示词时 | 每次会话 |
| `SelfEvolution` | 完整评估+改进循环 | 每次发布 |
| `BackgroundReviewer` | 每次对话后 | 每轮对话 |
| `Curator` | 定期技能维护 | 每天/每周 |
| `TrajectorySaver` | 收集微调数据 | 持续 |

---

## 与 Agent 的集成

自进化系统**不内置于 Agent 循环中**，而是作为独立的分析和改进 pass 外部运行。这种设计保持 Agent 轻量——大多数用户在生产环境中不需要自进化。

### Feature 开关

通过 `improve` feature flag 启用：

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["improve"] }
```

> 注意：`improve` 依赖 `eval` 评估框架。使用 `SelfEvolution` 或 `ImprovementLoop` 时，需同时启用 `eval`。

### 使用模式

```
┌─────────────────────────────────────────────────┐
│  生产环境（无自进化）                              │
│  agent.execute("执行任务").await                   │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│  自进化（独立 pass）                               │
│  SelfEvolution::new()                            │
│      .with_eval_cases(cases)                     │
│      .enable()                                   │
│      .run(agent_factory)                         │
│      .await                                      │
└─────────────────────────────────────────────────┘
```

### 与 Self-Reflection 的区别

| 维度 | Self-Reflection | Self-Improvement |
|------|----------------|------------------|
| 时机 | Agent 执行期间 | Agent 执行之后 |
| 范围 | 单任务 | 跨任务模式 |
| 反馈 | 语言（LLM 批判） | 结构化（代码分析） |
| Feature flag | `self-reflection` | `improve` |
| 集成方式 | 内置于 Agent 循环 | 外部批处理 |

另见：[24 - 评估系统](./24-eval-system.md) 了解驱动此流水线的评估框架。
