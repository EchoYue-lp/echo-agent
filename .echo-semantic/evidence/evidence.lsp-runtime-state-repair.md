---
schema_version: 1
id: evidence.lsp-runtime-state-repair
kind: evidence
observed_at: c04ab97fdbdc7712af36360de5db10cfeeccfec1
source_refs:
  - echo-integration/src/lsp/client.rs
  - echo-integration/src/lsp/manager.rs
  - echo-integration/src/lsp/jsonrpc.rs
  - docs/adr/0043-lsp-derived-handle-lifecycle.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - SDK Host and EKO consumers must adopt the changed load_config contract before cross-repository delivery
  - This evidence does not cover MCP transport close or other extension lifecycles
---

# LSP runtime state repair evidence

## 支持的结论

The client owns one shared runtime boundary for `running`, `initialized`, PID,
restart count, last error, and pending request admission. EOF settlement and
request registration use the same lock, so a request cannot enter after pending
calls are drained. The stdout reader settles EOF, invalid framing, and missing
or malformed `Content-Length`; the writer performs the same settlement and
terminates the child when stdin fails while stdout remains open. Initialization
can mark a client ready only while that same runtime remains running. Shutdown
sends bounded protocol messages, terminates the child, and joins the reader and
writer tasks before returning. Abnormal reader/writer terminals close request
admission first, kill and wait for the child without joining their own task,
then publish `running: false`; terminal status therefore does not precede real
process exit. Intentional shutdown does not invent a transport error or erase
an earlier abnormal terminal error.

The manager retains the last status after removing a client. Explicit restart
attempts consume `max_restarts`, including a failed spawn; exhaustion cannot
spawn another child. A repeated start closes its old client first. Cold
`load_config` rejects live entries; async `reload_config` awaits old teardown
before replacing configurations and extension routes. Old derived handles
remain closed after replacement.

## 来源与范围

Implementation is fixed at commit `c04ab97fdbdc7712af36360de5db10cfeeccfec1`,
including the initial `67abe61da2dab1dbd58168f2a32ee91df678d362` repair and
the `ee1ebb6aca5a6ed45fa2510a8c517d7d18090a7c` atomic-settlement follow-up.
ADR 0043 records the process owner and configuration API choice. This evidence
describes the framework source and does not assert SDK or EKO consumer delivery.

## 已知缺口

Independent rereview, all applicable merge gates, website documentation sync,
SDK/EKO consumer alignment, and remote `main` delivery remain necessary before
the Finding can be resolved.
