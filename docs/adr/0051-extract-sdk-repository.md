# ADR 0051: Extract echo-agent SDK into an Independent Repository

- Status: Accepted
- Date: 2026-09-15
- Owners: `echo-agent` framework and `echo-agent-sdk` maintainers

## Status

Accepted.

## Context

The framework workspace currently contains the complete SDK product: the ACP
extension protocol crate, SDK Host, contracts, three language SDKs, SDK
documentation, generators, and SDK CI. The initial import at
`ea21dfc58fa576296a0b1d0d3267c9632d84f0ae` was later superseded by the pushed
source-continuity commit
`b80cf068b2fb69b62913f23260980b4ce2ebf941`, which includes the frozen
framework SDK tree and its filtered history.

Keeping both products in one workspace makes framework-only changes pay the
SDK contract and language-toolchain cost, and leaves ownership ambiguous.

## Options

1. Keep the SDK crates and language assets as default framework workspace members.
2. Move only the two Rust crates and leave contracts and language SDKs in the framework.
3. Move the complete SDK product and leave only the reusable framework runtime and ACP adapter.

## Decision

Move the complete SDK product to `echo-agent-sdk`. The framework keeps its
product-neutral ACP Agent adapter and all Agent/Run/Task/Subagent, event,
cancel, recovery, persistence, and tool authorities. The SDK Host remains a
consumer of `echo_agent`; the framework never depends on SDK crates or wire
types.

The extraction removes these framework-owned paths:

- `echo-sdk-protocol/` and `echo-sdk-host/`;
- `contracts/sdk/`, `sdks/`, and `docs/sdk/`;
- SDK-only generator and language gate scripts.

The root workspace and framework CI are reduced to framework/runtime packages
and the learning consumer. Framework README and bilingual architecture docs
link to the independent SDK repository instead of presenting SDK packages as
workspace members.

## Consequences

- Framework-only builds no longer require the SDK language toolchains or generated
  contract assets.
- The SDK gains an independent version and CI boundary, while cross-repository
  updates require an explicit framework revision pin and a separate integration
  checkpoint.
- Framework and SDK have independent branches, lockfiles, CI, releases, and
  version cadence.
- SDK compatibility is governed by its own accepted external contract and
  explicit framework revision; full Rust public inventory is telemetry, not a
  runtime compatibility gate.
- The extraction is a repository ownership change, not a runtime behavior
  change. SDK Host and protocol semantics are repaired in the SDK repository
  after this framework baseline is frozen.
- The pushed source-continuity commit is the lossless SDK rollback/source
  checkpoint. It retains the initial-import commit as an ancestor; reverting
  the extraction commit restores framework paths without rewriting either
  repository's history.

## Cross-Repository Cutover

The initial extraction commit `1754877996778afac4e4db77ce37c330496760ea`
remains the rollback point and the first exact Host pin. It predates framework
PR #124 and PR #125, so the final cutover uses a two-phase handshake:

1. integrate the extraction with framework `main@0e09324a`, retaining the
   Journal/Workflow and Kubernetes runtime changes while keeping SDK-owned
   paths deleted, then push that candidate revision without merging it;
2. update `echo-agent-sdk` from source continuity so it includes the 9,724-item
   Wave 2 inventory, pins the pushed extraction candidate, and passes its own
   contract, Host, language, dependency, semantic, and remote CI gates;
3. merge the framework extraction, then replace the SDK candidate pin with the
   final framework main commit and revalidate before merging the SDK PR.

Issue #122 stays open across the handshake. A framework Finding may be resolved
once its deletion, equivalence, verification, and rereview evidence is closed,
but neither repository may treat that as proof that the other repository has
completed delivery.

## Verification

The extraction requires Cargo metadata and framework gates, documentation and
learning contracts, semantic continuity evidence for deleted paths, and an
independent review. The independent SDK repository must remain traceable to
source-continuity commit `b80cf068b2fb69b62913f23260980b4ce2ebf941` before the
framework gitlink is advanced; `ea21dfc` is retained only as initial-import
ancestry.
