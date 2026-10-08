---
schema_version: 1
id: evidence.rust-199-lint-repair
kind: evidence
observed_at: d8a73701768a3433c8b2afd5de08da573754673c
source_refs: [CHANGELOG.md, Cargo.toml, Cargo.lock, echo-core/src/circuit_breaker.rs, echo-integration/src/channels/session.rs, echo-integration/src/lsp/manager.rs, echo-integration/src/mcp/client.rs, echo-integration/src/mcp/mod.rs, echo-state/src/audit/mod.rs, echo-state/src/journal/mod.rs, src/acp/session.rs, src/agent/react/capabilities.rs, src/agent/subagent/executor.rs, src/plugin/prepared.rs, src/trace/mod.rs]
supports: [behavior.workspace-composition, rule.framework-layer-ownership]
limitations: [本地候选验证不等于远端交付, SDK 和 website 没有公共行为变化，无需修改]
---

# Rust 1.99 lint repair

## 支持的结论

Rust 1.99 将 atomic fetch_update 标为 deprecated；标准库源码显示 try_update 自 1.95 即稳定，原 fetch_update 转调 try_update。因此改名保持本项目 Rust 1.95 minimum、ordering 与 checked/saturating arithmetic。

旧 async_recursion 宏给 Boxed Future 额外添加无理由 must_use，触发新版 Clippy double_must_use。private dispatch_inner 改为等价的显式 lazy BoxFuture，整个原执行体仍在 async move 内，不提前执行 admission 或副作用；无其它使用点的宏依赖移除，锁文件只删对应包/边。

## 来源与范围

既有 workspace/learning tests 和 examples 纳入完整 all-target/all-feature 门禁；manifest 改动额外验证 18 个独立 feature。未改公共 ABI、SDK contract、应用策略或状态权威；source identity inventory 是漂移遥测，不作为修复完成度。

## 已知缺口

Rust 1.99 本地 fmt、两档严格 Clippy、workspace/all-target/all-feature tests、workspace no-default 与 18 个独立 feature check 全部通过。命令链真实退出 0；examples 已进入 all-target 链路。既有历史 Evidence 改绑可恢复的原 main，而非伪装为重新验证。本次没有运行独立 SDK、website 或平台主机验收。
