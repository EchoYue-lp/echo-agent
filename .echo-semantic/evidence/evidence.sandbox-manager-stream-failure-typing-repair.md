---
schema_version: 1
id: evidence.sandbox-manager-stream-failure-typing-repair
kind: evidence
observed_at: source:6ed43c02230186db2c60d15eeda68864a0c1dddb9719434c45439363536592e4
source_refs:
  - echo-core/src/sandbox.rs
  - echo-execution/src/sandbox/mod.rs
  - echo-execution/src/sandbox/manager.rs
  - echo-execution/src/sandbox/local.rs
  - docs/adr/0002-sandbox-cancellation-cleanup.md
supports: [finding.sandbox-manager-stream-failure-typing, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 选择 executor 或策略拒绝发生在 stream channel 建立前，仍通过 Result 返回；本 Finding 只覆盖已选择 backend 的建流失败
  - 未连接真实 Docker/Kubernetes backend；回归使用真实 LocalSandbox manager caller 路径触发 process spawn 失败
  - 远端 CI、main 交付与 Issue 关闭仍待后续验收
---

# Issue 82 SandboxManager typed stream failure 修复证据

## 支持的结论

`SandboxManager::execute_stream` 在已选 backend 的 `execute_stream` 返回错误时，
不再构造 `ExecutionResult { exit_code: -1 }` 伪装为 `Complete`。它现在发送唯一的
`SandboxStreamEvent::Failed` 终态；SandboxError 的取消与 I/O/启动错误分别保留为
`SandboxStreamFailure::Cancelled` 或 `SandboxStreamFailure::IoError`。Local backend
与 manager 共用同一内部映射，避免两条 stream failure 语义。

## 来源与范围

实现位于 `echo-execution/src/sandbox/manager.rs`，内部映射位于
`echo-execution/src/sandbox/mod.rs`，Local backend 复用位于
`echo-execution/src/sandbox/local.rs`；typed event 合同位于
`echo-core/src/sandbox.rs`。ADR 0002 已定义 live stream 的 `Failed` terminal，
本修复只补齐 manager 的 backend-start boundary，不引入新的状态权威、公共协议或
应用策略。

## 已知缺口

本证据绑定整合提交 `bf95ef61`、基准 `origin/main@dbe9e1112e38a8a835871242fcf8dbf5abd3db0b`
与源码摘要 `source:6ed43c02230186db2c60d15eeda68864a0c1dddb9719434c45439363536592e4`。
远端 PR/CI、完整合并门禁、远端 main 与 Issue #82 关闭不是本 repair evidence 的结论。
