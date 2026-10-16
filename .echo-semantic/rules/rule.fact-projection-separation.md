---
schema_version: 1
id: rule.fact-projection-separation
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [data_durability, contract_evidence, failure_concurrency]
observed_at: 733d352fc719f922b21bab1cd46206139564367f
behavior_refs: [behavior.observation-persistence]
code_refs: [echo-core/src/agent/event_envelope.rs, echo-state/src/journal/mod.rs, echo-state/src/journal/file.rs, echo-state/src/journal/segmented.rs, echo-state/src/delivery.rs, src/trace/mod.rs, src/eval/runner.rs, docs/en/41-persistence-concepts.md, docs/zh/41-persistence-concepts.md, docs/adr/0038-eval-trace-correlation-identity.md, docs/adr/0046-turn-execution-delivery-settlement.md, docs/adr/0055-checkpoint-journal-identity.md, echo-state/src/audit/mod.rs, echo-state/src/audit/file.rs, docs/adr/0053-trace-audit-persistence-visibility.md]
evidence_refs: [evidence.persistence-observation, evidence.checkpoint-journal-binding-repair, evidence.checkpoint-journal-binding-verification, evidence.checkpoint-journal-sdk-inventory, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification, evidence.turn-terminal-delivery-settlement-repair, evidence.turn-terminal-delivery-settlement-verification, evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification, evidence.diagnostic-persistence-failure-visibility-repair, evidence.diagnostic-persistence-failure-visibility-verification, evidence.framework-only-finding-closure-verification]
finding_refs: [finding.trace-effect-event-producers, finding.eval-trace-identity, finding.trace-audit-secret-boundary, finding.turn-terminal-commit-projection-order, finding.checkpoint-journal-binding, finding.diagnostic-persistence-failure-visibility, finding.in-memory-audit-successful-drop]
---

# Fact、Projection 与 Trace 分离

## 不变量或唯一权威

在明确 journal-backed 的领域中，Journal 保存 ordered fact，projection/checkpoint 可重建；Trace 解释执行但默认不决定业务 commit。其它 live/lossy event family 不自动拥有 Journal authority。

## 适用行为

已验证范围是 EventJournal/CheckpointedReducer/DeliveryLedger 与其 projection/replay；Agent/Task/Subagent/Workflow/Hook events、transcript、trace 和 UI/协议投影需逐 family 分类。

## 当前实现

EventEnvelope提供versioned identity/sequence；Journal generation identity与sequence共同限定事实位置，CheckpointedReducer只接受同generation checkpoint，DeliveryLedger继续使用同一Journal+reducer；RunStore独立保存producer-owned trace。Transcript pending marker是可恢复intent，只有ConversationStore applied receipt成为用户历史fact；typed settlement event和Trace都是该结果的projection。Turn execution由producer terminal决定，sink projection只决定delivery；外部SDK Host恢复用真实Journal反查receipt watermark，不能从索引投影发明已交付事实。Trace/Audit persistence failure通过独立有界observer delivery报告，不改变producer terminal。

## 期望行为

丢失投影可重建；trace写失败时业务路径可继续则trace不能决定提交；需要trace的质量投影遇到歧义或存储不一致必须失败可见；gap/retention floor必须显式可观察。

## 证据

Persistence 文档、ADR 0007/0019/0030/0053/0055 与 journal/delivery/event tests 提供证据；本次identity反例的focused验证、独立复审和本地integration gate均已通过，远端平台CI等待MR执行。

## 裁决记录

用户要求明确 Journal、Projection、Trace、Delivery Ledger 谁是事实源；diagnostic persistence
visibility 已闭合，完整 event family 分类仍未完成，故本规则保持 needs_review。

## 已准入Scheduler effect与物理未知

Scheduler复用同DeliveryLedger事实权威。EffectStarted的可选payload替换须精claim且仅Claimed phase，原event同时安装保护知识并重算原retained_bytes；无替换时旧序列化不变。原callback真实结束才能正常terminal；typed观察、observer Drop、shutdown与owner loss不证明物理结束，受保护attempt不得因liveness hint消失而重投。旧历史恢复与本次新effect准入不同：本次effect前必须安装保护，不能让旧payload豁免新已知未知。该局部由原B Evidence/Audit承接，不更改其它delivery未知的普通策略或宣全域exactly-once。

## Workflow受保护claim与观察知识

已接受A要求在同Store exact attempt上收到protect positive后才准Node effect；保护是禁止expiry/replay的知识而非physical receipt。原source真实terminal+EOF与精身份确认后才可用原ack/requeue；heartbeat、observer Drop、claim本地缺失或不同Store观察不提供该证明。Memory实例边界、File持久私有保护/原目录OS锁与remote声明能力分别验证，不能用业务KV、trace或后置RPC替代同权威保护。该规则不改其它普通已结束错误的重试合同。
