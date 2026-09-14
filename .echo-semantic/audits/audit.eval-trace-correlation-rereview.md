---
schema_version: 1
id: audit.eval-trace-correlation-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: state_authority
freshness: examined
revision: 81e2756cee9127fa23a9bb1023bd56aa8f954964
finding_refs: [finding.eval-trace-identity]
challenges:
  product-correlation-trace-ownership:
    revision: 81e2756cee9127fa23a9bb1023bd56aa8f954964
    source_refs: [src/eval/runner.rs, src/agent/react/mod.rs, echo-core/src/agent/event_envelope.rs, echo-core/src/tools/mod.rs]
    evidence_refs: [evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification]
  exact-summary-and-run-identity:
    revision: 81e2756cee9127fa23a9bb1023bd56aa8f954964
    source_refs: [src/eval/runner.rs, src/trace/mod.rs]
    evidence_refs: [evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification]
  terminal-and-optional-trace-boundary:
    revision: 81e2756cee9127fa23a9bb1023bd56aa8f954964
    source_refs: [src/eval/runner.rs, docs/adr/0037-eval-timeout-turn-settlement.md, docs/adr/0038-eval-trace-correlation-identity.md]
    evidence_refs: [evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification]
---

# Eval trace correlation 独立复审

## 审查范围

复审product/business run、Eval invocation correlation与真实trace Run ID的owner，EventIdentity/ExternalRunContext装配，RunStore summary/load选择，零候选与故障投影，Turn terminal保持，双语文档、ADR、语义证据、SDK零差异和Issue状态。

## 已检查故障假设

验证Eval是否仍读取共享product getter，formal run correlation是否篡改legacy product状态，nested/other child是否误选，多个精确候选是否按顺序猜测，summary与loaded Run是否交叉，零候选是否被错误强制失败，list/load/dangling/mismatch是否被吞掉，以及Failed/Cancelled/Timeout或未settled路径是否因trace变为成功或发生提前查询。

## 实际实现路径与证据

Eval为每次invocation分配唯一`eval-`值，经EventIdentity::for_run和ExternalRunContext同时进入run/turn/execution；ReactAgent继续分配`run_<uuid>`真实trace并保存该correlation。只有Turn settled后才list parent group，按三字段精确筛选唯一summary，再load并复核Run ID与完整tuple。唯一Run同时提供EvalResult.run_id、criteria、constraints和metrics；零候选保持可选，authority不一致失败。

真实React red命中product ID泄漏并exit 101；green证明真实trace可load、37/5 usage进入结果且legacy product不变。Lookup与projection tests覆盖其它child、零候选、歧义、list/load/dangling/mismatch；provider failure返回Failed诊断trace；unsettled list/load为0。EvalRunner 16、Eval 28、Improve 17、documentation contract 5 tests、Clippy/check、SDK零diff和semantic gates通过。

## 问题记录

独立review的Critical、Important、Minor均为0。`finding.eval-trace-identity`具备repair、verification与rereview证据，可标记resolved。Issue #49保持open，等待本地提交进入远端main后关闭。

## 残余风险

RunStore默认parent查询可能线性扫描retained traces；list/load不是事务快照。第三方Agent或RunStore仍可能违反terminal后静默、返回Running Run等生命周期合同；本修复不增加索引、retention、exporter或status一致性协议。

## 未检查项

未执行JsonlRunStore高并发stress、跨进程exporter、真实外部provider/Tool、完整workspace合并门禁、远端CI和其它75个open Finding。
