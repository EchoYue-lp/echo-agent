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
consumer_refs: [https://github.com/EchoYue-lp/echo-agent-sdk/commit/ea21dfc58fa576296a0b1d0d3267c9632d84f0ae]
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
