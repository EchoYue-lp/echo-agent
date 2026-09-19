---
schema_version: 1
id: evidence.plugin-generation-publication-authority-verification
kind: evidence
observed_at: source:e0466fe1f836ebe116374a7941ec3b4edbab8b0c9346247efa66405841a178f9
source_refs:
  - src/plugin/prepared.rs
  - tests/facade_smoke.rs
  - echo-agent-learning/examples/demo56_plugin_system.rs
  - echo-agent-learning/tests/documentation_contract.rs
  - docs/en/32-plugin-system.md
  - docs/zh/32-plugin-system.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - A local HTTP fault server covers MCP cancellation; no remote server or embedding application reload transaction was exercised
  - Combined gates, semantic snapshot and integrated review receipts are owned by evidence.foundation-36-72-51-integration-verification
  - Remote-main delivery remains outside this focused evidence
---

# Plugin target publication authority verification

## 支持的结论

The first red test failed to compile because target-scoped publication, typed stale errors and
generation-bound receipts did not exist. Independent Integrator and multi-server MCP cancellation
red tests then failed on counter collision and a missing first-server receipt. After implementation,
default prepared tests passed 15/15 and MCP-feature prepared tests passed 19/19. They cover stale
prepared and receipt rejection,
two independent Agents using cloned integrators, altered and foreign receipt rejection, duplicate
publication, idempotent withdrawal, cancelled apply and withdraw with pending cleanup, failed
MCP close and retry for both partial apply and active withdrawal, process-wide ordering for two
independent Integrators on one Agent, and no component file reads on apply/rollback. The
multi-server test uses the actual package `mcp.json`, a local HTTP server that completes the first
MCP handshake, and a second HTTP server that holds initialize. On cancellation, the first active
client is present in the receipt and rollback disconnects it; the second reserved cleanup scope
remains in the pending receipt.

## 来源与范围

The `mcp` feature test covers partial apply, failed first close, retained retry receipt, and
successful cleanup before same-generation retry. Facade and documentation tests keep the public
handle, bilingual guide and compiled demo aligned. This evidence covers the framework target
boundary only; it does not claim callback, persistent registry or product reload atomicity.

`cargo test -p echo_agent --lib plugin::prepared::tests --locked` passed 15/15;
`cargo test -p echo_agent --lib --features mcp plugin::prepared::tests --locked` passed 19/19;
`cargo test -p echo_agent --test facade_smoke --locked` passed 9/9;
`cargo test -p echo-agent-learning --test documentation_contract --locked` passed 12/12;
`cargo run -p echo-agent-learning --example demo56_plugin_system --locked` completed with
published and withdrawn generation 1. `cargo check -p echo_agent --no-default-features --locked`,
`cargo fmt --all -- --check`, focused MCP lib Clippy with `-D warnings` plus panic lints,
and MCP all-target Clippy with `-D warnings`
exited 0. Cargo ran with debug info disabled, incremental disabled and two build jobs after a
lane-local `cargo clean` restored disk space.

## 集成收据

本 Evidence 只保留 #72 focused、fault server、facade、文档与示例结果。合并源码上的
workspace、feature、semantic 与 integrated review 精确收据统一归
`evidence.foundation-36-72-51-integration-verification`。

## 已知缺口

本地集成收据不覆盖 remote server、embedding application reload transaction 或
remote-main delivery。
