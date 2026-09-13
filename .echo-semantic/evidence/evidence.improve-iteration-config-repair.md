---
schema_version: 1
id: evidence.improve-iteration-config-repair
kind: evidence
observed_at: source:e5661d8044dbe3eba9bc3ce5fd34a408b5fc89558472af0a66cbc6566a39c0ce
source_refs:
  - src/improve/eval_improvement.rs
  - src/improve/loop.rs
supports: [behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - 只修复max_iterations字段传递，不改变threshold、split、report或workspace lifecycle
  - 零迭代返回Some空LoopResult，不代表执行过质量评估
---

# Improve iteration config 修复证据

## 支持的结论

基准`2bac05803adf25192fc494824918169f9bf0ca1f`的public setter写入`EvalDrivenImprovement.max_iterations`，但`run`固定构造默认5次的`ImprovementLoop::new()`；真实pipeline测试配置2却返回5个iterations。当前`run`直接用既有字段构造唯一ImprovementLoop，其余默认策略保持不变。

## 来源与范围

`src/improve/eval_improvement.rs`是public配置与执行入口，`src/improve/loop.rs`是唯一consumer。没有新增builder、adapter、状态或字段；显式0沿Rust range语义产生Some空LoopResult且不调用Agent factory。

## 已知缺口

本修复不处理workspace路径冲突、early-stop cleanup、Eval timeout settlement或report写入失败；这些行为由相邻Finding继续跟踪。
