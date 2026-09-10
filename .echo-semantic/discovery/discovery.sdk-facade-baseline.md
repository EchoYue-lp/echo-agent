---
schema_version: 1
id: discovery.sdk-facade-baseline
kind: discovery
source_snapshot:
  base_revision: fcb5a46411b8d976583cbde97835dc0789046c7e
  content_digest: d27e9085f3b853752d308cafcd719ab4059427b10f0cc9106c7412f6d2d47906
scope: root echo_agent facade到ACP与多语言SDK Host适配边界
inspected_paths:
  - contracts/sdk
  - echo-sdk-protocol/src
  - echo-sdk-host/src/core_profile
  - echo-sdk-host/tests
  - sdks
  - src/acp
candidate_refs: [map.sdk-facade-parity]
unresolved:
  - Plan 8严格复审与完整workspace/feature验证
  - 三语言完整manifest状态和最终clean-checkout证据
---

# SDK facade 首次基线发现

## 扫描范围

扫描root facade inventory、协议生成器、Host dispatcher/handle/stream/extension、真实E2E与三语言Client基线。

## 候选事实

候选边界是标准ACP、core profile、feature family、source operation、extension bridge和language intrinsic共同组成的单一SDK facade。

## 归并结果

建立一张高风险能力图、一条路由行为、一条Rust唯一权威规则和一份合同证据；全仓其它能力暂不建模。

## 未决项

Plan 8机械闭合、focused运行证据和三语言源码门禁已完成；严格复审、完整门禁与语言SDK后续outcome尚未完成，inventory和behavior model保持开放。
