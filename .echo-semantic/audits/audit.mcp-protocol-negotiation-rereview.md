---
schema_version: 1
id: audit.mcp-protocol-negotiation-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: contract_evidence
freshness: examined
revision: eadf1a3d5a498bdbccd3742a7e1a457cb27b172d
finding_refs: [finding.mcp-version-doc-drift]
challenges:
  supported-version-selection:
    revision: eadf1a3d5a498bdbccd3742a7e1a457cb27b172d
    source_refs: [echo-integration/src/mcp/client.rs, echo-integration/src/mcp/types.rs]
    evidence_refs: [evidence.mcp-protocol-negotiation-repair, evidence.mcp-protocol-negotiation-verification]
  unknown-version-settlement:
    revision: eadf1a3d5a498bdbccd3742a7e1a457cb27b172d
    source_refs: [echo-integration/src/mcp/client.rs, docs/en/08-mcp.md, docs/zh/08-mcp.md]
    evidence_refs: [evidence.mcp-protocol-negotiation-repair, evidence.mcp-protocol-negotiation-verification]
---

# MCP protocol negotiation 独立复审

## 审查范围

复审 client initialize 响应的版本校验、typed 失败、initialized 通知时序、transport 关闭
结算、共享四版本权威及中英文合同。

## 已检查故障假设

验证 server 选择任一受支持旧版本时 client 是否错误拒绝；选择未知版本时 client 是否仍发送
initialized、发布 capability 或遗留未关闭 transport；文档是否继续只声明单个 latest 版本。

## 实际实现路径与证据

Client 直接读取共享 `SUPPORTED_PROTOCOL_VERSIONS`，在所有初始化副作用之前拒绝未知选择。
测试遍历完整共享集合，并对未知版本断言 typed error、零 initialized 通知与 transport 关闭。
中英文文档列出同一集合并描述失败合同。

Reviewer 检查提交 `eadf1a3d` 后未发现 Critical、Important 或 Minor 问题，结论 PASS。

## 问题记录

`finding.mcp-version-doc-drift` 具备 repair、verification 与独立 rereview 证据，可标记 resolved。
GitHub Issue #67 等待修复进入远端 main 后关闭。

## 残余风险

真实第三方 server 的网络行为与 feature 级互操作不属于本 Finding；完整 workspace 门禁由
集成分支执行。

## 未检查项

未执行完整 workspace gate、全部 feature 矩阵、远端 CI 或其它 open Finding。
