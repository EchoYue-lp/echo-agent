---
schema_version: 1
id: evidence.plugin-mcp-owner-isolation-verification
kind: evidence
observed_at: source:c692702d1e9c1752aa396348037aea8baab1b4a8f1bbc2979fe95fc5ec9c7323
source_refs:
  - echo-integration/src/mcp/identity.rs
  - echo-integration/src/mcp/mod.rs
  - echo-integration/src/mcp/resource_tool.rs
  - src/agent/react/capabilities.rs
  - src/plugin/prepared.rs
  - tests/facade_smoke.rs
  - echo-agent-learning/tests/documentation_contract.rs
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - Tests use in-process transports and local stdio fixtures rather than remote MCP servers
  - Full workspace gates, independent rereview and remote-main delivery are recorded separately
---

# Plugin MCP owner-qualified identity verification

## 支持的结论

Independent review supplied six blockers. Deterministic red tests reproduced Direct selector
misclassification, Direct lookup falling back to a Plugin, Plugin cleanup retry using the Direct
namespace, and owner replacement cleanup failure leaving stale Agent projections. These tests passed
after reserved-prefix escaping, strict Direct lookup, typed cleanup settlement and symmetric Agent
projection refresh were implemented.

Second-round review found a public builder source-compatibility regression and a missing typed receipt
field in equality. A compile-time facade function-pointer test failed against the typed-only builder,
and a receipt tamper test was accepted before the fix. Both pass after restoring the Direct wrapper,
adding the separate typed builder and including `mcp_connected_ids` in receipt equality.

Focused tests additionally cover Direct plus two same-name Plugin owners, exact wrong-owner refusal,
same-owner replacement, cancelled prepared debt that does not touch another owner, typed close_all
over active and debt clients, Unicode/punctuation selector round-trip, tool component-boundary digest,
same resource URI from two owners, Plugin Hook MCP owner injection, generation fencing, and existing
manager cancellation/debt behavior.

## 工程验证

`cargo test -p echo_integration --features mcp --lib --locked` passed 188/188 after the review fixes.
`cargo test -p echo_agent --features mcp --lib plugin::prepared::tests --locked` passed 20/20.
`cargo test -p echo_agent --features mcp --lib agent::react::capabilities::tests --locked` passed 7/7.
Focused panic-lint Clippy and default `echo_agent` check exited 0. MCP facade tests passed 10/10,
documentation contracts passed 12/12, and demo56 completed its publish/rollback lifecycle while
printing the owner-qualified MCP selector. Formatter and semantic strict/change-evidence checks
also exited 0 on the final candidate.

## 来源与范围

The verification covers the framework MCP manager, ReactAgent projections, Plugin Integrator,
Plugin Hook adapter, public facade, documentation contract and local example. It does not claim
an embedding application's Registry transaction or a remote MCP interoperability result.

## 已知缺口

This candidate evidence does not mark the Finding resolved. Independent rereview, complete applicable
AGENTS gates, semantic verification and remote-main delivery remain required.
