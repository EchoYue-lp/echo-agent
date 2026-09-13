---
schema_version: 1
id: audit.task-patch-claim-cas-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: failure_concurrency
freshness: examined
revision: 78b9f06b4320531fd8f41260887cd69c1343e995
finding_refs: [finding.task-patch-claim-race]
challenges:
  canonical-producer-precondition:
    revision: 78b9f06b4320531fd8f41260887cd69c1343e995
    source_refs: [echo-orchestration/src/tasks/revisioned.rs, docs/adr/0008-canonical-runtime-task-authority.md]
    evidence_refs: [evidence.task-patch-claim-cas-repair, evidence.task-patch-claim-cas-verification]
  interleaved-runtime-mutation:
    revision: 78b9f06b4320531fd8f41260887cd69c1343e995
    source_refs: [echo-orchestration/src/tasks/revisioned.rs]
    evidence_refs: [evidence.task-patch-claim-cas-repair, evidence.task-patch-claim-cas-verification]
  public-sdk-contract:
    revision: 78b9f06b4320531fd8f41260887cd69c1343e995
    source_refs: [contracts/sdk/parity-manifest.json, contracts/sdk/facade-operation-catalog.json, echo-sdk-protocol/tests/facade_inventory.rs, scripts/check-language-sdks.sh]
    evidence_refs: [evidence.task-patch-claim-cas-verification]
---

# Task patch execution CAS 独立复审

## 审查范围

复审 canonical `TaskRevisionService` producer、`RevisionedTaskStore` exact execution compare、claim/retry/settlement 确定性交错，以及 `TaskGraphCommit` 新 public precondition 的 SDK 合同同步。

## 已检查故障假设

验证 producer 是否可能遗漏 `expected_executions` 或携带非读取时 snapshot，interleaved claim 是否仍可被 relation patch 覆盖，以及测试是否会因错误类型、未实际 claim 或手工构造 commit 而假通过。同步检查 `None` 兼容分支是否被误述为安全 patch，以及 canonical identity、alias、scope 和 digest 是否漂移。

## 实际实现路径与证据

`TaskRevisionService::apply_patch_to_loaded` 从 loaded graph 构造完整 typed execution map；Store 在 relation revision 后 exact compare 当前 map。只读 reviewer 首轮识别测试仅覆盖 consumer，修复后新增 test-only interleaving Store：它先断言 canonical commit 的 precondition 非空且等于 loaded snapshot，再注入真实 claim；缺失或错误 snapshot 返回 Backend，未发生 claim 会令 commit 成功，均无法满足最终 `RevisionConflict` 和持久 `Running + claim` 断言。16 个 revisioned 测试全部通过。SDK 生成与语言门禁确认字段为 `value:task external_contract`，当前统计为 9683/5607。

## 问题记录

首轮 Important 测试缺口和 Minor 文档不实均已直接修复并通过定向复审；`finding.task-patch-claim-race` 具备 repair、verification 与 rereview 证据，可标记 resolved。未关闭其它 Task、Subagent、Workflow 或 SDK Finding。

## 残余风险

第三方 `RevisionedTaskStore` 的跨进程原子比较仍由实现方保证；`None` direct/legacy commit 仅为兼容路径，不具备 canonical patch 的安全保证。

## 未检查项

未审查外部 Store 集成、远端 Linux/Windows CI、完整 workspace 合并门禁或其它 85 个 open Finding。
