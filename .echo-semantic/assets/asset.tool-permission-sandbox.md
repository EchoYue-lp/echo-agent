---
schema_version: 1
id: asset.tool-permission-sandbox
kind: asset
title: Tool、Permission 与 Sandbox Effect Pipeline
asset_type: state_authority
status: needs_review
risk: high
observed_at: e59fe773d92bca409fe0606b608c4812f87f7ac2
boundary_refs: [boundary.tool-permission-sandbox]
code_refs: [echo-core/src/tools/mod.rs, echo-core/src/tools/permission.rs, echo-execution/src/tools.rs, echo-orchestration/src/human_loop/service.rs, src/agent/react/run/pipeline.rs, echo-execution/src/skills/hooks.rs, echo-execution/src/sandbox/local.rs]
consumer_refs: [src/agent/react/subsystems/tool_exec.rs, echo-tools/src/lib.rs]
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.streaming-tool-validation-repair, evidence.streaming-tool-validation-verification, evidence.tool-read-cache-authority-repair, evidence.tool-read-cache-authority-verification, evidence.tool-registry-owned-handle-repair, evidence.tool-registry-owned-handle-verification]
finding_refs: [finding.tool-read-cache-scope, finding.tool-registry-mutation-active-call-deadlock, finding.streaming-tool-validation, finding.plan-mode-write-surface, finding.approval-authority, finding.hook-protected-path, finding.sandbox-minimum-isolation, finding.guard-direction-contract, finding.trace-audit-secret-boundary, finding.effect-cleanup-owner]
candidate_refs: []
---

# Tool、Permission 与 Sandbox Effect Pipeline

## 资产身份

Tool registry/schema/execution primitive 与 ReactAgent permission/approval/rewrite/guard、artifact、resource cleanup 的组合边界；不把 programmatic caller 或 trusted Hook 误并为同一 policy path。

## 来源与消费者

ReactAgent、Skills/Hooks、Subagents 和直接 callers 消费；echo-tools 提供具体 effect。

## 生命周期

Register/visibility、owned generation lookup、shared validation、context-scoped cache/epoch、authorize/rewrite、execute/stream、persist artifact、cancel/cleanup/terminal。

## 候选关系

PermissionPolicy、PermissionService 与 Hook decision 是组合层，不应各自宣称最终 effect authority。

## 未知与限制

Hook permission precedence 和部分工具 cleanup 等待下一阶段审计。
