---
schema_version: 1
id: discovery.sdk-facade-baseline
kind: discovery
source_snapshot:
  base_revision: 6d55fae97367dedb690d9a7d865ed97d0038b253
  content_digest: 64131952ceb6f498fe94fc34482afe3ecf1e1e77f5a1d31a6ff3ce81b7e0eb01
scope: root echo_agent facade到ACP与多语言SDK Host适配边界
inspected_paths:
  - contracts/sdk
  - echo-sdk-protocol/src
  - echo-sdk-host/src/core_profile
  - echo-sdk-host/tests
  - sdks
  - src/acp
candidate_refs: [map.sdk-facade-parity, map.protocol-surfaces]
unresolved:
  - 1441个deferred identity的capability分组、外部用户价值与逐组产品合同决策
  - registry/binary publication明确不在source-first合同范围
---

# SDK facade 首次基线发现

## 扫描范围

扫描root facade inventory、协议生成器、Host dispatcher/handle/stream/extension、真实E2E与三语言Client基线。

## 候选事实

候选边界是标准ACP、core profile、feature family、source operation、extension bridge、identity级SDK scope和language intrinsic共同组成的单一SDK facade。

## 归并结果

SDK 子边界保留一张高风险能力图、一条路由行为、一条 Rust 唯一权威规则和合同证据；全仓其它能力由 `discovery.workspace-baseline` 与父级 maps 建模。

## 未决项

Plan 8机械闭合与三语言route baseline已完成；schema v2把当前外部合同与Host/Rust-only、language intrinsic、helper、deferred分开。Deferred只按capability审查，不阻塞全仓baseline或制造1441个独立任务。
