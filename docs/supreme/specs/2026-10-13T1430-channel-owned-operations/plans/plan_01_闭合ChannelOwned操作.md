---
schema_version: 4
slug: channel-owned-operations/plan
outcome:
  summary: ChannelManager所有真实异步路径在释放注册表锁后调用原插件且保留唯一清理权威
  acceptance:
    - 原health和start/send/stop/close metadata重入反例真实通过，失败/取消/重试不丢owner
    - SDK三语言相同场景、适用完整门禁、独立复审及精确交付快照通过
out_of_scope:
  - 新增channel入站wire或权限策略
  - SDK第二lookupmap/生命周期状态机
  - 完整Rust public identity自动成为跨语言API
design_ref: docs/supreme/specs/2026-10-13T1430-channel-owned-operations/design.md
design_sections:
  - ref: design.md § 核心结构与数据流
    digest: sha256:770e277b36640e33ed2cdad4b29c648ea4f8d832508d318a82c1112c084e3d93
  - ref: design.md § 生命周期与异常
    digest: sha256:ba16efad74282f151854e603257c842cc9840862a9d8f5b7c3474e70f4d85497
  - ref: design.md § 验收标准
    digest: sha256:aea8d1c042f3caaf37f32463392819c7293d30ecbc279beba752173f2f0cf9f0
delivery_ref: null
todos:
  - id: canonical-owned-channel
    summary: 框架同entry提供owned引用和生命周期操作，保留旧公共访问能力
    files:
      - echo-integration/src/channels/manager.rs
      - docs/adr
      - docs/en
      - docs/zh
    acceptance:
      - metadata不依赖entry操作锁；原handler/stopped责任收敛；取消、失败、批量快照及get_mut行为具真实回归
  - id: sdk-short-guard-adaptation
    summary: SDK所有现有channel调用和close路径切换为取owned操作后释放manager guard再await
    files:
      - ../echo-agent-sdk/echo-sdk-host/src/core_profile/facade/integrations.rs
    acceptance:
      - 不重建proxy/第二registry/重复批量loop；原真实health重入和三语言场景通过且不增加wire
artifact_id: plan:0504903c-5755-40b6-adb6-97139e3f2078
lifecycle: ready
---
