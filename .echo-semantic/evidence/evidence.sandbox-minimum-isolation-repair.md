---
schema_version: 1
id: evidence.sandbox-minimum-isolation-repair
kind: evidence
observed_at: bd17c73075d6b3cf8e00877fa0fb10d36694ea54
source_refs:
  - echo-core/src/sandbox.rs
  - echo-execution/src/sandbox/policy.rs
  - echo-execution/src/sandbox/manager.rs
supports: [finding.sandbox-minimum-isolation, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - Manager policy may still fall back below its preferred level when a command has no explicit minimum
  - This source inspection does not establish current-branch tests, full gates, or remote delivery
---

# Issue 83 explicit minimum isolation repair

## 支持的结论

`SandboxCommand::with_minimum_isolation` sets a caller floor. `SandboxPolicy::evaluate_with_limits`
keeps that floor above policy caps. `SandboxManager` checks the selected executor against the
explicit floor before buffered, limited, or streaming execution; `allow_fallback` cannot override
it. `is_available_at` also checks the selected executor's actual isolation.

## 来源与范围

The implementation entered main in `3735f7e0` and remains present at `origin/main@f7c1fef7`.
The manager can still use fallback for a policy preference above the caller's explicit floor;
this does not weaken `minimum_isolation`.

## 已知缺口

Current-branch focused tests, full local gate, and independent rereview have separate
verification and audit receipts. PR/CI, remote main, and Issue closure remain pending.
