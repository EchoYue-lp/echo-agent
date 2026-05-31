# 评估系统 — 可量化、可重复的 Agent 测试

## 概述

评估系统提供了结构化的框架，用于定义测试用例、对 Agent 运行测试并评分结果。它基于 trace 系统，将评估结果与执行轨迹关联，便于调试和回归检测。

```
定义用例 → 运行 Agent → 评分结果 → 生成报告
                ↓
          Trace 关联 (run_id, 工具调用, token 数, 文件变更)
                ↓
          HTML 报告 / A/B 对比 / 回归套件
```

---

## 解决的问题

没有结构化评估系统时，Agent 质量靠"感觉"判断——手动抽查无法规模化，也无法检测回归：

- **无法量化质量**：成功标准主观且不一致
- **无法检测回归**：提示词变更导致 10 个用例中 3 个失败却未被发现
- **无法 A/B 对比**：无法客观比较两种提示词策略
- **无法约束行为**：Agent 写入禁止路径或过度调用工具未被检测

---

## 核心概念

### EvalCase

单个测试用例——测试什么以及如何判断成功：

```rust
use echo_agent::eval::{EvalCase, SuccessCriteria, EvalConstraints};

let case = EvalCase {
    id: "read_before_edit_001".into(),
    name: "先读后改".into(),
    description: "Agent 应在编辑文件前先读取".into(),
    task: "修复 src/main.rs 中的拼写错误".into(),
    project_fixture: Some(PathBuf::from("fixtures/main_with_typo")),
    success_criteria: SuccessCriteria::AllOf(vec![
        SuccessCriteria::ToolUsed { tool_name: "read_file".into() },
        SuccessCriteria::TestPass { command: "cargo test".into() },
    ]),
    constraints: EvalConstraints {
        required_read_before_edit: true,
        max_files_changed: Some(1),
        forbidden_paths: vec!["Cargo.toml".into()],
        ..Default::default()
    },
};
```

### SuccessCriteria（成功标准）

七种标准类型，支持灵活评估：

| 标准 | 说明 | 使用场景 |
|------|------|----------|
| `TestPass` | Shell 命令退出码为 0 | `cargo test`、`pytest`、`npm test` |
| `OutputContains` | 最终输出包含子串 | 简单关键字检查 |
| `ToolUsed` | Agent 调用了指定工具 | 验证工具选择 |
| `ToolNotUsed` | Agent 未调用指定工具 | 验证工具回避 |
| `AllOf` | 所有子标准通过 | 组合检查 |
| `AnyOf` | 至少一个子标准通过 | 灵活替代 |
| `LlmGraded` | LLM-as-Judge 断言 | 语义质量检查 |
| `SweBench` | 克隆仓库、应用补丁、运行测试 | 基准测试风格 |

### EvalConstraints（评估约束）

Agent 必须遵守的护栏：

```rust
EvalConstraints {
    max_files_changed: Some(3),           // 限制影响范围
    max_tool_calls: Some(15),             // 防止失控循环
    forbidden_paths: vec![".env".into()], // 保护敏感文件
    required_read_before_edit: true,      // 强制先读后写
}
```

---

## EvalRunner（评估运行器）

运行器执行用例并收集结果：

```rust
use echo_agent::eval::EvalRunner;
use std::path::PathBuf;

let runner = EvalRunner::new(PathBuf::from("/tmp/eval_workspace"))
    .with_run_store(run_store)        // 用于 trace 关联
    .with_grader(grader, grading_agent); // 用于 LlmGraded 标准

// 运行单个用例
let result = runner.run(&case, &agent).await;
println!("得分: {:.2}, 成功: {}", result.score, result.success);

// 运行所有用例，每个用例使用新 Agent
let report = runner.run_all(&cases, || create_agent()).await;
println!("通过: {}/{}", report.passed, report.total);
```

运行器自动完成：
- 运行前将 `project_fixture` 复制到临时工作区
- 通过 `run_id` 将结果关联到执行轨迹
- 从轨迹填充指标（工具调用、token 数、文件变更）
- 根据轨迹评估约束

---

## LlmGrader — LLM 评判

用于无法用简单子串检查表达的语义质量评估：

```rust
use echo_agent::eval::{LlmGrader, grader::Assertion};

let grader = LlmGrader::new();

let assertions = vec![
    Assertion {
        id: "准确性".into(),
        check: "解释是否正确描述了 Rust 所有权".into(),
        expected: "提到所有三条所有权规则".into(),
    },
    Assertion {
        id: "清晰度".into(),
        check: "解释是否清晰且结构良好".into(),
        expected: "使用代码示例和类比".into(),
    },
];

let report = grader.grade(&grading_agent, task, output, &assertions).await;
println!("通过率: {:.0}%", report.pass_rate * 100.0);

for result in &report.results {
    println!("  [{}] {} (置信度: {:.1})",
        if result.passed { "通过" } else { "失败" },
        result.assertion_id,
        result.confidence,
    );
}
```

---

## A/B 对比

在相同用例上比较两种 Agent 配置：

```rust
use echo_agent::eval::AbComparator;

let comparison = AbComparator::compare(
    &cases,
    || create_agent_with_prompt_v1(),
    || create_agent_with_prompt_v2(),
).await;

println!("{}", AbComparator::format_summary(&comparison));
// 输出:
// A/B 对比结果:
//   基线:    0.7200 平均 (n=10)
//   实验组:  0.8500 平均 (n=10)
//   差值: +0.1300 → 改善
//   改善: 6  退化: 1  无变化: 3
```

---

## 回归套件

从过去的成功运行构建评估用例，检测回归：

