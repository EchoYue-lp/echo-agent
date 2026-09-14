# ADR 0041: Semantic Governance Continuity from the SDK Baseline

- Status: Accepted
- Date: 2026-09-14
- Owners: `echo-agent` framework, semantic governance maintainers

## Status

Accepted.

## Context

The whole-workspace semantic governance branch starts from
`b21aba01b34e74c93d783a89db895282ba831c3c`. Its final continuity comparison
found twenty obligations whose fingerprints were intentionally changed or
removed while the SDK identity effort was reclassified under ADR 0031 and ADR
0032. The report did not identify a new runtime failure. It identified that the
earlier SDK-only baseline and its source dependencies no longer described the
current project-level governance model.

The final verification task cannot use its task-scoped preflight to authorize
all earlier commits retroactively. It also cannot treat a passing test suite as
proof that changed semantic obligations were preserved. The semantic
continuity contract therefore requires an explicit decision for the four
changed obligations and the sixteen removed obligations.

## Options Considered

1. Expand the final task preflight back to `b21aba01` and claim it authorized
   every preceding repair. This would fabricate historical authorization and
   would hide the real task boundaries.
2. Restore the old SDK-only descriptions and source fingerprints. This would
   make the comparison pass by discarding the accepted workspace governance
   and SDK scope classification.
3. Preserve every unchanged obligation, explicitly reconcile the four changed
   obligations with the canonical SDK contract-scope scenario, and retire only
   the obsolete discovery and source-dependency obligations.

## Decision

Choose option 3. The decision applies only to the predecessor revision
`b21aba01b34e74c93d783a89db895282ba831c3c` and to the candidate result bound
by `evidence.semantic-governance-b21-continuity`. Future comparisons require a
new result-specific continuity decision.

The following changed obligations are resolved as conflicts and converge on
`map.sdk-facade-parity#scenario:sdk-contract-scope`:

| Obligation | Reason |
| --- | --- |
| `behavior.sdk-facade-routing` | Route closure now also distinguishes the accepted external SDK contract from Host/Rust-only, language-intrinsic, helper, and deferred identities. |
| `evidence.sdk-contracts` | The evidence now proves scope classification and later focused repairs instead of treating every Rust public identity as a language parity task. |
| `map.sdk-facade-parity#scenario:source-operation-closure` | Source-operation closure remains mapped, while consumer contract acceptance is owned by the separate contract-scope scenario. |
| `rule.sdk-rust-authority` | Rust remains the semantic authority; the rule now states that SDK scope cannot create a second runtime authority. |

The following discovery obligations are retired because their coarse SDK-only
questions were replaced by the accepted capability-level governance model:

- `discovery.sdk-facade-baseline#unresolved:intrinsic 语言行为与逐项领域/失败语义证据`
- `discovery.sdk-facade-baseline#unresolved:三语言整体 Parity complete 状态与最终发布检出证据`
- `discovery.sdk-facade-baseline#unresolved:全仓 inventory 与 behavior model closure`

The first two concerns continue in the current `deferred` capability backlog
and explicit release limitations. The third was closed by the whole-workspace
inventory and behavior-model baseline; its remaining dynamic unknowns are
listed in `discovery.workspace-baseline` rather than hidden by SDK identity
counts.

The following historical `evidence.sdk-contracts` source-dependency
obligations are retired. The files and their current behavior are not retired;
only the obsolete predecessor blob fingerprints cease to be current evidence:

