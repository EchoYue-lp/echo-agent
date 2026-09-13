---
schema_version: 1
id: audit.background-task-terminal-authority-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: failure_concurrency
freshness: examined
revision: source:d61c2341a008920576462b3051374115cf1b4da682c341852b052224f022d027
finding_refs: [finding.background-task-wait]
challenges:
  atomic-terminal-and-multi-waiter:
    revision: source:d61c2341a008920576462b3051374115cf1b4da682c341852b052224f022d027
    source_refs: [echo-orchestration/src/tasks/background_task.rs]
    evidence_refs: [evidence.background-task-terminal-authority-repair, evidence.background-task-terminal-authority-verification]
  admission-execution-cancel-deadline:
    revision: source:d61c2341a008920576462b3051374115cf1b4da682c341852b052224f022d027
    source_refs: [echo-orchestration/src/tasks/background_task.rs, docs/adr/0039-background-task-terminal-authority.md]
    evidence_refs: [evidence.background-task-terminal-authority-repair, evidence.background-task-terminal-authority-verification]
  panic-type-erasure-and-sdk-contract:
    revision: source:d61c2341a008920576462b3051374115cf1b4da682c341852b052224f022d027
    source_refs: [echo-orchestration/src/tasks/background_task.rs, contracts/sdk/parity-manifest.json, echo-sdk-protocol/tests/facade_inventory.rs]
    evidence_refs: [evidence.background-task-terminal-authority-repair, evidence.background-task-terminal-authority-verification]
---

# BackgroundTask terminal authority 独立复审

## 审查范围

复审BackgroundTask handle的status/result/panic唯一权威，Clone与multi-waiter观察，TaskSpawner admission/execution的cancel与deadline，child task settlement，type-erased registry，双语文档、ADR0039、SDK inventory、语义证据与Issue状态。

## 已检查故障假设

验证Notify注册与state检查间是否仍可lost wakeup，首个waiter消费T后其他waiter是否阻塞，cancel/deadline在permit队列和execution内是否都收敛，零并发是否永久Pending，abort后是否未await child就发布terminal，type-erased视图是否丢失Failed/Cancelled，以及普通错误文案是否可伪造panic出处。

## 实际实现路径与证据

`BackgroundTaskHandleState<T>`在一个短同步mutex内原子提交status、单消费者result与typed panic provenance。wait先enable Notified再检查state，使用单次调用绝对deadline；后续waiter从永久terminal立即返回。TaskSpawner从接纳时计算deadline，cancel/deadline覆盖排队与执行；execution cancel/timeout先abort并await child再terminal。`JoinError::is_panic()`是唯一true来源，type-erased registry读同一live state。

三个旧实现red分别命中multi-waiter、queued cancel与zero concurrency；修复后BackgroundTask 18、echo_orchestration 336、doctest 11通过。All-target Clippy、panic-policy Clippy、crate check、facade smoke 10、documentation contract 5、SDK 90 artifacts、facade inventory 75、TypeScript 156、Python 168和Java source/connection checks全绿。语义strict snapshot与high-risk change-evidence通过。

## 问题记录

首轮独立review发现1个Important文本panic分类和1组Minor陈旧Rustdoc/ADR。typed provenance、文案伪造反例与文档已修复；最终review为PASS，Critical、Important、Minor均为0。`finding.background-task-wait`具备repair、verification与rereview证据，可标记resolved。Issue #39保持open，等待本地提交进入远程main后关闭。

## 残余风险

Detached supervisor在runtime shutdown或TaskSpawner owner drop时没有统一settlement API；process-local handle不提供持久恢复，任意T仍只能被一个waiter消费。这些边界不破坏本次声明的运行期合同。

## 未检查项

未执行真实runtime shutdown、跨平台stress、长时间高并发等待、完整workspace合并门禁、远程CI和其他74个open Finding。
