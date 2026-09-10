---
schema_version: 1
id: evidence.sdk-contracts
kind: evidence
observed_at: source:64a3f6010a8c386321bee7ac23bcf0cac3f6cc8bd588d22c1ac2c87d942b317c
source_refs:
  - Cargo.toml
  - contracts/sdk/parity-manifest.json
  - contracts/sdk/parity-manifest.schema.json
  - contracts/sdk/public-api.txt
  - contracts/sdk/schema/echo-agent-extension-v1.schema.json
  - contracts/sdk/facade-operation-catalog.json
  - echo-core/src/compression.rs
  - echo-execution/src/skills/external/prompt_exec.rs
  - echo-execution/src/skills/external/loader.rs
  - echo-execution/src/skills/external/run_script_tool.rs
  - echo-execution/src/skills/hooks.rs
  - echo-execution/src/skills/registry.rs
  - echo-integration/src/mcp/client.rs
  - echo-orchestration/src/tasks/revisioned.rs
  - echo-orchestration/src/tasks/runtime_executor.rs
  - echo-orchestration/src/workflow/mod.rs
  - echo-state/src/compression/compressor/hybrid.rs
  - echo-state/src/compression/compressor/sliding_window.rs
  - echo-state/src/compression/compressor/summary.rs
  - echo-state/src/compression/horizon.rs
  - echo-state/src/compression/levels.rs
  - src/state/mod.rs
  - echo-sdk-protocol/src/inventory.rs
  - echo-sdk-protocol/src/methods.rs
  - echo-sdk-protocol/tests/facade_inventory.rs
  - echo-sdk-protocol/tests/extension_contract.rs
  - echo-sdk-host/src/core_profile/facade/mod.rs
  - echo-sdk-host/Cargo.toml
  - echo-sdk-host/src/core_profile/facade/memory.rs
  - echo-sdk-host/tests/core_profile_e2e.rs
  - echo-sdk-host/tests/extension_bridge_e2e.rs
  - echo-sdk-host/tests/facade_feature_adapters_e2e.rs
  - echo-sdk-host/tests/support/mod.rs
  - sdks/typescript/src/client.ts
  - sdks/typescript/src/types.ts
  - sdks/typescript/src/wire.ts
  - sdks/typescript/test/typed-bridge.test.js
  - sdks/python/src/echo_agent_sdk/client.py
  - sdks/python/src/echo_agent_sdk/__init__.py
  - sdks/python/tests/test_lifecycle.py
  - sdks/java/src/main/java/com/echoagent/sdk/AgentComponentCall.java
  - sdks/java/src/main/java/com/echoagent/sdk/AgentComponentDescriptor.java
  - sdks/java/src/main/java/com/echoagent/sdk/AgentComponentHandler.java
  - sdks/java/src/main/java/com/echoagent/sdk/AgentComponentOutcome.java
  - sdks/java/src/main/java/com/echoagent/sdk/AgentComponentRequest.java
  - sdks/java/src/main/java/com/echoagent/sdk/AgentComponentResult.java
  - sdks/java/src/main/java/com/echoagent/sdk/AgentComponentStreamChunk.java
  - sdks/java/src/main/java/com/echoagent/sdk/AgentComponentStreamComplete.java
  - sdks/java/src/main/java/com/echoagent/sdk/CompressionCall.java
  - sdks/java/src/main/java/com/echoagent/sdk/CompressionHandler.java
  - sdks/java/src/main/java/com/echoagent/sdk/CompressionOutcome.java
  - sdks/java/src/main/java/com/echoagent/sdk/CompressionOutput.java
  - sdks/java/src/main/java/com/echoagent/sdk/ContextCompressorDescriptor.java
  - sdks/java/src/main/java/com/echoagent/sdk/ExtensionStreamWriter.java
  - sdks/java/src/main/java/com/echoagent/sdk/TokenizerReference.java
  - sdks/java/src/main/java/com/echoagent/sdk/EchoAgentClient.java
  - sdks/java/src/main/java/com/echoagent/sdk/JsonExtensionOutcome.java
  - sdks/java/src/main/java/com/echoagent/sdk/TypedExtensionSupport.java
  - src/agent/react/capabilities.rs
  - src/agent/react/mod.rs
  - src/agent/react/subsystems/tool_exec.rs
  - src/eval/grader.rs
  - src/eval/runner.rs
  - src/improve/loop.rs
supports: [behavior.sdk-facade-routing, rule.sdk-rust-authority]
limitations:
  - 三语言manifest状态尚未达到全部done
---

# SDK 合同与运行证据

## 支持的结论

当前合同可确定列出root facade、route、signature、feature和语言状态；真实Host测试已覆盖ACP、core、family、extension、全部canonical source adapter、typed compressor/AgentComponent consumer trait及Workflow/A2A facade stream的显式关闭、Session关闭和connection EOF。

## 来源与范围

来源包括`echo-sdk-protocol/src/facade.rs`、`echo-sdk-protocol/tests/facade_inventory.rs`、`echo-sdk-host/src/core_profile/facade/source_operations.rs`、`echo-sdk-host/src/core_profile/facade/stream.rs`、`echo-sdk-host/src/core_profile/facade/workflow.rs`、`echo-sdk-host/src/core_profile/facade/integrations.rs`、`echo-sdk-host/tests/core_profile_e2e.rs`、`echo-sdk-host/tests/facade_feature_adapters_e2e.rs`、`contracts/sdk/source-contract.json`、`sdks/shared/contract-digests.json`和`sdks/shared/facade-operation-catalog.json`。

## 已知缺口

机械闭合、第四轮finding修复对应的focused E2E、no-bridge/bridge零告警组合、完整workspace/all-feature/单feature门禁、三语言源码门禁、90个合同artifact和19个ExtensionBridge E2E均已通过；组件流终态、Sandbox取消、MCP发布失败清理及SkillLoadPolicy live路径均有反例。improve单feature显式包含eval；sdk-extension-bridge显式包含其tokenizer所需的唯一facade adapter authority。CI保留Linux lld flags并追加-D warnings，先编译全部bridge test targets，再以明确的--test参数真实执行19个ExtensionBridge E2E；full profile执行23个。第十轮独立复审结论为pass，因此证据支持Plan 8 completed；三语言manifest状态未全部done，仍不能支持总体Parity complete声明。
