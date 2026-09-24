---
schema_version: 1
id: rule.permission-effect-order
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: permission_external
focus: [result_side_effect, state_authority, failure_concurrency, time_lifecycle]
observed_at: source:9c022b1c18ebac8e3b0322adcc36aa003f7712600bee8ba16691e8f469aa1998
behavior_refs: [behavior.effect-permission-execution]
code_refs: [echo-core/src/tools/permission.rs, echo-orchestration/src/human_loop/service.rs, echo-execution/src/skills/hooks.rs, src/agent/react/run/pipeline.rs, echo-execution/src/tools.rs, echo-execution/src/sandbox/k8s.rs]
evidence_refs: [evidence.effects-extensions, evidence.streaming-tool-validation-repair, evidence.streaming-tool-validation-verification, evidence.tool-read-cache-authority-repair, evidence.tool-read-cache-authority-verification, evidence.tool-registry-owned-handle-repair, evidence.tool-registry-owned-handle-verification, evidence.mcp-tool-local-classification-repair, evidence.mcp-tool-local-classification-verification, evidence.hook-protected-path-repair, evidence.hook-protected-path-verification, evidence.readonly-tool-capability-repair, evidence.readonly-tool-capability-verification, evidence.effect-cleanup-owner-repair, evidence.effect-cleanup-owner-verification]
finding_refs: [finding.tool-registry-mutation-active-call-deadlock, finding.streaming-tool-validation, finding.plan-mode-write-surface, finding.approval-authority, finding.hook-protected-path, finding.sandbox-minimum-isolation, finding.trace-audit-secret-boundary, finding.effect-cleanup-owner, finding.tool-pipeline-example-drift]
---

# Permission 与 Effect 顺序

## 不变量或唯一权威

ReactAgent 的 LLM 驱动自动 Tool effect 必须先完成该入口定义的可见性、参数校验、permission/hook 决策和 invocation resource 绑定，再执行外部副作用与结果持久化。

## 适用行为

适用于 ReactAgent 自动 Tool pipeline。公开 ToolManager 是 caller-owned primitive；用户配置 Hook 与 Skill 是 trusted extension，其 command/http/MCP/Subagent effect 由扩展合同与 embedding application policy 负责。

## 当前实现

PermissionPolicy/Service、Hook/Guard与ToolExecutionPipeline分层影响ReactAgent自动调用；protected-path 由 PermissionService 在 Hook Allow 前按有效输入拒绝，readonly Agent 的构造、LLM surface 与执行门禁共用 ToolCapabilities。ToolManager以owned Arc generation进入stream/non-stream，在effect前完成统一validation，并用context key与write-lifetime epoch限制read result复用，Sandbox/具体Tool持有资源。
Artifact、Sandbox 与 Git worktree 各自持有精确资源身份及清理债务；Agent close 尝试结算保留的 Sandbox executor，普通实例 cleanup 不执行全局 label sweep。

## 期望行为

直接用户交互与 Agent 自动权限路径分离；deny/ask/allow 优先级只有一个解释，修改参数在执行前生效，cleanup 完成后才发布对应终态。

## 证据

Tool/permission contracts、PermissionService、Hook reducer、pipeline 和 sandbox tests 提供部分证据。

## 裁决记录

Stream validation、#60 protected-path 和 #81 readonly custom Tool 已在当前任务分支闭合；其它 Hook source order、deny-first、approval与cleanup冲突仍需审计，且本Rule不扩展为direct-user或trusted-extension权限门控。
本轮 #47 只闭合 Artifact/Sandbox/Worktree 精确清理 owner，不替代剩余 permission 与 trace Finding 的复审。
