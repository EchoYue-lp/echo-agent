---
schema_version: 1
id: evidence.subagent-depth-target-policy
kind: evidence
observed_at: e71319c53ae5474ed11fc67fd68f09c4bf9524c1
source_refs:
- src/agent/subagent/executor.rs
- src/agent/snapshot.rs
- src/tools/builtin/agent_dispatch.rs
- docs/adr/0094-subagent-tool-boundary-inheritance.md
supports:
- behavior.task-subagent-execution
- rule.task-subagent-authority
limitations:
- Focused local framework target-policy contribution only; no full matrix, EKO GUI/config/run recovery, SDK accepted/pin, native/service or exact main CI acceptance.
- Earlier hard-tool-scope 244-test evidence remains a separate historical checkpoint; no old Finding or Asset status/observed revision is refreshed.
- Raw artifacts are ignored and local to this worktree; hashes identify originals without replacing them.
---

# Exact Subagent actor policy: focused verification

## 支持的结论

The original Subagent executor stamps the request's target actor policy into
its original invocation runtime, including Sync and Teammate. It copies the
value without advancing a second level; the existing dispatch edge alone
owns `child_policy`. Correct Fork, exact-attempt and Team member counting is
preserved. A missing runtime carrier is created through its existing Default,
without inventing run or conversation identifiers.

The original snapshot selects effective policy once with unchanged
invocation-versus-legacy precedence. An explicit invocation with no policy
does not fall back to legacy. An explicit `can_delegate() == false` excludes
`agent_tool` through the same original available/eligible/visible catalog used
by model schemas, tool search and actual execution. Policy-only qualification
keeps the NoDeferred initial `tool_search` schema hidden; explicit visibility
and nonempty hard scopes retain their existing search behavior. A positive
hard scope containing only `agent_tool`, intersected with nondelegating
policy, remains genuinely empty. Policy `None` and empty tool declarations
preserve the generic framework behavior.

## 来源与范围

`observed_at` is the task's Git base, not main delivery. The qualified candidate
is source v3, aggregate digest
`d56ae671ed41130beeebebd90db086de8e9489c44d64e185e957c7c740d6c16c`.
Later documentation and evidence edits preserve every qualified Rust byte.
Only executor and snapshot production changed in this increment;
AgentDispatch production bytes remain identical to the base commit.

| Qualified Rust source | SHA-256 |
| --- | --- |
| `src/agent/snapshot.rs` | `652679a49062a8f130bb632badd8fbda6ca17450da584bfdea6da286c18e2d4e` |
| `src/agent/subagent/executor.rs` | `cf015d92fedb8dcd157ef4fdb4000427cf284013134061b423a0b65680000ce8` |
| `src/tools/builtin/agent_dispatch.rs` | `4497a468a22f469830f5aa56ff5fbed7e6d8ff506768b6f8470ec131d2a78c13` |

Design `871e2c18ec5df7202fde450e3a289ad40800e985f4d2d5e966ebfb827323735c`
and Plan `c1a1e251edd167de78b5197327c13263a96165011fcce3a2a2f000cdd793101c`
bind the same `plan:25322745-56ac-46b5-ad57-bcc9ba046278` Outcome. This evidence
covers its generic framework contribution, not the EKO product policy.

## Executed controls and checks

Both narrow restorations retained current public structures and compiled:

- Removing only the target stamp made the actual Sync child report depth 0
  at maximum 1 while the target request named depth 1. The exact direct-chain
  selector naturally exited 101: 0 passed, 1 failed, 0 ignored, 1085 filtered.
- Passing `None` only to original ToolRuntime policy qualification exposed
  `agent_tool` in real primary model requests despite explicit nondelegation.
  Its exact primary selector naturally exited 101 with the same counts.

