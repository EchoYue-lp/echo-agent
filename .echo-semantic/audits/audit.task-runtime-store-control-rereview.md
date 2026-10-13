---
schema_version: 1
id: audit.task-runtime-store-control-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: state_authority
freshness: examined
revision: 8843afd5029afba9be8a6551de4ab0a93951311f
finding_refs: [finding.task-runtime-store-control]
challenges:
  one-backend-and-lossless-result:
    revision: 8843afd5029afba9be8a6551de4ab0a93951311f
    source_refs: [echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/tasks/runtime_service.rs, src/agent/subagent/team/mod.rs]
    evidence_refs: [evidence.task-runtime-store-control-repair, evidence.task-runtime-store-control-verification]
  exact-control-and-independent-waves:
    revision: 8843afd5029afba9be8a6551de4ab0a93951311f
    source_refs: [echo-orchestration/src/tasks/runtime.rs, echo-orchestration/src/tasks/runtime_executor.rs, src/agent/subagent/types.rs]
    evidence_refs: [evidence.task-runtime-store-control-repair, evidence.task-runtime-store-control-verification]
  retained-settlement-and-immutable-observation:
    revision: 8843afd5029afba9be8a6551de4ab0a93951311f
    source_refs: [echo-orchestration/src/tasks/runtime_executor.rs, docs/adr/0060-task-execution-store-cas-and-local-interruption.md]
    evidence_refs: [evidence.task-runtime-store-control-repair, evidence.task-runtime-store-control-verification]
---

# Task runtime Store/control 独立完整复审

## 审查范围

2026-10-12 独立 reviewer `framework_semantic_review` 对基准
`95ec66c6083a8feb17a1c28de0d01e1a24ca8a28` 到当前未提交候选进行完整 implementation
复审，不继承缺少可恢复源码锚点的历史 pass。计划与用户指定设计为 none；按用户目标和
仓库约束审查，完整读取实际 ADR 0060、全部变更和直接框架消费者。

## 已检查故障假设

检查注册 Store 后 CRUD/执行是否仍分叉，execution CAS 是否遗漏当前关系或执行前置条件，
Completed/result 是否分开提交，显式 null 是否丢失，精确取消/暂停是否升级为 root，
独立 wave 是否被局部中断挡住，恢复是否重发 live claim，慢 CAS/review 是否阻塞监督，
父撤销和 backend 异常是否遗留永等待 watcher，以及 cleanup/reconcile 是否覆盖真实终态。

## 实际实现路径与证据

同一 `TaskRevisionService` 和注册 backend 管理关系、执行、结果和精确中断。执行能力声明
在副作用前检查，缺失能力明确 Unsupported；结果与终态同一 CAS，保留 absent/null 区分。
独立 settlement owner 受监督，真实 receipt 经唯一私有发布边界投影；已发布 Settled 不降级。

审者核验 23 份完整门禁日志、12 份 red 日志的 SHA 与字节数，并确认最终日志实际包含
对应反例测试。实际 3426 passed、0 failed、3 existing ignored、59 examples 编译；
三个 ignored 分别依赖真实 LSP、公共学术 provider、Zotero 凭据，未宣称验证这些外部服务。
排除语义材料的 tracked diff SHA 为
`c7e98cefe25ed9370d08d9f490fa19d86bfa5e964ef767bb03f1cce443138efe`，
与完整门禁一致。ADR 原 SHA `1966e62f87e320543943c37e4a174991da168c12faf2486f6ab8064f769c4b2f`
匹配原门禁，当前 SHA `4379c21fa6a1b0d57bc2727c4cb8ecac7ca208331e3a26cbfe83471856671c27`；
真实文件比较仅为原 accepted 状态转换为规范 Status 章节，没有虚构签署或 main 交付。
现场严格快照、base/change-evidence 校验与 diff 检查通过，index 无未解决冲突。

## 问题记录

完整复审结论 pass，Critical/Important/Minor 均为 0。本地 Finding 的 repair、verification
和本次 rereview 链闭合；相关广义 Behavior/Rule 仍保持 needs_review，其他 Finding 不受影响。

## 残余风险

第三方 backend 的真实持久事务与跨进程 owner fencing 仍由实现方验证。审查 SDK/CLI
转换入口不代替其真实执行证据；不证明框架全部能力或 SDK accepted families 已完成。

## 未检查项

SDK 三语言真实 Host、CLI 完整编译运行、官网同步、已推送 successor pin、远端 CI、
精确 main 验收和外部 Issue 关闭仍由协调任务单独完成。本次未执行 Cargo 或 Git 交付动作。

历史定位说明：原未提交 source8896e508 的846个源码/文档文件已逐字节重建，唯一 source_digest 精确匹配，并保存到本地 recoverable Git snapshot `8843afd5029afba9be8a6551de4ab0a93951311f`（refs/echo-semantic/snapshots/sdk-framework-reviewed-8896）。这是内部审查证据快照，不是交付分支提交、公开 dependency 或 main。原结论与限制保持；新 parent-drop 源码不借此历史 pass。
