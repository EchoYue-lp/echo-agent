---
schema_version: 1
id: evidence.task-runtime-store-control-repair
kind: evidence
observed_at: 8843afd5029afba9be8a6551de4ab0a93951311f
source_refs:
- echo-orchestration/src/tasks/mod.rs
- echo-orchestration/src/planning/validator.rs
- echo-orchestration/src/tasks/revisioned.rs
- echo-orchestration/src/tasks/runtime.rs
- echo-orchestration/src/tasks/runtime_service.rs
- echo-orchestration/src/tasks/runtime_executor.rs
- src/agent/subagent/team/mod.rs
- src/agent/subagent/types.rs
- src/acp/extension.rs
- docs/adr/0060-task-execution-store-cas-and-local-interruption.md
- docs/en/09-tasks.md
- docs/zh/09-tasks.md
- docs/en/lifecycles.md
- docs/zh/lifecycles.md
supports:
- behavior.task-subagent-execution
- rule.task-subagent-authority
- behavior.protocol-projection
- rule.protocol-role-separation
limitations:
- 只证明本框架修复候选及列出的故障假设，不证明 SDK/CLI/website consumer 或远端 main/CI 已完成
- 既有其他 open Finding 与 needs_review 场景保留；完整 Rust public inventory 不自动成为三语言 API
- 2026-10-12 独立完整复审重新绑定当前候选，核对等价类型别名/条件合并/文档平铺、ADR 格式和完整门禁；见当前 rereview Audit
- ADR Status 仅规范已确认决策的机器可读格式；本地 Cargo 全门禁运行于该格式调整之前，runtime 源码未再变化
---

# Task runtime Store、输出与精确控制修复证据

## 支持的结论

注册任意 Store 后 CRUD 与执行不再分叉。通用 execution CAS 重算、typed interruption、输出原子提交、null 保留、独立分支推进、root/exact 优先级、慢 review/CAS 并行监督、detached owner 退出和 immutable watcher 统一归框架。Team 的缓存不再冒充持久输出。ACP child cancellation 只影响 invocation。

## 来源与范围

基准为 `95ec66c6083a8feb17a1c28de0d01e1a24ca8a28`。ADR 0060 记录既有实现复用、框架/SDK/EKO 分层、公开 backend opt-in 及恢复 owner 证明边界。初次设计依据是 Temporal CancellationScope、LangGraph checkpoint、Tokio cancellation safety；本框架只保留通用机制。

以下 red 均执行旧实现与新增场景，失败为真实行为反例（并非通过删除/放宽测试）：局部取消破坏独立 B、暂停阻塞独立 wave、null→None、orphan interruption、receipt actual status 不符、Team cancel 迟到成功、永久 CAS 冲突、root pause 复活 exact cancel、lease 取消父、整 wave 后才结算、慢 review/CAS 阻止 root abort/其它 exact settlement、CRUD-only preflight 触 backend、父撤销后 owner 异常 watcher 永等待、cleanup/reconcile 覆盖已提交 receipt。

日志位于该次 local execution 的 `/tmp/`，以下摘要绑定内容；长期行为证明以仓库中可重跑的同名回归测试为准，原始日志未被当作可跨机器恢复的 source ref。

| Log | SHA-256 |
|---|---|
| framework-task-runtime-local-interruption-red.log | `1fccd75756461144992f57604d2329612b89044ccce09e81aeaf44799fa6aa88` |
| framework-task-review-red-orchestration.log | `1625251b01a149bc43ac4c1aac90a06636d677e5f8c509aee9a1e8bf78b55e0f` |
| framework-task-review-red-team-fixed-fixture.log | `e3a8fb6c4ac03167a261471f2192d49663a396b0f704e455403ed399eae378ab` |
| framework-task-conflict-red.log | `208c5a66aa716ef0cd40ce2a52939d6ac90318c84ec17faae8e45107afa43c5f` |
| framework-root-pause-exact-cancel-red.log | `1a6d7f569b8a5e94f192e2083bfa1e25c04770f7a1ca7a628896cf00623cd75d` |
| framework-extension-child-token-red.log | `d2bb932dbf43251d6ae4a2c9aa6e25345be90d475b22d159070835dccf6b261d` |
| framework-immediate-attempt-settlement-red.log | `13bd70406e040864f3c647e4710dc21f1350fa6ddbee3506407be4a9c27a6812` |
| framework-supervised-settlement-red.log | `96ffa7a749dd1ce476716c42a189a45306680f5ece2e5797093904cf0d32ca99` |
| framework-runtime-capability-red.log | `3c64a428533e4b69f50a5c906b53af53922298693a158666980e36bf2c11f10a` |
| framework-settlement-owner-exit-red.log | `de7b5f192a37e8cf8ed8ecd27c8b736d635fd240c3fe4bc4cab15d45c13e4af1` |
| framework-settlement-receipt-preservation-red.log | `fc63afb65d3e40a4982aa05210f9a119c371c9eb9ebb7700ded224c25065a455` |
| framework-attempt-publication-red.log | `d27f7bad3f76d96a49fe5cdba1cf0ec32efc28b69b014829d76bd8153bf5b5e3` |

## 已知缺口

新 Finding 在最终语义独立复审归档前保持 open。第三方 backend 跨进程事务与 owner fencing、SDK/CLI 消费验证、正式 successor main 与远端 CI 属于分别交付的证据，不由本修复扩大宣称。


历史定位说明：原未提交 source8896e508 的846个源码/文档文件已逐字节重建，唯一 source_digest 精确匹配，并保存到本地 recoverable Git snapshot `8843afd5029afba9be8a6551de4ab0a93951311f`（refs/echo-semantic/snapshots/sdk-framework-reviewed-8896）。这是内部审查证据快照，不是交付分支提交、公开 dependency 或 main。原结论与限制保持；新 parent-drop 源码不借此历史 pass。
