---
schema_version: 1
id: audit.improve-singleton-split-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: trigger_input
freshness: examined
revision: 2bac05803adf25192fc494824918169f9bf0ca1f
finding_refs: [finding.improve-single-case-panic]
challenges:
  singleton-disposition:
    revision: 2bac05803adf25192fc494824918169f9bf0ca1f
    source_refs: [src/improve/loop.rs]
    evidence_refs: [evidence.improve-singleton-split-repair, evidence.improve-singleton-split-verification]
  ratio-and-group-boundaries:
    revision: 2bac05803adf25192fc494824918169f9bf0ca1f
    source_refs: [src/improve/loop.rs, src/eval/mod.rs]
    evidence_refs: [evidence.improve-singleton-split-verification]
---

# Improve singleton split 独立复审

## 审查范围

复审`ImprovementLoop::stratified_split`对单例、多例、混合criteria、有限与非有限ratio的分配，以及empty holdout进入EvalReport后的行为。

## 已检查故障假设

验证单例是否仍触发clamp panic或同时泄漏到holdout，长度至少为2的group是否可能产生空侧，每个case是否重复、遗漏或跨criteria分组，以及全singleton empty holdout是否引入新panic。

## 实际实现路径与证据

单例直接使用train-only disposition；多例把比例结果约束到`1..=len-1`。Rust浮点转无符号整数的饱和语义加上clamp使NaN与正负无穷也落入有效区间。迭代分配让每个case恰好进入一侧，criteria variant继续隔离分组；HashMap无序只改变未承诺的跨组排列。旧实现测试真实命中`min = 1, max = 0` panic，修复后的4个focused tests、Clippy与Improve feature check通过。

## 问题记录

独立review无blocker；`finding.improve-single-case-panic`具备repair、verification和rereview证据，可标记resolved。`improve-iteration-config`、Eval workspace generation与timeout settlement保持open。

## 残余风险

全部group均为单例时没有独立泛化分数且loop可能运行完整迭代；HashMap不保证跨组输出顺序；非有限ratio没有单独持久回归测试。

## 未检查项

未执行live Agent `run_async`、workspace cleanup、完整workspace合并门禁、远端CI或其它82个open Finding。
