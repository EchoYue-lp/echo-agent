---
schema_version: 1
id: evidence.effect-cleanup-owner-repair
kind: evidence
observed_at: source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74
source_refs:
  - echo-core/src/tools/artifact.rs
  - echo-core/src/sandbox.rs
  - echo-execution/src/sandbox/resource_owner.rs
  - echo-execution/src/sandbox/docker.rs
  - echo-execution/src/sandbox/k8s.rs
  - echo-execution/src/sandbox/manager.rs
  - echo-tools/src/git_worktree.rs
  - src/agent/react/mod.rs
  - docs/adr/0072-resource-cleanup-ownership.md
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 进程或Tokio runtime崩溃后的资源恢复不由进程内owner保证
  - 外部OS actor可在身份检查和路径操作之间改名，当前实现只有本地安全点检查
  - 本修复不解决其它permission、trace或sandbox typed failure Finding
---

# 精确资源 cleanup owner 修复证据

## 支持的结论

Artifact scope 删除遇到活动 writer 返回 `WouldBlock`，保留精确 pending root；writer
释放后结算，删除失败保留 `Retryable` debt。异步 writer finalization 持有 owner 到
flush、sync、rename 和 scope release 完成。根目录使用物理身份与保留的目录 guard；
alias、替换路径和 age sweep 不能借活动 writer 的身份删除新目标。
会话产物何时过期由宿主决定，React close 不替宿主提前删除；框架使清理请求的
pending、成功及 retryable 状态可观察。

Docker/K8s backend 在创建前登记精确资源名，共享实例级 active/debt registry；
per-command cleanup 和实例 `cleanup()` 只结算本实例的名称，活动 owner 或失败删除
保持可见。全局 label sweep 只作为显式手工恢复。`SandboxManager::cleanup()` 汇总
所有 backend 的结果；Agent close 在 Turn drain 后尝试保留的 sandbox executor，并在
MCP 或 sandbox 任一结算失败时继续尝试另一方。

Git worktree 创建先预留新路径，独立有界 owner 持有 checkout 与 Git admin 目录身份，
在 caller drop 后仍完成 marker 发布或补偿。receipt acknowledgement 把所有权交给
caller；未确认 receipt 只走正常的干净 managed-worktree 移除。marker 写失败只在
身份、分支及干净状态均匹配时移除原 checkout，不使用 `--force`，不删除分支；
歧义 add 或补偿失败返回路径与分支作为恢复事实。

## 来源与范围

源码修复提交 `1f53a12cd2c51290cd02c45acd06e2da5989fbb0`；任务分支已合入
远端 `main@bd17c73075d6b3cf8e00877fa0fb10d36694ea54`，当前源码为
`source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74`。
ADR 0072 记录 owner 选择、可重试债务与路径安全点边界。

## 已知缺口

进程内 registry 不覆盖重启；根目录被外部进程移动时，路径 API 无法自动找回原目录。
外部 OS actor 与路径操作间的竞态按 ADR 0072 保留为本地安全点残余。
