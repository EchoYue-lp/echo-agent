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
observed_at: 9d1f3f2b5fdc204c08ecdec32ed22e8df95870e9
code_refs:
  - echo-sdk-protocol/src/inventory.rs
  - echo-sdk-protocol/src/facade.rs
  - echo-sdk-host/src/core_profile/facade/source_operations.rs
  - echo-sdk-host/src/core_profile/facade/stream.rs
  - echo-sdk-host/src/core_profile/extension_bridge.rs
rule_refs: [rule.sdk-rust-authority]
evidence_refs: [evidence.sdk-contracts, evidence.tool-registry-owned-handle-verification, evidence.background-task-terminal-authority-verification]
finding_refs: [finding.sdk-component-stream-terminal, finding.sdk-sandbox-cancellation, finding.sdk-mcp-publication-cleanup, finding.sdk-skill-load-policy-bridge, finding.sdk-no-bridge-warnings]
---

# SDK facade 路由行为

## 重要承诺

每个canonical facade项只能映射到一个真实route和一个identity级SDK scope；route与具名capability group独立决定contract acceptance，language status只说明实现证据。

## 当前行为

每条canonical source operation均到达具体Host adapter；存在live Agent消费点的consumer trait进入typed compressor或AgentComponent bridge，无Host消费点的trait保留具体process-local依据。BackgroundTask Clone作为Rust trait implementation进入language-intrinsic scope，不生成语言facade。AgentComponent覆盖可覆写default方法、IntentClassifier、异步SkillLoadPolicy、Sandbox/Workflow typed stream和cancel-aware执行，且只对Workflow mutation使用排他admission；Agent、extension、Workflow和A2A stream均有真实生产者与统一生命周期。

## 期望行为

所有canonical route有可执行或可复核依据；`external_contract`必须三语言done，Host/Rust-only、language intrinsic、helper和deferred必须保留显式disposition，不能以scope或空handler掩盖缺失实现。

## 触发、结果与副作用

Client发送operation和signature后，Host先核对协商能力、feature、signature、receiver与generation，再调用唯一Rust权威。

## 失败、重试与恢复

非法输入产生稳定typed error；adapter不自行重试、补写终态或从失败路径切换到另一实现。

## 证据

`echo-sdk-protocol/src/facade.rs`和`echo-sdk-protocol/tests/facade_inventory.rs`证明清单闭集；`echo-sdk-host/src/core_profile/facade/source_operations.rs`、`echo-sdk-host/src/core_profile/facade/stream.rs`、`echo-sdk-host/src/core_profile/facade/workflow.rs`、`echo-sdk-host/src/core_profile/facade/integrations.rs`及对应E2E证明真实执行。第四轮review的四个finding已由第五轮复审确认闭合；第五至十轮发现的feature、test-target、CI false-green及Linux linker边界已修复，完整workspace、单feature、合同、三语言门禁和19个bridge E2E均通过，第十轮独立复审结论为pass。

## 裁决记录

用户确认语言API保持惯用表达；ADR 0031/0032进一步确认Rust identity inventory不等于外部合同，deferred只按capability推进。