```rust
use echo_agent::eval::RegressionSuite;

// 从历史轨迹构建
let suite = RegressionSuite::from_traces(&past_runs);
println!("从轨迹生成 {} 个回归用例", suite.len());

// 添加手动用例
let suite = suite.with_case(EvalCase { /* ... */ });

// 运行回归检查
let report = suite.run_all(&runner, &agent).await;
if report.failed > 0 {
    println!("回归: {} 个之前通过的用例现在失败!", report.failed);
}
```

---

## 轨迹回放

离线分析过去的运行，无需重新运行 Agent：

```rust
use echo_agent::eval::TrajectoryReplay;

let replay = TrajectoryReplay::new(run);

// 分析工具使用
let counts = replay.tool_call_counts();
for (tool, count) in &counts {
    println!("  {tool}: {count} 次调用");
}

// 检测策略违规
let violations = replay.detect_write_without_read();
for v in &violations {
    println!("违规: {v}");
}

// 生成指标
let metrics = replay.to_metrics(&constraints);
for m in &metrics {
    println!("  {}: {:.2} — {}", m.name, m.score, m.detail);
}
```

---

## 触发准确率

衡量子 Agent 路由效果：

```rust
use echo_agent::eval::{TriggerAccuracy, trigger::TriggerTestCase};

let cases = vec![
    TriggerTestCase {
        query: "读取 src/main.rs".into(),
        expected_agent: "code-explorer".into(),
        should_trigger: true,
        runs_per_query: 5,  // 运行 5 次以检测稳定性
    },
    TriggerTestCase {
        query: "2+2 等于几".into(),
        expected_agent: "".into(),
        should_trigger: false,
        runs_per_query: 1,
    },
];

let accuracy = TriggerAccuracy::evaluate(&cases, &actual_triggers);
println!("精确率: {:.2}", accuracy.precision);
println!("召回率: {:.2}", accuracy.recall);
println!("F1: {:.2}", accuracy.f1);
```

---

## HTML 报告

生成自包含的 HTML 报告，便于分享和审查：

```rust
use echo_agent::eval::generate_html;

// 静态报告
let html = generate_html(&report, "评估报告 v2.1");
std::fs::write("report.html", html)?;

// 带反馈的交互式审查
use echo_agent::eval::server::generate_review_html;
let html = generate_review_html(&report, "审查会话");
std::fs::write("review.html", html)?;
```

---

## 完整示例

```rust
use echo_agent::prelude::*;
use echo_agent::eval::*;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. 定义用例
    let cases = vec![
        EvalCase {
            id: "hello_001".into(),
            name: "Hello world".into(),
            description: "Agent 说 hello".into(),
            task: "Say hello world".into(),
            project_fixture: None,
            success_criteria: SuccessCriteria::OutputContains {
                substring: "hello".into(),
            },
            constraints: Default::default(),
        },
    ];

    // 2. 创建运行器
    let runner = EvalRunner::new(PathBuf::from("/tmp/eval"));

    // 3. 使用工厂运行
    let report = runner.run_all(&cases, || {
        let config = AgentConfig::new("qwen3-max", "assistant", "You are helpful");
        Box::new(ReactAgent::new(config))
    }).await;

    // 4. 生成报告
    let html = generate_html(&report, "我的评估");
    std::fs::write("eval_report.html", html)?;

    println!("结果: {}/{} 通过", report.passed, report.total);
    Ok(())
}
```

---

## 架构

```
┌──────────────────────────────────────────────────────┐
│                    EvalRunner                         │
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐   │
│  │ EvalCase  │  │ Fixture  │  │ SuccessCriteria  │   │
│  │  (任务)   │  │  (复制)  │  │  (评判)          │   │
│  └──────────┘  └──────────┘  └──────────────────┘   │
│                                                      │
│  ┌──────────────────────────────────────────────┐    │
│  │              Agent.execute(task)               │    │
│  └──────────────────┬───────────────────────────┘    │
│                     │                                 │
│  ┌──────────────────▼───────────────────────────┐    │
│  │              Trace (Run)                       │    │
│  │  run_id, events, token_usage, timings          │    │
│  └──────────────────┬───────────────────────────┘    │
│                     │                                 │
│  ┌──────────────────▼───────────────────────────┐    │
│  │              EvalResult                        │    │
│  │  success, score, metrics, violations           │    │
│  └──────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────┘
         │                    │
         ▼                    ▼
┌─────────────────┐  ┌─────────────────┐
│   EvalReport    │  │  HTML Report    │
│  (汇总)         │  │  (可视化)       │
└─────────────────┘  └─────────────────┘
```

---

## 与 Agent 的集成

评估系统**不内置于 Agent 循环中**，而是作为独立的评估 pass 外部运行。这种设计保持 Agent 轻量——大多数用户在生产环境中不需要评估。

### Feature 开关

通过 `eval` feature flag 启用：

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["eval"] }
```

### 使用模式

```
┌─────────────────────────────────────────────────┐
│  生产环境（无评估）                                │
│  agent.execute("执行任务").await                   │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│  评估（独立 pass）                                 │
│  let runner = EvalRunner::new(workspace);        │
│  let report = runner.run_all(&cases, factory);   │
│  generate_html(&report, "报告");                  │
└─────────────────────────────────────────────────┘
```

评估是**事后分析工具**，不是运行时钩子。适用场景：
- 提示词变更后（A/B 对比）
- 发布前（回归套件）
- 定期执行（CI/CD 质量门禁）

另见：[25 - 自进化系统](./25-self-improvement.md) 了解评估如何驱动改进循环。
