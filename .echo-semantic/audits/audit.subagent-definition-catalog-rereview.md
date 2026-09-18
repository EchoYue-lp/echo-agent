---
schema_version: 1
id: audit.subagent-definition-catalog-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: contract_evidence
freshness: examined
revision: d0ff62ec8b17e847d6abc57837327321297c39c6
finding_refs: [finding.subagent-definition-catalog]
challenges:
  pending-versus-executable-catalog:
    revision: d0ff62ec8b17e847d6abc57837327321297c39c6
    source_refs: [src/agent/subagent/registry.rs, src/agent/react/capabilities.rs, src/tools/builtin/agent_dispatch.rs]
    evidence_refs: [evidence.subagent-definition-catalog-repair, evidence.subagent-definition-catalog-verification]
---

# Definition-only Subagent catalog 独立复审

## 审查范围

独立 reviewer 检查 SubagentRegistry、ReactAgent registration API、AgentDispatchTool schema、文档和定向测试。

## 已检查故障假设

检查 definition-only 是否进入模型可见候选、同名 hydration 后是否进入、remove 后是否退出，以及 pending definition 是否仍可被低层查询。

## 实际实现路径与证据

RegistryState 保留 pending definition；get/contains 可见，list_available/list_by_tag/agent_names/executable catalog 只包含 instance 或 factory。Agent tool schema 读取同一 revisioned executable catalog。

## 问题记录

复审未发现阻断项，结论 pass。

## 残余风险

显式程序化 dispatch 仍可请求未装配名字并在解析时失败，这是保留的低层合同。

## 未检查项

未重复执行完整 workspace gate 或远端 CI。
