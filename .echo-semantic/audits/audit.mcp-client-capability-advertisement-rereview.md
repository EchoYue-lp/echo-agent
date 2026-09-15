---
schema_version: 1
id: audit.mcp-client-capability-advertisement-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: contract_evidence
freshness: examined
revision: f30a1fc05153832870c420d5415d436aadb8b07f
finding_refs: [finding.mcp-client-capability-advertisement]
challenges:
  initialize-capability-truthfulness:
    revision: f30a1fc05153832870c420d5415d436aadb8b07f
    source_refs: [echo-integration/src/mcp/client.rs]
    evidence_refs: [evidence.mcp-client-capability-advertisement-repair, evidence.mcp-client-capability-advertisement-verification]
  protocol-version-compatibility:
    revision: f30a1fc05153832870c420d5415d436aadb8b07f
    source_refs: [echo-integration/src/mcp/server.rs, docs/en/08-mcp.md, docs/zh/08-mcp.md]
    evidence_refs: [evidence.mcp-client-capability-advertisement-verification]
---

# MCP client capability advertisement独立复审

## 审查范围

Reviewer检查client initialize构造、server-to-client handler可达性、四版本server协商和双语
文档；transport cleanup与client-side版本拒绝由其它Finding覆盖。

## 已检查故障假设

检查roots/sampling/elicitation/experimental被广告但无handler、存在第二initialize入口重新
广告，以及server接受未知版本。

## 实际实现路径与证据

唯一client入口发送空capabilities，wire测试断言精确`{}`；server四版本回显、未知拒绝。
最终review pass。

## 问题记录

Critical 0、Important 0、Minor 0，#65可关闭。

## 残余风险

未来新增client capability必须新增完整handler和lifecycle。

## 未检查项

未连接公网第三方MCP server。
