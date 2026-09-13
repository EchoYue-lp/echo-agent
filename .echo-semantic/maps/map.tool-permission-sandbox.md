---
schema_version: 1
id: map.tool-permission-sandbox
kind: capability_map
title: Tool、Permission、Sandbox 与外部 Effect
risk: high
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
boundary_refs: [boundary.tool-permission-sandbox]
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions]
finding_refs: [finding.tool-read-cache-scope, finding.streaming-tool-validation, finding.plan-mode-write-surface, finding.approval-authority, finding.hook-protected-path, finding.sandbox-minimum-isolation, finding.guard-direction-contract, finding.trace-effect-event-producers, finding.trace-audit-secret-boundary, finding.effect-cleanup-owner, finding.tool-pipeline-example-drift]
audit_refs: []
related_map_refs: [map.agent-session-turn, map.task-subagent-workflow, map.observation-persistence-delivery, map.extension-lifecycle]
scenarios:
  agent-automated-policy-pipeline:
    status: needs_review
    source_refs: [echo-core/src/tools/mod.rs, echo-execution/src/tools.rs, src/agent/react/run/pipeline.rs, src/agent/snapshot.rs]
    behavior_refs: [behavior.effect-permission-execution]
    evidence_refs: [evidence.effects-extensions]
    finding_refs: [finding.streaming-tool-validation, finding.plan-mode-write-surface, finding.approval-authority, finding.hook-protected-path, finding.trace-audit-secret-boundary, finding.tool-pipeline-example-drift]
    unknown: ReactAgent 自动工具路径有确定 stage 顺序，但 streaming validation、permission precedence、secret retention 与示例合同尚未闭合
    next_step: 沿真实 15-stage pipeline 执行 focused audit，不以静态示例数组作为权威
  programmatic-tool-manager:
    status: needs_review
    source_refs: [echo-core/src/tools/mod.rs, echo-execution/src/tools.rs]
    finding_refs: [finding.tool-read-cache-scope, finding.streaming-tool-validation]
    unknown: 公开 ToolManager 是 caller-owned primitive；read cache 缺 workspace/invocation identity 且 streaming path 跳过统一 validation
    next_step: 审计 cache scope 与 stream/non-stream execution kernel，对 caller-owned 权限策略不作产品假设
  trusted-hook-effects:
    status: mapped
    source_refs: [echo-execution/src/skills/hooks.rs, src/agent/react/run/pipeline.rs]
    behavior_refs: [behavior.extension-publication]
    rule_refs: [rule.permission-effect-order]
    evidence_refs: [evidence.effects-extensions]
  plan-and-readonly-surface:
    status: mapped
    source_refs: [src/agent/react/mod.rs, src/agent/react/run/pipeline.rs, echo-tools/src/registry.rs]
    finding_refs: [finding.plan-mode-write-surface]
  permission-and-approval:
    status: mapped
    source_refs: [echo-core/src/tools/permission.rs, echo-orchestration/src/human_loop/service.rs, echo-tools/src/shell.rs]
    finding_refs: [finding.approval-authority, finding.hook-protected-path]
    rule_refs: [rule.permission-effect-order]
  sandbox-and-resource-cleanup:
    status: mapped
    source_refs: [echo-core/src/sandbox.rs, echo-execution/src/sandbox/manager.rs, echo-execution/src/sandbox/local.rs, echo-core/src/tools/artifact.rs, echo-tools/src/git_worktree.rs]
    finding_refs: [finding.sandbox-minimum-isolation, finding.effect-cleanup-owner]
    evidence_refs: [evidence.effects-extensions]
  guard-and-trace-projection:
    status: needs_review
    source_refs: [echo-core/src/guard/mod.rs, src/agent/snapshot.rs, src/trace/mod.rs, echo-state/src/audit/memory.rs]
    finding_refs: [finding.guard-direction-contract, finding.trace-effect-event-producers, finding.trace-audit-secret-boundary]
    unknown: Guard directions/event producers 未闭合，trace 与 in-memory audit 原样保存输入，不能作全局 secret-redaction 保证
    next_step: audit 每个 observation backend 的 retention/redaction contract
  command-cell-runtime:
    status: mapped
    source_refs: [echo-core/src/tools/cell.rs, echo-orchestration/src/tasks/command_cell.rs, docs/adr/0025-deterministic-command-cell-watcher.md]
    behavior_refs: [behavior.effect-permission-execution]
    evidence_refs: [evidence.effects-extensions, evidence.task-subagent-workflow]
  k8s-caller-drop:
    status: needs_review
    source_refs: [echo-execution/src/sandbox/manager.rs, echo-execution/src/sandbox/k8s.rs]
    unknown: stream consumer drop 是否可中断 Pod 创建并跳过 delete 尚无故障注入证据
    next_step: 在真实或 deterministic K8s fake backend 上验证 caller-drop cleanup
  direct-user-surface:
    status: excluded
    source_refs: [docs/en/39-framework-application-boundary.md]
    reason: 用户主动 terminal/MCP/file-picker 的产品交互与 Agent 自动工具 permission 不是同一 framework 入口
    risk: high
    recheck_when: embedding application 将 direct-user 操作通过 framework Agent pipeline 执行时
---

# Tool、Permission、Sandbox 与外部 Effect

## 能力范围

覆盖 Tool contract/manager/pipeline、visibility、permission/approval、Guard/Hook rewrite、Sandbox、artifact、CommandCell 和具体 file/process/network/Git effect。

## 入口与输出

LLM tool call 进入 ReactAgent policy pipeline；programmatic ToolManager call 由 caller 拥有 policy；用户配置 Hook 是 trusted extension 且可在 PermissionStage 前产生 effect。三类入口输出 typed ToolResult/Failure/artifact/event 或外部状态变化。

## 行为关系

ToolManager 管执行 primitive，ReactAgent pipeline 组合自动工具策略，PermissionService 管其用户许可，CommandPolicy 管 shell 命令类别，SandboxPolicy 管隔离；trusted Hook effect 不伪装成同一授权入口。

## 状态与数据流

ReactAgent requested/effective invocation 在执行前固定；permit/cache/retry 由 ToolManager 管理，但 cache scope 与 streaming validation 已有 Findings；resource guard/artifact 保存完整输出与 cleanup 信息。

## 策略来源与优先级

Intervention、visibility、plan、Hook、Permission、read-before-edit、Skill allowlist、execute、post Hook、Guard、artifact、trace/callback 依序作用。

## 生命周期与失败路径

Validate/authorize/rewrite/execute/stream/cancel/timeout/retry/drain/cleanup/terminal；partial side effect 不等于无副作用失败。

## 权限与敏感信息

Agent 自动路径使用 typed permission；本地直接用户功能不被其误挡；secret redaction/retention 是 backend-specific contract，当前不能宣称 trace/audit 全局不保存原始输入。

## 用户侧投影

Permission prompt、Tool progress/result、CommandCell snapshot 与 artifact ref 是投影，不自行扩大权限。

## 场景处置清单

Agent pipeline、programmatic primitive、trusted Hook effect 与 CommandCell 已拆分；已知缺口进入 Findings，K8s caller-drop 保持 needs_review，direct-user surface 明确 excluded。

## 未展开项

具体 domain tool 的算法正确性在后续 Finding/audit 中按需展开。
