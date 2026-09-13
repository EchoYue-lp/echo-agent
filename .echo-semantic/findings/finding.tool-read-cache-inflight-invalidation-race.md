---
schema_version: 1
id: finding.tool-read-cache-inflight-invalidation-race
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: failure_concurrency
focus: [state_authority, data_durability]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: []
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.tool-permission-sandbox.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# In-flight Read 可在 Write 后复活陈旧 cache

## 问题

Read cache miss 后执行期间，Write 可先 clear cache 并完成；旧 Read 随后无 generation/epoch 检查地把写前结果重新存入 cache。

## 触发条件与影响

同一 ToolManager 上并发读写时，后续读可命中已被 Write 失效过的陈旧值；补 workspace key 也不能解决该竞态。

## 证据

`echo-execution/src/tools.rs` 的独立 read/write semaphore、write-before-execute clear 与 read-after-execute store 构成源码反例。

## 处理记录

Failure-concurrency Audit 确认；后续 repair 需 per-scope epoch/CAS 或协调 invalidation，并补确定性交错测试。
