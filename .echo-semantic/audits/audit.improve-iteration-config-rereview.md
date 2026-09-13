---
schema_version: 1
id: audit.improve-iteration-config-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: trigger_input
freshness: examined
revision: 57066461ddbe8a32ce63f1b75dd40603530e786e
finding_refs: [finding.improve-iteration-config]
challenges:
  configured-iteration-consumption:
    revision: 57066461ddbe8a32ce63f1b75dd40603530e786e
    source_refs: [src/improve/eval_improvement.rs, src/improve/loop.rs]
    evidence_refs: [evidence.improve-iteration-config-repair, evidence.improve-iteration-config-verification]
  zero-and-short-circuit-semantics:
    revision: 57066461ddbe8a32ce63f1b75dd40603530e786e
    source_refs: [src/improve/eval_improvement.rs]
    evidence_refs: [evidence.improve-iteration-config-verification]
---

# Improve iteration config 独立复审

## 审查范围

复审public max_iterations setter、EvalDrivenImprovement run/report路径、ImprovementLoop唯一consumer，以及2次、0次、disabled和empty cases边界。

## 已检查故障假设

验证setter值是否仍在wrapper丢失，配置2测试是否因early-stop或empty cases假通过，配置0是否错误返回None或构造Agent，以及report数量是否脱离实际iterations。

## 实际实现路径与证据

EvalDrivenImprovement只有一个max_iterations字段，run直接填入唯一ImprovementLoop，后者以`0..self.max_iterations`消费。配置2使用非空且持续失败的case，test avg保持0，不触发0.95 early-stop，因此旧实现稳定执行5轮而修复后执行2轮。配置0返回Some空LoopResult且factory调用数为0；disabled或empty cases继续在构造loop前返回None。Report遍历实际result.iterations，不伪造额外迭代。

## 问题记录

独立review无finding；`finding.improve-iteration-config`具备repair、verification与rereview证据，可标记resolved。Singleton已解决，workspace generation与Eval timeout保持open。

## 残余风险

达到threshold时实际迭代可合法少于上限；配置0表示enabled但不执行质量评估；真实LLM成本和report写入失败未验证。

## 未检查项

未执行workspace lifecycle、report故障注入、完整workspace合并门禁、远端CI或其它81个open Finding。
