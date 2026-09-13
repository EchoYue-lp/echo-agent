---
schema_version: 1
id: behavior.sdk-facade-routing
kind: behavior
status: verified
expectation: human_confirmed
risk: high
primary_focus: contract_evidence
focus: [state_authority, time_lifecycle, failure_concurrency]
boundary: boundary.sdk-facade-parity
observed_at: source:385c413d4058aa4224078a94c93eeb3b1b0d3e3e92e0ceb31b63a20ee17a4a47
code_refs:
  - echo-sdk-protocol/src/facade.rs
  - echo-sdk-host/src/core_profile/facade/source_operations.rs
  - echo-sdk-host/src/core_profile/facade/stream.rs
  - echo-sdk-host/src/core_profile/extension_bridge.rs
rule_refs: [rule.sdk-rust-authority]
evidence_refs: [evidence.sdk-contracts]
finding_refs: [finding.sdk-component-stream-terminal, finding.sdk-sandbox-cancellation, finding.sdk-mcp-publication-cleanup, finding.sdk-skill-load-policy-bridge, finding.sdk-no-bridge-warnings]
---

# SDK facade 路由行为

## 重要承诺

每个canonical facade项只能映射到一个真实标准方法、typed family、Host adapter、extension bridge或有证据的语言本地实现。

## 当前行为

每条canonical source operation均到达具体Host adapter；存在live Agent消费点的consumer trait进入typed compressor或AgentComponent bridge，无Host消费点的trait保留具体process-local依据；AgentComponent覆盖可覆写default方法、IntentClassifier、异步SkillLoadPolicy、Sandbox/Workflow typed stream和cancel-aware执行，且只对Workflow mutation使用排他admission；Agent、extension、Workflow和A2A stream均有真实生产者与统一生命周期。

## 期望行为

所有canonical route有可执行或可复核依据；`feature_unavailable`、`intrinsic`和空handler不能掩盖缺失实现。

## 触发、结果与副作用

Client发送operation和signature后，Host先核对协商能力、feature、signature、receiver与generation，再调用唯一Rust权威。

## 失败、重试与恢复

非法输入产生稳定typed error；adapter不自行重试、补写终态或从失败路径切换到另一实现。

## 证据

`echo-sdk-protocol/src/facade.rs`和`echo-sdk-protocol/tests/facade_inventory.rs`证明清单闭集；`echo-sdk-host/src/core_profile/facade/source_operations.rs`、`echo-sdk-host/src/core_profile/facade/stream.rs`、`echo-sdk-host/src/core_profile/facade/workflow.rs`、`echo-sdk-host/src/core_profile/facade/integrations.rs`及对应E2E证明真实执行。第四轮review的四个finding已由第五轮复审确认闭合；第五至十轮发现的feature、test-target、CI false-green及Linux linker边界已修复，完整workspace、单feature、合同、三语言门禁和19个bridge E2E均通过，第十轮独立复审结论为pass。

## 裁决记录

用户确认功能与语义全部对等、语言API保持惯用表达，并要求Plan 8独立折叠为一个任务级提交。
