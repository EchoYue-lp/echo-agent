---
schema_version: 1
id: audit.hook-protected-path-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: permission_external
freshness: examined
revision: bd17c73075d6b3cf8e00877fa0fb10d36694ea54
finding_refs: [finding.hook-protected-path]
challenges:
  hook-allow-short-circuit:
    revision: bd17c73075d6b3cf8e00877fa0fb10d36694ea54
    source_refs: [src/agent/react/run/pipeline.rs, echo-orchestration/src/human_loop/service.rs]
    evidence_refs: [evidence.hook-protected-path-repair, evidence.hook-protected-path-verification]
  effective-input-audit:
    revision: bd17c73075d6b3cf8e00877fa0fb10d36694ea54
    source_refs: [src/agent/react/run/pipeline.rs, echo-orchestration/src/human_loop/service.rs]
    evidence_refs: [evidence.hook-protected-path-repair, evidence.hook-protected-path-verification]
---

# Hook protected-path 独立复审

## 审查范围

独立 reviewer 复核 `#60/#81` 源码、ADR、双语文档、focused 结果与
protected-path 决策链；测试夹具改为 programmatic Hook 后，另一名独立 reviewer
增量复核最终测试差异。本对象只关闭 `#60` 的 Hook 绕过反例。

## 已检查故障假设

检查 PreToolUse 或 PermissionRequest Hook Allow 是否在 protected-path 检查前
短路，以及 Hook/handler 重写输入是否对原输入审批、遗漏有效输入或重复审计。

## 实际实现路径与证据

PermissionStage 在 Hook 决策消费前检查有效输入；handler 重写后复用
PermissionService checker。定向测试覆盖无 Hook、两类 Allow、确定性的
programmatic PreToolUse rewrite 和 handler rewrite；拒绝审计有且仅有一次。
增量 reviewer 确认新夹具仍走 `run_pre_tool_use` 的相同结果合并与 permission 顺序。

## 问题记录

首次独立只读 reviewer 返回 pass、0 action items；测试夹具变更后的增量 reviewer
返回 pass、0 blocking findings。两位 reviewer 均未自行运行测试。`#60` 已具备
repair、verification 与 rereview 证据，
仅表示当前任务分支 Finding resolved。

## 残余风险

完整 workspace 门禁与 17 项独立 feature 检查已通过；PR/CI 与远端 main 交付
尚待完成。Hook 自身 command/http
effect 的策略仍由 trusted extension 边界承担。增量 reviewer 指出当前改写反例不再
同时覆盖外部 command 执行与 Allow 输出；这两部分由现有 parser/execution 测试分层覆盖。

## 未检查项

未连接真实外部审批 provider，也未执行远端 CI 或生产长时间运行。
