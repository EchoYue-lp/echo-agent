---
schema_version: 1
id: evidence.subagent-tool-boundary-inheritance
kind: evidence
observed_at: e4e50396876c5e7e85b52cdc3dbb510bab9c4f4d
source_refs:
- echo-core/src/agent/mod.rs
- src/tools/builtin/agent_dispatch.rs
- src/agent/subagent/context.rs
- src/agent/subagent/executor.rs
- src/agent/snapshot.rs
- src/agent/react/run/stream_channel.rs
- echo-agent-learning/examples/demo61_agent_factory.rs
supports:
- behavior.task-subagent-execution
- rule.task-subagent-authority
limitations:
- Focused local hard-tool-scope contribution only; no full feature matrix, EKO GUI/run binding, SDK, native/service or main CI acceptance.
- Known depth-policy target stamping and schema/search eligibility gaps remain unmodified; no depth-product completion claim.
- Raw artifacts are ignored and available only in this worktree; recorded hashes identify originals rather than replacing them.
---

# Inherited hard tool scope: focused verification

## 支持的结论

`SubagentContext.allowed_tools` and the original invocation/snapshot boundary
retain a known nonempty ancestor tool upper bound through Fresh/history and
Sync/Fork/Teammate/concrete Team dispatch. The original Agent execution lock
precedes catalog intersection with `AgentInvocationContext.allowed_tools`.
Queued registration, role changes, search activation and forced out-of-scope
calls cannot widen that bound. `None` and an empty declaration retain the
historical no-additional-whitelist behavior; a nonempty bound matching no role
tools does not become unrestricted. The original visibility/pipeline remains
the execution authority.

## 来源与范围

`observed_at` is the Git starting point, not a claim that this repair is on
main. The actual qualified Rust candidate was source v3 in the dedicated
`fix/Echoyue/subagent-tool-boundary` worktree; its aggregate source digest is
`655c87278bc6a051240d74ebf9749e9c748ae21956bc38595ad65a432d0bcc9f`.
Later ADR and semantic-mapping edits preserve every Rust source byte.

The current candidate decision is `docs/adr/0094-subagent-tool-boundary-inheritance.md`,
SHA-256 `05c473c1f83f83063472eb1df76b2dc362374eef0b0a8d6246cbc63e77ddf7ed`.
It is a new candidate file and is not represented as a source already present
in the `observed_at` base commit.

| Qualified Rust source | SHA-256 |
| --- | --- |
| `echo-agent-learning/examples/demo61_agent_factory.rs` | `3ad606b9d716ccb9d8897b0a24c772734b24975755b94b576bd414e9b3403c31` |
| `echo-core/src/agent/mod.rs` | `8e93d18f65faf250dd4b5c94e359e50bd9900ff560dfc5cf6ee3d1c422ecc892` |
| `src/agent/react/run/stream_channel.rs` | `eda58622abfbdd4b6a59c393a47abf180cd1625f4ee170813ea37307236c38a6` |
| `src/agent/snapshot.rs` | `6b61f6ee34f548a7c955d6145bfd5ff26ffdfb2f036d56979c3dc728acc363e1` |
| `src/agent/subagent/context.rs` | `59f24567b29854a184221f7caae2e46d2d9d90b76e10f905a2d6a0dd9021982f` |
| `src/agent/subagent/executor.rs` | `02a8b995af52b0b19be726b7803bb0a433b202d395c733823e500b448b0f062c` |
| `src/tools/builtin/agent_dispatch.rs` | `535c7b4f4d6e2c9f14ead6213af602550901cc2f83e267df139f74d685d81c7f` |

## Executed controls and checks

Two narrow restorations of prior semantic branches each compiled and failed
one selected assertion with natural exit 101: 0 passed, 1 failed, 0 ignored,
1080 filtered. The ancestor-loss control exposed `write_file` in actual model
requests; the queued deny-complement control exposed `late_scope_write` after
the original execution mutex wait, shared tool registration and tool search.
The new public structure remained present. These controls are not builds of
the whole earlier revision.

The first BEFORE attempt failed compilation with E0046 because the test wrapper
omitted its required plain `execute_stream` method. No test ran; it is not
behavioral red evidence. The wrapper forwarder was added, independently
reviewed, and both original control selectors were rerun with separate logs.

All final commands used `--locked`, the exact owned target, jobs 1,
dev/test debug 0, incremental 0 and isolated `TS_RS_EXPORT_DIR`. Test commands
used `--test-threads=2`:

| Focused command | Actual result |
| --- | --- |
| `cargo test -p echo_agent --features subagent --lib agent_dispatch` | 22 passed, 0 failed, 0 ignored, 1059 filtered; exit 0 |
| `cargo test -p echo_agent --features subagent --lib agent::subagent` | 196 passed, 0 failed, 0 ignored, 885 filtered; exit 0 |
| `cargo test -p echo_agent --features subagent --lib agent::snapshot` | 26 passed, 0 failed, 0 ignored, 1055 filtered; exit 0 |
| `cargo check -p echo-agent-learning --example demo61_agent_factory --features testing` | exit 0 |
| `cargo check -p echo-agent-learning --example demo50_subagent_communication --features subagent` | exit 0 |
| `cargo clippy -p echo_agent --features subagent --lib --tests -- -D warnings` | exit 0 |
| `cargo clippy -p echo-agent-learning --example demo61_agent_factory --features testing -- -D warnings` | exit 0 |

These are 244 selected test passes, with all nine new controls selected once
by exact name. No final compiler warning occurred. `cargo fmt --all`, its
check and `git diff --check` also returned 0. Original failure output was
retained. After preserving logs, test-binary identity and necessary artifacts
outside target, no readers remained and exact `cargo clean --locked` returned
0, removing 4177 files / 1.1 GiB. The target was absent before metadata review.

## Original artifacts and qualification limits

Raw originals live under
`.echo-semantic/local/subagent-tool-boundary/` in this worktree:

- `execution/cargo-run.json` holds exact commands, features and natural exits.
- `execution/before-v3-manifest.json`, corresponding patches/logs and
  `execution/before-v3-restoration.json` retain the two controls and final-byte restoration.
- `execution/before-ancestor-scope.log` retains the initial compile failure.
- `execution/after-v3-manifest.json` and `execution/final-focused-observation.json`
  retain final results and the nine exact selected test names.
- `source-v3/source-freeze.json` and its source tar retain qualified file hashes;
  `source-review-v3.json` and `implementation-review.json` retain independent
  source and focused-contribution review, with their stated limits.
- `execution/rust-lease-return.json`, SHA-256
  `a317dc74562b37dcd55dc4a628925cd0e91a7d6679a6640f7c8d3accc90e1420`,
  retains exact clean, reader and target-absence observations.

## 已知缺口

No existing Finding or asset status is closed or refreshed by this evidence.
Hard tool bounds passing does not prove target-child depth stamping, last-child
model/search eligibility, EKO's configurable 1..3 policy, persisted run binding
or recovery. Those remain explicit follow-up work under the original owners.
