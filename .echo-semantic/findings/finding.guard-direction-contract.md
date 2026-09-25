---
schema_version: 1
id: finding.guard-direction-contract
kind: finding
type: intent_gap
status: resolved
severity: medium
primary_focus: contract_evidence
focus: [failure_concurrency, result_side_effect]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.guard-direction-contract-repair, evidence.guard-direction-contract-verification]
audit_refs: [audit.tool-permission-sandbox.failure-concurrency]
decision_refs: []
repair_evidence_refs: [evidence.guard-direction-contract-repair]
verification_evidence_refs: [evidence.guard-direction-contract-verification]
rereview_audit_refs: [audit.guard-direction-contract-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Guard ToolInput/ToolOutput 与生产可达性错位

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/57

## 问题

GuardDirection 定义 ToolInput/ToolOutput，但未发现生产调用；工具输出使用通用 Output，单 Guard error 又被降级为 Warn，使上层 fail-closed 分支难以到达。

## 触发条件与影响

Consumer 配置 tool-specific guard 或依赖 guard error 阻断时，实际行为可能不同于公共合同。

## 证据

`echo-core/src/guard/mod.rs` 与 `src/agent/snapshot.rs` 的 direction/error handling 提供证据。

## 处理记录

候选修复将四个方向接到独立生产边界：Input 用户输入、ToolInput 最终有效参数、
ToolOutput 工具结果、Output 模型文本最终答案。ToolInput 在 approval rewrite 后仅允许
Pass/Warn/Block，Transform 作为阻断处理，不改变 #37 精确 receipt 的身份。GuardManager
错误传播到边界执行 fail-closed。`final_answer` 工具结果只使用 ToolOutput，避免
写入 transcript 后二次 Output 检查造成投影分歧。ADR 0076、focused 验证和限制见所引证据；
独立复审指出 streaming raw ToolStream 和 error-only raw diagnostic 两个反例；候选
现已在配置 GuardManager 时抑制未受检 stream 文本/进度，仅交付受检终态 ToolResult；
无输出失败的诊断进入 caller、Trace、Audit、callback 和 transcript，typed
failure/effects 保持不变。修正后仍待再次独立复审。
后续复审又确认结构化 data/模型富内容，以及非空 output 旁独立 error 可绕过文本
守护。本候选在文本被替换时撤销平行内容投影，并分别守护 output/error；真实工具
入口及模型 Context 发布路径已有 red/green 回归。可信用户 PostToolUse hook 在
ToolOutput guard 前看到原始结果，这是既有 #102 顺序，ADR 0076 明确边界。
后续复审发现 post-use raw `block_reason` 被 caller/telemetry 复用，以及
ToolFailure 自由文本 retry key/postcondition 到 caller/Trace；本候选已将 blocked
reason 归一到受检错误，分别守护并撤销或替换自由文本，保留 typed 恢复字段与
ADR 0074 的 effect/path 事实合同。
独立复审已通过；完整门禁、PR/CI 和 main 交付前 Issue 仍保持 open。
