---
schema_version: 1
id: audit.sandbox-manager-stream-failure-typing-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: failure_concurrency
freshness: examined
revision: source:33686a6a0273c6f8ea85608bff92fed9774f00bfefe1bbd34aa1a2316b236cbc
finding_refs: [finding.sandbox-manager-stream-failure-typing]
challenges:
  backend-start-failure-terminal:
    revision: source:33686a6a0273c6f8ea85608bff92fed9774f00bfefe1bbd34aa1a2316b236cbc
    source_refs: [echo-execution/src/sandbox/manager.rs, echo-execution/src/sandbox/mod.rs, echo-execution/src/sandbox/local.rs, echo-core/src/sandbox.rs]
    evidence_refs: [evidence.sandbox-manager-stream-failure-typing-repair, evidence.sandbox-manager-stream-failure-typing-verification]
---

# SandboxManager 建流失败终态独立复审

## 审查范围

独立 reviewer 基于整合提交 `bf95ef61` 复核 `SandboxManager::execute_stream` 的 backend 建流错误路径、
`SandboxStreamEvent::Failed` 合同、Local backend 共用错误映射、真实 caller 回归测试、
ADR 与 focused verification evidence；基准为 `origin/main@dbe9e1112e38a8a835871242fcf8dbf5abd3db0b`。

## 已检查故障假设

已选择 backend 在启动进程或建立输出流前失败时，manager 可能把错误伪装为
`Complete(exit_code=-1)`，使消费者无法区分“未启动”与“命令完成”。同时检查取消
错误是否仍保持 `Cancelled` typed 分类，以及 manager 与 Local 是否存在分歧映射。

## 实际实现路径与证据

最终实现删除 manager 的合成 `ExecutionResult` 分支，统一通过 sandbox 模块的内部
映射发送单个 `Failed` terminal；Local backend 复用该映射。真实 manager caller test
稳定观察 `Failed(IoError)` 后 EOF，focused suites、Clippy、fmt 与 diff check 均通过。
独立 reviewer 结论为 PASS，Critical、Important、Minor 均为 0。

## 问题记录

本轮只闭合 framework 的 backend-start stream terminal 语义。选择 executor 或策略
拒绝仍在 channel 建立前返回 `Result`，不属于本 Finding 的伪造 completion 路径。

## 残余风险

完整 workspace 合并门禁已在整合快照上通过；远端 CI、远端 main 与 Issue #82 关闭尚未发生。
真实 Docker/Kubernetes backend 故障仍需部署环境验收。

## 未检查项

未连接真实 Docker/Kubernetes daemon 或跨平台远端 runner；未替代主任务的完整门禁与
交付审查。
