---
schema_version: 1
id: finding.readonly-tools-custom-registration-bypass
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: permission_external
focus: [state_authority, result_side_effect, contract_evidence]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.readonly-tool-capability-repair, evidence.readonly-tool-capability-verification]
audit_refs: [audit.tool-permission-sandbox.permission-external, audit.readonly-tool-capability-rereview]
decision_refs: []
repair_evidence_refs: [evidence.readonly-tool-capability-repair]
verification_evidence_refs: [evidence.readonly-tool-capability-verification]
rereview_audit_refs: [audit.readonly-tool-capability-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# readonly_tools 不约束 custom Write/Execute Tool

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/81

## 问题

Builder 的 readonly_tools 只让 StandardToolPack 安装只读集合，随后 custom tools 仍无条件 add；自定义 Write/Execute 工具进入所谓 read-only Agent。

## 触发条件与影响

Embedding consumer 组合 readonly_tools 与 custom mutation tool 时，模型仍可看见并执行写副作用，违背构造期只读合同。

## 证据

`src/agent/react/builder.rs` 与 `src/agent/react/mod.rs` 展示 standard/custom 注册顺序和缺少 permission-based filter。

## 处理记录

`cd37e5d3` 以本地 ToolCapabilities 同时约束构造期自定义工具注册、后续工具入口、
LLM 可见性与执行期硬门禁。只读观察工具保持可用；会写入持久 recall telemetry 的
memory 工具不被误判为只读。focused 反例与独立复审分别见上述 verification 与
rereview 引用。当前任务分支完整门禁与 17 项独立 feature 编译已通过；PR/CI 与
远端 main 交付仍待完成。
