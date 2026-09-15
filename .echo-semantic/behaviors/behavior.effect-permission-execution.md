---
schema_version: 1
id: behavior.effect-permission-execution
kind: behavior
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: permission_external
focus: [result_side_effect, failure_concurrency, time_lifecycle, state_authority]
boundary: boundary.tool-permission-sandbox
observed_at: e59fe773d92bca409fe0606b608c4812f87f7ac2
code_refs: [echo-core/src/tools/mod.rs, echo-core/src/tools/permission.rs, echo-core/src/tools/cell.rs, echo-execution/src/tools.rs, echo-orchestration/src/human_loop/service.rs, echo-orchestration/src/tasks/command_cell.rs, echo-execution/src/sandbox/local.rs, echo-execution/src/sandbox/k8s.rs, echo-execution/src/skills/hooks.rs, src/agent/react/run/pipeline.rs]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.streaming-tool-validation-repair, evidence.streaming-tool-validation-verification, evidence.tool-read-cache-authority-repair, evidence.tool-read-cache-authority-verification, evidence.tool-registry-owned-handle-repair, evidence.tool-registry-owned-handle-verification, evidence.mcp-tool-local-classification-repair, evidence.mcp-tool-local-classification-verification, evidence.k8s-sandbox-cleanup-settlement-repair, evidence.k8s-sandbox-cleanup-settlement-verification]
finding_refs: [finding.tool-read-cache-scope, finding.tool-read-cache-inflight-invalidation-race, finding.tool-registry-mutation-active-call-deadlock, finding.streaming-tool-validation, finding.plan-mode-write-surface, finding.approval-authority, finding.hook-protected-path, finding.sandbox-minimum-isolation, finding.guard-direction-contract, finding.trace-effect-event-producers, finding.trace-audit-secret-boundary, finding.effect-cleanup-owner, finding.k8s-sandbox-cleanup-settlement, finding.tool-pipeline-example-drift]
---

# Tool、Permission 与 Sandbox Effect

## 重要承诺

ReactAgent 自动工具调用的 schema/validation、可见性、permission、rewrite、execution、artifact 和 cleanup 应按确定顺序执行；programmatic ToolManager 与 trusted Hook effect 各自保留 caller/extension policy owner。

## 当前行为

ToolManager管Arc Tool generation注册、owned lookup、统一validation、context-scoped read cache/epoch与执行并发；stream/non-stream共用输入及cache freshness机制，registry guard不进入异步Tool lifetime。ReactAgent的PermissionPolicy/PermissionService、Hook/Guard与tool pipeline影响自动工具调用；trusted Hook command/http可在PermissionStage前执行；Sandbox和具体工具产生文件、进程、网络、数据库或Git effect。K8s Pod执行由detached backend owner持有，terminal在有界进程组、输出与Pod删除结算之后产生。

## 期望行为

直接用户交互不被 Agent 自动权限模式误挡；Agent 自动 effect 遵循 typed policy；日志/trace/audit 的 secret retention 必须按 backend 明确，破坏性 effect 有可恢复边界。

## 触发、结果与副作用

LLM tool call、程序调用、Hook 与 Skill 可触发 effect，结果以 typed ToolResult/ToolFailure/artifact/event 返回。

## 失败、重试与恢复

Invalid arguments、deny/ask、timeout、cancel、partial output、child process 与 cleanup failure 必须保留可诊断状态，不以 panic 或成功文本掩盖。

## 证据

Tool contracts、ToolManager、PermissionService、sandbox tests、artifact contract 与 ADR 0002/0005 提供已知证据。

## 裁决记录

完整 Hook permission precedence 与用户交互/自动路径组合仍需定向审计。
