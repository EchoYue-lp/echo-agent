---
schema_version: 1
id: audit.extension-invocation-cancellation-scope-rereview
kind: audit
boundary_ref: boundary.protocol-surfaces
lens: failure_concurrency
freshness: examined
revision: 8843afd5029afba9be8a6551de4ab0a93951311f
finding_refs: [finding.extension-invocation-cancellation-scope]
challenges:
  invocation-cannot-cancel-parent-or-sibling:
    revision: 8843afd5029afba9be8a6551de4ab0a93951311f
    source_refs: [src/acp/extension.rs, docs/adr/0060-task-execution-store-cas-and-local-interruption.md]
    evidence_refs: [evidence.task-runtime-store-control-repair, evidence.task-runtime-store-control-verification]
  parent-close-and-child-drain:
    revision: 8843afd5029afba9be8a6551de4ab0a93951311f
    source_refs: [src/acp/extension.rs]
    evidence_refs: [evidence.task-runtime-store-control-repair, evidence.task-runtime-store-control-verification]
---

# ACP invocation child cancellation 独立复审

## 审查范围

2026-10-12 独立 reviewer `framework_semantic_review` 对基准 95ec66c6 到当前整体
implementation 修复进行完整复审，不继承无法恢复锚点的历史 pass；plan/design 均 none。
完整读取实际 ADR 0060、ACP invocation 路径、测试和共享门禁证据。

## 已检查故障假设

单次 invocation 取消、timeout、stream release 是否仍取消传入的 parent/sibling；
父取消是否仍向下传播；close_admission 是否拒绝后续调用并取消连接 fallback；
cancel_all 是否清理所有 child 而不取消独立 custom parent；drain 是否等待真实 lease release。

## 实际实现路径与证据

`ExtensionInvocationAuthority::lease` 复用 framework token 的 child，隔离单次调用取消。
连接关闭和 drain 保留原权威，不新增 SDK 局部取消状态机、不产生 Task/Agent terminal。
旧实现三个取消隔离反例失败，当前 13 个 ACP extension 测试与全部 23 项 framework 门禁通过。
审者实际核对日志 SHA、最终测试命名和完整源码；同一快照上的严格语义检查退出 0。

## 问题记录

完整复审 pass，Critical/Important/Minor 均为 0。修复与验证证据的源码引用和边界相符，
本地 Finding 可 resolved；相关广义 protocol Behavior/Rule 仍 needs_review。

## 残余风险

SDK bridge 正确使用此原语仍需自己的 transport 和三语言证据。此记录不覆盖其它 ACP、
A2A、channel 生命周期，也不提升这些边界的审查状态。

## 未检查项

已发布 framework successor、SDK/CLI consumer、远端 CI、main 与外部 Issue 状态不在本次
本地实现闭合范围。完整 Task slice 复审结论另见 audit.task-runtime-store-control-rereview。

历史定位说明：原未提交 source8896e508 的846个源码/文档文件已逐字节重建，唯一 source_digest 精确匹配，并保存到本地 recoverable Git snapshot `8843afd5029afba9be8a6551de4ab0a93951311f`（refs/echo-semantic/snapshots/sdk-framework-reviewed-8896）。这是内部审查证据快照，不是交付分支提交、公开 dependency 或 main。原结论与限制保持；新 parent-drop 源码不借此历史 pass。
