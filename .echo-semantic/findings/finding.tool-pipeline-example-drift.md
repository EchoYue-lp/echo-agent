---
schema_version: 1
id: finding.tool-pipeline-example-drift
kind: finding
type: evidence_gap
status: open
severity: medium
primary_focus: contract_evidence
focus: [time_lifecycle, result_side_effect]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution, behavior.workspace-composition]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.workspace-structure, evidence.tool-pipeline-example-repair]
audit_refs: [audit.observation-persistence-delivery.contract-evidence, audit.tool-permission-sandbox.result-side-effect, audit.tool-pipeline-example-rereview]
decision_refs: []
repair_evidence_refs: [evidence.tool-pipeline-example-repair]
verification_evidence_refs: [evidence.tool-pipeline-example-verification]
rereview_audit_refs: [audit.tool-pipeline-example-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# demo64 Tool pipeline 合同与生产顺序漂移

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/101

## 问题

最初的 executable contract 声明 13 stages，包含已不存在的 ParseValidateStage，并把 Trace 放在 PostHook 前。一次手工同步改成了 16-stage 静态列表；Guard 方向修复加入 ToolInputGuardStage 后，生产默认管线已是 17 stages，示例再次漂移。中英文工具文档仍保留最初的旧顺序。

## 触发条件与影响

文档、学习示例或审查以 demo64 为事实源时，会得到错误的权限、观察和副作用顺序；测试只打印静态数组，无法检测生产漂移。

## 证据

在 `69a3b864` 上，`echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs` 的静态总览与 `src/agent/react/run/pipeline.rs` 的真实顺序分别为 16 和 17 阶段；真实执行 tracing 给出同样的 17 阶段。

## 处理记录

候选修复从真实默认管线调用的结构化 tracing 读取阶段名称，动态展示顺序，并检查关键权限、守卫、执行和终态观察关系；不新增公开 runtime API。独立复审已通过，完整门禁、PR/CI 和 main 交付后再判定关闭。
