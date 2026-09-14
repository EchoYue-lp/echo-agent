---
schema_version: 1
id: audit.eval-evolution.failure-concurrency
kind: audit
boundary_ref: boundary.eval-evolution
lens: failure_concurrency
freshness: examined
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.eval-timeout-settlement, finding.improve-iteration-config, finding.improve-single-case-panic, finding.eval-workspace-generation-isolation, finding.background-review-detached-persistence-settlement]
challenges:
  eval-timeout-settlement:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/eval/runner.rs, echo-core/src/agent/mod.rs, src/agent/react/run/stream_channel.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  improve-iteration-and-workspace:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/improve/loop.rs, src/improve/eval_improvement.rs, src/eval/runner.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  background-review-settlement:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/evolution/background_review.rs, src/evolution/dreaming.rs, src/evolution/runtime_integration.rs]
    evidence_refs: [evidence.provider-protocol-quality]
---

# Eval、Improve 与 Background Review 并发失败审计

## 审查范围

审查Eval timeout producer settlement、Improve iteration/split/temp workspace、BackgroundReviewer detached task与Dreaming调度边界。

## 已检查故障假设

验证timeout是否等待Agent effect/trace完成，max_iterations是否生效，单case是否panic，并发/early-stop是否删除或遗留错误workspace，以及丢弃review handle后auto-persist是否结算。

## 实际实现路径与证据

Eval timeout只cancel并立即评分；ReactAgent有detached bounded reaper但EvalRunner无法await，dyn Agent也无receipt。max_iterations未传入，单case clamp可panic。Fixture目录按case ID，Improve按全局tmp/improve_i且early-stop在cleanup前break，并发可互删/遗留。BackgroundReviewer返回JoinHandle且允许discard，任务可auto-persist，错误仅在无人观察的ReviewOutcome；max_iterations也未消费且无cancel/deadline。Dreaming调度明确留给应用。

## 问题记录

确认三个既有Finding；新增Eval workspace generation isolation与Background Review detached persistence settlement。

## 残余风险

重叠Dreaming pass对warm memory执行get-modify-put无pass-level single-flight/CAS，当前缺应用调度证据，作为audit atomicity residual。

## 未检查项

未检查echo-agent-cli调度、外部Agent settlement或动态并发复现。