| Obligation | Predecessor source |
| --- | --- |
| `evidence.sdk-contracts#source:32b420e6f7b28d734215ecb15db11f14e20086bbc7bbc59d8a8ce4454b9167ea` | `echo-sdk-protocol/src/inventory.rs` |
| `evidence.sdk-contracts#source:35635cc0a36bab6f610d6bf3600715478c8366008c5fa3487578cbc57a5991d8` | `echo-orchestration/src/tasks/revisioned.rs` |
| `evidence.sdk-contracts#source:426fa3b6642b1fff7d6f9ab9f1da307282feeca815bf81f516742ee0b28b6e4b` | `sdks/typescript/test/catalog.test.js` |
| `evidence.sdk-contracts#source:4a86b212b87eebc92f0fc432454fef61bbd7f86871f84eff4ccca9331eddb9c3` | `contracts/sdk/facade-operation-catalog.json` |
| `evidence.sdk-contracts#source:4d6c04985154a23a9af488d00c81c5db942a4c8f465c3e5528dc1accf0595866` | `src/improve/loop.rs` |
| `evidence.sdk-contracts#source:4fdfdab6ad12b22f4c3564516428d9790e3e5beb6dfa80ce7329fea36fd5d6bc` | `contracts/sdk/parity-manifest.schema.json` |
| `evidence.sdk-contracts#source:66ac870a436382ca2d401d55887b3f5bf4ad640832831b152d76ac6cb57a9d7c` | `sdks/java/src/test/java/com/echoagent/sdk/FacadeParityTest.java` |
| `evidence.sdk-contracts#source:6f5348656cd4e60d29c836e52c3e24b76a75b040d2055cfac5d1bf0326fa4790` | `contracts/sdk/public-api.txt` |
| `evidence.sdk-contracts#source:89124f4d3b44bd898aa06aac9ccd542314a56b5390047c5032e038525613f63d` | `echo-sdk-protocol/tests/facade_inventory.rs` |
| `evidence.sdk-contracts#source:9d0901fd1f4eb24cbf176b0ed780ff154fc0e52e4b94b256e50fdcd1018cabab` | `sdks/python/tests/test_catalog.py` |
| `evidence.sdk-contracts#source:a59e3b9eb419a7deb16c8211e9d3f13230e9cd8f9e096fd226c1a498c2eca866` | `src/eval/runner.rs` |
| `evidence.sdk-contracts#source:cf8a478a48c3458009224cf19301c8c008604da2120b7faaf7f8d0a9dbfea612` | `src/agent/react/capabilities.rs` |
| `evidence.sdk-contracts#source:fb9558861fd7c0804ffa045d8724c47a859215a176caa6c999f6468a39747ffe` | `contracts/sdk/parity-manifest.json` |

This is an evidentiary retirement, not permission to delete a framework API,
runtime path, SDK artifact, or test. Current source references and the focused
repair Evidence remain authoritative for the successor contents.

## Consequences

- Project semantic completion remains measured by capabilities, behaviors,
  rules, state authorities, lifecycles, Findings, and evidence rather than SDK
  identity totals.
- The SDK contract-scope scenario becomes the canonical replacement for the
  four changed SDK governance obligations. Rust remains the only runtime
  semantic authority.
- The 1,441 deferred identities remain a capability backlog. This decision does
  not mark them delivered and does not close any Finding.
- Each high-risk repair continues to rely on its own task-scoped preflight,
  repair evidence, verification evidence, and independent rereview.

## Compatibility Impact

There is no additional runtime, public API, wire, persistence, or language SDK
compatibility change. This ADR records changes that are already present in the
linear governance branch. Remote CI, merge, release, and the remaining open
Findings are still outside the local final-verification claim.

## Rollback

Rollback means reverting the governance commits after
`b21aba01b34e74c93d783a89db895282ba831c3c`, restoring the predecessor semantic
objects and source dependencies, removing the result-specific equivalence and
continuity Evidence, and rerunning strict snapshot, SDK contracts, engineering
gates, and semantic continuity. Removing only this ADR or its Evidence without
restoring the predecessor semantics is not a valid rollback.

## Verification

- Run the documentation contract for the ADR index update.
- Run semantic diff and strict snapshot validation after refreshing the source
  digest and affected semantic references.
- Compare `b21aba01b34e74c93d783a89db895282ba831c3c` with the committed candidate
  result and require every obligation to be `preserved`, `replaced`, or
  `retired`.
- Require the four conflict resolutions to reference behavior-equivalence
  evidence with matched SDK authority, route, scope, and language-contract
  scenarios.
