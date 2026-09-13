---
schema_version: 1
id: audit.tool-permission-sandbox.failure-concurrency
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: failure_concurrency
freshness: examined
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.tool-read-cache-scope, finding.tool-read-cache-inflight-invalidation-race, finding.streaming-tool-validation, finding.sandbox-minimum-isolation, finding.sandbox-manager-stream-failure-typing, finding.guard-direction-contract]
challenges:
  tool-cache-and-validation:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-execution/src/tools.rs, echo-tools/src/files/files.rs, src/agent/react/run/pipeline.rs, echo-sdk-host/src/core_profile/extension_bridge.rs]
    evidence_refs: [evidence.effects-extensions]
  sandbox-selection-and-stream:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-core/src/sandbox.rs, echo-execution/src/sandbox/policy.rs, echo-execution/src/sandbox/manager.rs]
    evidence_refs: [evidence.effects-extensions]
  guard-direction-and-error:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-core/src/guard/mod.rs, src/agent/react/run/react_loop.rs, src/agent/snapshot.rs]
    evidence_refs: [evidence.effects-extensions]
---

# Tool Cache、Validation、Sandbox 与 Guard 并发失败审计

## 审查范围

审查 ToolManager cache/execute/stream、Sandbox minimum/fallback/stream typing 和 Guard direction/error 到 ReactAgent 的可达性。

## 已检查故障假设

验证共享 cache 是否跨 workspace，read/write in-flight 是否复活陈旧值，stream 是否跳 validation，fallback 是否低于 caller minimum，以及 Guard 错误/方向合同是否可达。

## 实际实现路径与证据

Cache key 不含 working_dir/run/conversation；Read miss 与 Write clear/execute 交错后，旧 Read 可在 Write 后重新 store。Streaming Tool 跳过 schema 和 custom validation。Sandbox selector 在 allow_fallback 时可低于 caller minimum；backend 建流失败又被包装为 Complete(-1) 而非 Failed。生产 Guard 只用 Input/Output，ToolInput/ToolOutput 无入口，单 Guard Err 被吞成 Warn。

## 问题记录

四个既有 Finding 均确认；新增 cache in-flight invalidation race 与 sandbox stream failure typing。

## 残余风险

ReadOnly 同时被当作无写副作用和可缓存，是否拆分 cacheability 需合同审查；K8s caller drop 在副作用 Audit 处理。

## 未检查项

未运行并发/stream fault tests，也未展开全部 domain tool 的 cache purity。
