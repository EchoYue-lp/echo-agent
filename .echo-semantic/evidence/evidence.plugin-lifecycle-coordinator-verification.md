---
schema_version: 1
id: evidence.plugin-lifecycle-coordinator-verification
kind: evidence
observed_at: source:f4b876a02bb253bb7d9dd92aca7982dbd352bab4f641a3646ff9bc5cb3304c05
source_refs:
  - tests/plugin_coordinator.rs
  - echo-core/src/plugin/lifecycle.rs
  - echo-agent-learning/examples/demo56_plugin_system.rs
  - echo-agent-learning/tests/documentation_contract.rs
  - docs/en/32-plugin-system.md
  - docs/zh/32-plugin-system.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - The same-name MCP matrix reuses typed Direct and Plugin identities; transport fault settlement remains covered by the #72 and #75 evidence
  - Hook cancellation proves operation-local at-most-once attempt, not durable delivery
  - Full workspace gates, independent rereview, remote CI, and mainline delivery remain outstanding
---

# Plugin lifecycle coordinator verification

## 支持的结论

The initial public-facade test failed at module resolution before the coordinator existed. The
all-feature integration suite now passes 14/14 and covers the complete lifecycle sequence,
monotonic reload generation, idempotent reconcile, durable disable/enable intent, uninstall,
shutdown without intent mutation, and restart reconciliation from the persisted registry.

Fault tests prove that callback deactivation debt leaves desired state committed, reports
`ActualPending`, blocks a later operation, and resumes the same receipt. A failed activation is
settled by the lifecycle authority before retry. A `PluginDisabled` Hook future is deliberately
dropped after its attempt marker; retry commits without invoking it again. Direct, plugin A, and
plugin B retain distinct typed MCP identities for the same local server name.

Review regressions additionally prove dependency-first init/activate/loaded and reverse dependency
withdraw/unregister/disabled order even when lexical order conflicts with topology. A coordinator
bound to Agent A rejects Agent B before a disable can change registry revision or enabled intent.
Late callback registration invalidates convergence and activates on the next reconcile. Both an
event await and a publication await can be cancelled while the same receipt remains
`ActualPending` at the exact retry phase.

Generation-wide invalid preparation is rejected in `Preparation` before callback or wiring
withdrawal. Repeating the invalid retry preserves the old generation and callback counts; after
the blocking plugin-data path is repaired, the same operation id reparses files and converges on a
new generation. Publication cancellation uses a local MCP listener and oneshot entry signal rather
than timing sleep; the cancelled target exposes a canonical pending cleanup receipt before retry.
An all-scope startup with a missing dependency converges under the same operation after that
dependency is installed on disk. A Project-only view continues to reject a dependency that exists
only in Local scope, then converges after the dependency appears in Project scope. A duplicate-name
scan failure also preserves the old Registry and actual generation until same-operation retry.

## 已执行验证

- `cargo test --test plugin_coordinator --all-features --locked`: 14/14 passed.
- `cargo test -p echo_core plugin::lifecycle --all-features`: 10/10 passed.
- `cargo test -p echo-agent-learning plugin_publication_docs_and_demo_share_the_coordinator_receipt_contract --all-features`: 1/1 passed.
- `cargo run -p echo-agent-learning --example demo56_plugin_system --locked`: passed.
- `cargo clippy -p echo_agent --test plugin_coordinator --all-features --locked -- -D warnings`: passed.
- `cargo clippy -p echo_agent --lib --all-features --locked -- -D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable`: passed.
- `cargo clippy -p echo_core --lib --all-features --locked -- -D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable`: passed.
- `cargo fmt --all -- --check`: passed.

## 来源与范围

The integration test uses real registry persistence, immutable preparation, Agent publication,
callback lifecycle and programmatic Hook execution. Existing #72/#75 fault suites remain the
authority for transport-level MCP cleanup and owner projection details.

## 已知缺口

Focused tests do not replace the final workspace gates or feature matrix. The candidate does not
claim durable Hook acknowledgement, external MCP transport E2E, remote CI, or mainline delivery.
