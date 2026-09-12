---
schema_version: 1
id: discovery.sdk-facade-baseline
kind: discovery
source_snapshot:
  base_revision: 4af71b1d40558436350efd16a6f320c0f2193745
  content_digest: e28cd89039dec2d12836a93492e0cd1db58369abfd07bb720a4b3008d9bcb4a0
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
  - intrinsic 语言行为与逐项领域/失败语义证据
  - 三语言整体 Parity complete 状态与最终发布检出证据
  - 全仓 inventory 与 behavior model closure
---

# SDK facade 首次基线发现

## 扫描范围

扫描root facade inventory、协议生成器、Host dispatcher/handle/stream/extension、真实E2E与三语言Client基线。

## 候选事实

候选边界是标准ACP、core profile、feature family、source operation、extension bridge和language intrinsic共同组成的单一SDK facade。

## 归并结果

建立一张高风险能力图、一条路由行为、一条Rust唯一权威规则和一份合同证据；全仓其它能力暂不建模。

## 未决项

Plan 8机械闭合、focused运行证据、完整门禁和三语言可执行 route baseline 已完成；intrinsic 语言行为、可执行示例和全仓 inventory/behavior model 仍保持开放。
