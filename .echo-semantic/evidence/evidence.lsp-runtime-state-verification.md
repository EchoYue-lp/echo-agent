---
schema_version: 1
id: evidence.lsp-runtime-state-verification
kind: evidence
observed_at: ee388b5eda47ca4569bee339be20e736ae145020
source_refs:
  - echo-integration/src/lsp/client.rs
  - echo-integration/src/lsp/manager.rs
  - echo-integration/src/lsp/jsonrpc.rs
  - echo-agent-learning/tests/example_contracts/demo55_lsp_tools.rs
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - One opt-in real-language-server smoke test was not run
  - SDK Host and EKO consumer builds are outside this framework-only verification scope
---

# LSP runtime state verification evidence

## 支持的结论

Mock child-process tests exercise EOF after initialization, a pending request
interrupted by EOF, atomic rejection after EOF settlement, writer failure while
stdout remains open, malformed and missing length headers, abnormal-error
retention across stop, clean stop, restart success and exhaustion, failed
initial spawn and failed restart accounting, rejected synchronous config
mutation, async reload teardown and route replacement, repeated start,
retained-handle invalidation, and rejection after manager shutdown. JSON-RPC
header tests include non-ASCII input. EOF, writer failure, and malformed-header
fixtures directly verify that the child has exited before terminal status is
observed, without calling shutdown or restart first. The focused LSP run passed
20 tests with zero failures and one ignored opt-in live-server smoke test.

## 来源与范围

- `cargo test -p echo_integration --features lsp lsp --locked`: 20 passed, 0 failed, 1 ignored.
- `cargo check -p echo_agent --no-default-features --features lsp --locked`: passed.
- `cargo test -p echo-agent-learning --features lsp --test example_contracts contract_demo55_lsp_tools --locked`: 1 passed, 0 failed.
- `cargo clippy -p echo_integration --lib --tests --features lsp --locked -- -D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed after the last source edit.

The Cargo commands reused the existing #111 lane target directory to avoid a
second dependency cache. The focused evidence was followed by the complete framework
merge gate, all 17 isolated feature checks, independent rereview, PR CI, and signed
main delivery in PR #135.

## 已知缺口

The opt-in real-language-server smoke remains a non-blocking environment test. No
required framework verification or delivery evidence remains open.
