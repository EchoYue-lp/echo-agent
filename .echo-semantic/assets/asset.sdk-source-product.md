---
schema_version: 1
id: asset.sdk-source-product
kind: asset
title: Historical echo-agent-sdk source-import product
asset_type: protocol
status: deprecated
risk: high
observed_at: c5f7688212d45d5bdcdbf60342605e8bfb176cae
boundary_refs: [boundary.protocol-surfaces, boundary.sdk-facade-parity, boundary.workspace-architecture]
code_refs: [echo-sdk-protocol, echo-sdk-host, contracts/sdk, sdks, docs/sdk, scripts/check-sdk-contracts.sh, scripts/check-language-sdks.sh, scripts/export-language-sdk-catalog.sh, .github/workflows/rust-ci.yml]
consumer_refs: []
behavior_refs: [behavior.protocol-projection, behavior.sdk-facade-routing, behavior.workspace-composition]
rule_refs: [rule.protocol-role-separation, rule.sdk-rust-authority, rule.framework-layer-ownership]
evidence_refs: [evidence.sdk-contracts, evidence.workspace-structure, evidence.sdk-repository-extraction-equivalence, evidence.sdk-repository-extraction-verification]
finding_refs: [finding.sdk-repository-extraction]
candidate_refs: [asset.framework-acp-adapter]
---

# Historical echo-agent-sdk source-import product

## 资产身份

该资产只记录 initial source-import checkpoint 的完整产品集合，不是当前 owner。

## 来源与消费者

该资产从 `echo-agent` 的 source revision `989e3296` 导入并已推送到独立仓库；当前
source-continuity owner 是 `asset.external-sdk-repository`。

## 生命周期

源快照可追溯、可复制到独立仓库；framework 不再拥有这些产品文件。SDK 后续通过固定
framework revision、external contract 和独立 CI 继续演进。

## 候选关系

它与 `asset.framework-acp-adapter` 不是两个执行 authority：SDK Host 消费 framework ACP
adapter，协议和语言客户端只投影 framework 运行时事实。

## 未知与限制

source-import 检查点仍保留旧 protocol/framework coupling、合同漂移和源 workspace CI
矩阵；它已 deprecated，后续收敛由独立 SDK 仓库的 b80cf06 source-continuity owner 负责。

## 仓库产物定位

| 原定位字符串（完整保留） | 实际角色 | 不可变关联 |
|---|---|---|
| https://github.com/EchoYue-lp/echo-agent-sdk/commit/ea21dfc58fa576296a0b1d0d3267c9632d84f0ae | Initial-import immutable product locator; it is not a framework source path. | https://github.com/EchoYue-lp/echo-agent-sdk/commit/ea21dfc58fa576296a0b1d0d3267c9632d84f0ae |

这两个原commit均真实存在且是公开SDK `7f14a1b9c751b0612ceb42224f83f29d09aea46c`的祖先。原asset_type、code_refs、观察、状态、业务限制和全部既有正文保持。URL是独立仓库产物定位，不是F观察revision内的production_source；此纠正不删除SDK产品义务，其Owner仍为独立SDK及原提取Finding/ADR0051，不把说明链接称为机器跨仓强边或519项连续性通过。
