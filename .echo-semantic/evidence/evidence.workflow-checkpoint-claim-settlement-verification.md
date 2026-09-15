---
schema_version: 1
id: evidence.workflow-checkpoint-claim-settlement-verification
kind: evidence
observed_at: eb8744566dcd5a734531869ebde9f3506b132163
source_refs:
  - echo-orchestration/src/workflow/checkpoint_store.rs
  - echo-orchestration/src/workflow/graph.rs
  - echo-sdk-host/tests/extension_bridge_e2e.rs
  - echo-sdk-protocol/tests/extension_contract.rs
  - sdks/typescript/test/typed-bridge.test.js
  - sdks/python/tests/test_lifecycle.py
  - sdks/java/src/test/java/com/echoagent/sdk/TypedExtensionTest.java
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - 完整workspace门禁与远端CI留到汇总MR前执行
  - crash-cut是确定性文件状态注入，不会强制终止测试进程
---

# Workflow checkpoint claim结算验证证据

## 支持的结论

Workflow定向63项通过，覆盖默认fail-closed、Memory/File exact attempt、跨实例长resume持续
renew、原始lease年龄后不回收、tag冲突、owner-cleared crash cut和安全重新claim。Protocol
extension合同29项通过。

Host all-feature E2E真实执行remote save、generation CAS、claim、heartbeat renew、首次provider
失败后的requeue，以及第二次resume成功后的ack。TypeScript build与13项typed bridge、Python
focused 2项加Ruff、Java TypedExtensionTest 5项均通过。

## 来源与范围

两轮独立review先发现跨实例recover/renew竞态及三语言/heartbeat协商缺口；修复后的最终
review为pass，Critical、Important、Minor均为0。

## 已知缺口

未注入真实进程kill或网络分区；最终all-workspace与生成合同由MR门禁统一执行。