The first BEFORE attempt did not run tests: two new fixtures matched
`Arc<SubagentEvent>` as the enum, producing two E0308 compile errors. It is not
behavioral red evidence. The two matches were corrected to borrow `as_ref()`
and clone the owned event name, independently source-reviewed, then the same
original selector was rerun. The failed compile log is retained separately.
After each actual control child fully exited, final source bytes were restored
with fresh mtimes to prevent reuse of the control artifact.

Every final build/check command used `--locked`, the exact owned target,
jobs 1, dev/test debug 0, incremental 0 and an isolated `TS_RS_EXPORT_DIR`;
test commands used `--test-threads=2`.

| Focused command | Actual result |
| --- | --- |
| `cargo test -p echo_agent --features subagent --lib agent_dispatch` | 25 passed, 0 failed, 0 ignored, 1061 filtered; exit 0 |
| `cargo test -p echo_agent --features subagent --lib agent::subagent` | 196 passed, 0 failed, 0 ignored, 890 filtered; exit 0 |
| `cargo test -p echo_agent --features subagent --lib agent::snapshot` | 28 passed, 0 failed, 0 ignored, 1058 filtered; exit 0 |
| `cargo check -p echo-agent-learning --example demo61_agent_factory --features testing` | exit 0 |
| `cargo check -p echo-agent-learning --example demo50_subagent_communication --features subagent` | exit 0 |
| `cargo clippy -p echo_agent --features subagent --lib --tests -- -D warnings` | exit 0 |

The three test sets contain 249 distinct selected passes. All five new test
definitions were actually selected once and passed, covering 12 direct chains
(Sync/Fork/Teammate with maxima 1/2/3/4), original Team member edges, actual
primary and legacy/explicit-None precedence, last-child model/search/forced
call rejection, no extra DispatchStarted event, empty intersections and
NoDeferred behavior. Existing Fresh/queued registration/hard-scope/legacy
controls were rerun in the same filters. No final compiler warning occurred.
Framework maximum 4 remains distinct from EKO's product maximum 3.

After preserving raw logs, source/patch/restore records and test-binary hashes
outside target, the target had no readers. Exact `cargo clean --locked`
returned 0, removing 4123 files / 1.1 GiB; target was absent before metadata
review. No additional Rust lease is retained.

## Original artifacts

Raw originals are in this worktree under
`.echo-semantic/local/subagent-depth-target-policy/`:

- `source-v1/` and `source-v2/` retain prior source/review stages;
  `source-v3/source-freeze.json` and source tar retain the qualified bytes.
- `source-review-v3.json` retains the independent two-fixture increment review,
  SHA-256 `efe59733f32e73f25cc47054e29bbdd65f5e33635417150c0920686d526341bc`.
- `execution/attempt-1-compile-only/` retains initial E0308 output and its
  zero-selected result, separately from both actual assertion controls.
- `execution/before-i2*` and `execution/before-i1*` retain exact mutation,
  original byte/mode/mtime, narrow patch, raw output, natural exit and final
  restoration. `execution/final-source-originals-v3/` retains original bytes.
- `execution/after-*-result.json` and corresponding logs retain all six final
  command lines, features, natural exits, warnings and actual selection counts.
- `execution/focused-observation.json` retains the five exact selected names,
  disjoint 249-test union, source guard and final test-binary identity.
- `execution/rust-lease-return.json`, SHA-256
  `ac8bff3c2b68b10973ac1fbfa17f59c41a24e786c5dcffe46ca1e7f40c5fb20a`,
  retains exact clean, no-reader, disk and target-absence observations.

## 已知缺口

No existing Finding or Asset status/observed revision is closed or refreshed.
Framework default limits, public API shapes, EKO configuration/roles/1..3
policy and SDK accepted/pin are unchanged by this increment. Full feature
matrix, EKO actual GUI/run-binding/recovery/Verification acceptance, native
or remote effects and exact main CI remain independent pending delivery.
The primary explicit-policy test exercises the generic framework boundary;
it is not an EKO Verification product acceptance test.
