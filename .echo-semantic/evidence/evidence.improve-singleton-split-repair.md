---
schema_version: 1
id: evidence.improve-singleton-split-repair
kind: evidence
observed_at: source:a2317ccf488e81ce737d93a5c7b13369d67228da5e54baf56c14210a47794342
source_refs:
  - src/improve/loop.rs
supports: [behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - singleton criteria没有独立holdout样本，不能据此声称该criteria具备泛化评分
  - 不修复max_iterations传递、Eval workspace generation、timeout settlement或cleanup
---

# Improve singleton split 修复证据

## 支持的结论

基准`6d66479fd520da9cbbb66723faa35ce69a8963a8`对每个criteria group调用`split_idx.clamp(1, len - 1)`；真实单例EvalCase稳定触发`min > max` panic。当前实现将单例完整放入train，不复制到holdout；长度至少为2的group继续把split约束在`1..=len-1`。

## 来源与范围

`src/improve/loop.rs`的唯一`stratified_split`按真实`EvalCase`和`SuccessCriteria`分组。分配改用`into_iter().enumerate()`，不再通过动态index切片。选择train-only是因为analysis loop需要失败case生成critique，同时blind holdout不能复用训练样本。

## 已知缺口

全部criteria group均为单例时holdout为空，LoopResult不会获得独立泛化评分；这比复制训练样本或进程panic更符合现有blind holdout合同。相邻Improve/Eval Finding保持开放。
