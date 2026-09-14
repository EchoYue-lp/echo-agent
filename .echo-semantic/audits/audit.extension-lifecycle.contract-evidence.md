---
schema_version: 1
id: audit.extension-lifecycle.contract-evidence
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: contract_evidence
freshness: examined
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.mcp-version-doc-drift]
challenges:
  mcp-version-doc-and-negotiation:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-integration/src/mcp/types.rs, echo-integration/src/mcp/client.rs, echo-integration/src/mcp/server.rs, docs/en/08-mcp.md, docs/zh/08-mcp.md, scripts/verify.sh]
    evidence_refs: [evidence.effects-extensions, evidence.workspace-structure]
---

# MCP Version 与双语文档合同审计

## 审查范围

审查MCP client当前请求版本、server兼容列表、双语文档与测试/CI执行边界。

## 已检查故障假设

验证文档所称最新版本是否匹配client请求和server协商，现有测试是否覆盖完整兼容列表，以及测试存在是否被误作已运行。

## 实际实现路径与证据

Client发送2025-11-25；server兼容2025-11-25、2025-06-18、2025-03-26、2024-11-05。双语文档仍把2025-03-26写成最新。Server tests未显式覆盖2025-06-18，未发现client验证server返回协商版本；默认CI tests不运行MCP feature，verify脚本虽含all-feature tests但本revision没有执行证据。

## 问题记录

确认mcp-version-doc-drift；runtime/API无需修改，repair限定双语文档并补当前/兼容版本合同覆盖。

## 残余风险

关闭Finding前必须实际运行MCP定向测试；不能以测试源码存在替代结果。

## 未检查项

未执行MCP feature tests、真实server互操作或外部规范兼容验收。
