---
schema_version: 1
id: asset.tool-permission-sandbox
kind: asset
title: Tool、Permission 与 Sandbox Effect Pipeline
asset_type: state_authority
status: needs_review
risk: high
observed_at: source:e09ea140ba27724c9d3fa92c16a8139087ddfd0256c1f0d614f153e6086b54cd
boundary_refs: [boundary.tool-permission-sandbox]
code_refs: [echo-core/src/tools/mod.rs, echo-core/src/tools/permission.rs, echo-core/src/tools/artifact.rs, echo-execution/src/tools.rs, echo-execution/src/sandbox/resource_owner.rs, echo-execution/src/sandbox/manager.rs, echo-execution/src/sandbox/docker.rs, echo-execution/src/sandbox/k8s.rs, echo-orchestration/src/human_loop/service.rs, src/agent/react/run/pipeline.rs, src/agent/react/mod.rs, echo-execution/src/skills/hooks.rs, echo-execution/src/sandbox/local.rs, echo-tools/src/git_worktree.rs]
consumer_refs: [src/agent/react/subsystems/tool_exec.rs, echo-tools/src/lib.rs]
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.streaming-tool-validation-repair, evidence.streaming-tool-validation-verification, evidence.tool-read-cache-authority-repair, evidence.tool-read-cache-authority-verification, evidence.tool-registry-owned-handle-repair, evidence.tool-registry-owned-handle-verification, evidence.hook-protected-path-repair, evidence.hook-protected-path-verification, evidence.readonly-tool-capability-repair, evidence.readonly-tool-capability-verification, evidence.effect-cleanup-owner-repair, evidence.effect-cleanup-owner-verification]
finding_refs: [finding.tool-read-cache-scope, finding.tool-registry-mutation-active-call-deadlock, finding.streaming-tool-validation, finding.plan-mode-write-surface, finding.approval-authority, finding.hook-protected-path, finding.sandbox-minimum-isolation, finding.guard-direction-contract, finding.trace-audit-secret-boundary, finding.effect-cleanup-owner]
candidate_refs: []
---

# Tool、Permission 与 Sandbox Effect Pipeline

## 资产身份

Tool registry/schema/execution primitive 与 ReactAgent permission/approval/rewrite/guard、artifact、resource cleanup 的组合边界；不把 programmatic caller 或 trusted Hook 误并为同一 policy path。

## 来源与消费者

ReactAgent、Skills/Hooks、Subagents 和直接 callers 消费；echo-tools 提供具体 effect。

## 生命周期

Register/visibility、owned generation lookup、shared validation、context-scoped cache/epoch、authorize/rewrite、execute/stream、persist artifact、cancel/cleanup/terminal。Artifact writer、Sandbox backend 与 worktree creator 分别持有资源身份直至结算；未完成清理保留显式 debt。

## 候选关系

PermissionPolicy、PermissionService 与 Hook decision 是组合层，不应各自宣称最终 effect authority。

## 未知与限制

Hook Allow protected-path、readonly custom Tool 与 #47 精确资源 cleanup 反例已闭合；其它 Hook permission precedence 和部分工具 cleanup 仍等待专项审计。
