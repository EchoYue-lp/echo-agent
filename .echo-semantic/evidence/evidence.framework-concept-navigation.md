---
schema_version: 1
id: evidence.framework-concept-navigation
kind: evidence
observed_at: 1cb25e80515ea17624fe652be1fd29c096b9a880
source_refs:
  - README.md
  - README.zh.md
  - docs/en/README.md
  - docs/zh/README.md
  - docs/en/architecture.md
  - docs/zh/architecture.md
  - docs/en/concepts.md
  - docs/zh/concepts.md
  - docs/en/lifecycles.md
  - docs/zh/lifecycles.md
  - docs/adr/0040-framework-concept-documentation-authority.md
  - echo-agent-learning/tests/documentation_contract.rs
supports: [behavior.workspace-composition, behavior.agent-turn-lifecycle, behavior.context-memory-lifecycle, behavior.task-subagent-execution, behavior.effect-permission-execution, behavior.observation-persistence, behavior.extension-publication, behavior.protocol-projection, behavior.sdk-facade-routing, rule.framework-layer-ownership, rule.turn-terminal-authority, rule.context-persistence-separation, rule.task-subagent-authority, rule.permission-effect-order, rule.fact-projection-separation, rule.extension-generation-authority, rule.protocol-role-separation, rule.sdk-rust-authority]
limitations:
  - 概念导航不修改runtime行为，不关闭任何open Finding
  - 结构合同不能单独证明所有自然语言承诺正确
  - 完整workspace合并门禁、docs.rs发布渲染和远程CI尚未执行
---

# Framework concept navigation 证据

## 支持的结论

中英文正式文档现各有Architecture、Core Concepts和Lifecycles三个基础入口。Architecture以Cargo的11-package topology、root facade、SDK protocol/Host和learning consumer组织分层；Concepts以identity、owner、scope/persistence和non-responsibility区分Agent、Session、Conversation、Invocation、Turn、Task、Plan、Subagent、Context、Checkpoint、Journal、Projection、Trace、Delivery和限定Revision；Lifecycles以trigger/admission、authority、event/effect、cancel/failure、terminal、recovery/cleanup和projection串联七条跨领域流。

Root README的Architecture段已收窄为分层入口，保留了已由Cargo metadata校验的feature、workspace和example摘要。双语docs index提供同序Start Here。ADR0040记录三页方案、事实源优先级、open Finding承诺上限、兼容与回滚。

`foundational_framework_docs_are_routed_and_structurally_paired`先在六页与12个导航链接缺失时稳定red，发布后验证文件/入口存在、双语heading profile、冻结的table/fence结构、稳定concept token和本地链接；独立反例验证双侧同时删节/表/diagram或交换导航顺序都不能通过。完整documentation contract 10、all-feature example contracts 21、root facade smoke 10、目标Clippy `-D warnings`、learning crate check和formatter已通过。Clippy首次命中Rust 1.97 `manual_is_multiple_of`，使用同仓`is_multiple_of(2)`模式单点修正后同一命令与完整组合均通过。

## 来源与范围

概念内容来自已有Capability Map、Behavior、Rule、source/tests、ADR和resolved Evidence；业界参考只用于文档结构与概念分层。六页只路由到已有领域文档和Cargo/example-contract消费者，不复制完整API/config。

## 已知缺口

当前71个open Finding仍是实现与细节文档的已知边界。基础文档使用限定名称和保守承诺，但不取代各Finding后续repair/verification/rereview。

独立review先后发现7个Important文档/合同过度承诺，均已按源码调用顺序、owner-specific限定和反例测试修正。最终review为PASS，Critical、Important、Minor均为0。

Review后完整documentation contract 10、all-feature example contracts 21、facade smoke 10、目标Clippy/check、formatter、semantic strict/change-evidence和92/92 Finding-Issue对账全部通过。Cargo manifests、example源码、SDK contract/sdks和runtime路径零diff；71个open和21个resolved Finding状态不变。
