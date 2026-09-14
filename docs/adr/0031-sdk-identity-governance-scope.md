# ADR 0031: SDK Identity Inventory Is Not Project Semantic Completion

- Status: Accepted
- Date: 2026-09-13
- Owners: `echo-agent` framework, SDK contract maintainers

## Status

Accepted.

## Context

The SDK facade inventory expanded the root Rust public surface into 9,682
canonical identities. An identity may represent a type, field, enum variant,
method, trait implementation, stream, handle, or operation. This inventory is
useful for detecting facade drift and proving that a specific Rust item has a
TypeScript, Python, and Java disposition.

It is not the right unit for repository-wide semantic governance. Advancing one
field or method at a time made SDK mapping volume look like project progress,
although the framework's state authorities, lifecycles, recovery behavior, and
side-effect boundaries had not yet been modeled as a whole.

After PR #23, 5,606 canonical identities are implemented in all three language
SDKs. The remaining 4,076 identities all use the `intrinsic` route surface.
Standard ACP, core extension, family, bridge, invoke, and value route surfaces
have no remaining not-done canonical identities.

## Options Considered

1. Continue opening one pull request per missing canonical identity until all
   9,682 identities are implemented in all three languages. This preserves a
   simple counter but spends most work on language-local and process-local
   shapes rather than framework behavior.
2. Delete the identity inventory and keep only hand-written SDK documentation.
   This removes excessive work but also loses deterministic drift detection and
   traceability from Rust facade items to language dispositions.
3. Keep the complete inventory as SDK telemetry and a classified backlog, while
   making semantic boundaries and Findings the unit of project governance.

## Decision

Choose option 3.

Repository-wide semantic governance is organized by `Capability`, `Behavior`,
`Rule`, state authority, lifecycle, `Finding`, and `Evidence`. Completion is
based on closed inventory and behavior-model coverage, dispositioned Findings,
and verified engineering evidence. It is never derived from the number of SDK
identity rows marked `done`.

`contracts/sdk/parity-manifest.json` remains the only canonical SDK identity
inventory. Its totals, language statuses, classifications, and route surfaces
remain machine-checked drift data. Missing intrinsic mappings form an
independent SDK backlog. A future SDK batch requires a real externally useful
capability or a confirmed Finding; identity count alone does not authorize a
slice. PR #23 is the final pull request in the per-identity sequence, and no
per-identity PR #24 is created.

Host facade parity and overall language parity remain distinct. Plan 08 proved
the Rust Host route and bridge boundary. Overall language parity remains open
until the separately scoped SDK contract is satisfied, but it does not block
the whole-workspace semantic baseline or unrelated framework Findings.

### PR #23 squash continuity

The squash has merge base
`f12563c33de96b89baf9312807182f9500baa159` and predecessor revisions
`f12563c33de96b89baf9312807182f9500baa159` and
`37313cd5303ca21b4a232335f342b2b59da554df`. The result keeps the PR branch's
updated `evidence.sdk-contracts` source references and retires these seven
target-main-only source-dependency obligations:

- `evidence.sdk-contracts#source:1239a1b718dbf0fbbacb6295516cc8a2532a5b3c3ab81957411462fe558b0d9d`
  (`sdks/typescript/src/index.ts`)
- `evidence.sdk-contracts#source:22d1578ba412e215fd4debfd0a28a4989e2248db7482688db26e021712935e9d`
  (`sdks/python/README.md`)
- `evidence.sdk-contracts#source:98fa802a0bee147cd5bb2ddc7560a6a7f50da6e11f429d196663ebb1d6d87c27`
  (`echo-sdk-protocol/src/inventory.rs`)
- `evidence.sdk-contracts#source:c39feedc79e0030a353c011d217882a8e4d5ddfad0f830a5dcec7b26bb4cdf99`
  (`contracts/sdk/parity-manifest.json`)
- `evidence.sdk-contracts#source:cb3f0313fe66a8635b44b30c120ad5af1501c5795582e9d036ce09d37bcd26d7`
  (`sdks/typescript/README.md`)
- `evidence.sdk-contracts#source:ce8db8a82970ce189656158ad4ad70c651823774b00d7f98f9b5082e7505fa8f`
  (`echo-sdk-protocol/tests/facade_inventory.rs`)
- `evidence.sdk-contracts#source:db076592749cf914d34bb3989dcd15d1441ce6149788041bbb086faee05df5f5`
  (`sdks/python/src/echo_agent_sdk/__init__.py`)

The reason for retirement is evidentiary, not behavioral: PR #23 changed those
files and refreshed the current evidence to their successor contents. Keeping
the old content-digest obligations would claim that obsolete blobs are still
current dependencies. The compatibility impact is none for runtime behavior,
public API, wire contracts, and language SDK behavior; only the historical
source-dependency fingerprints are retired.

Rollback restores the seven target-main versions to the source-reference set in
`evidence.sdk-contracts`, removes the continuity resolution, and reruns the
two-predecessor comparison before reverting this decision.

## Consequences

- Whole-workspace progress is reported by boundary coverage and Finding status,
  not as `5,606 / 9,682` or another SDK ratio.
- The SDK inventory and its drift CI remain intact; no existing language SDK
  capability is removed or rolled back.
- The next framework governance stage is whole-workspace semantic discovery.
- SDK intrinsic work can proceed later as capability-sized batches without
  blocking framework lifecycle, authority, or recovery work.
- `echo-website` remains unchanged until an externally visible SDK contract is
  intentionally updated.

## Verification

- Recompute canonical totals and not-done route surfaces from the parity
  manifest.
- Validate Plan 07 and Plan 08 lifecycle artifacts.
- Run strict semantic snapshot and high-risk change-evidence validation.
- Compare both squash predecessors with the candidate result and require every
  obligation to be `preserved`, `replaced`, or `retired`.
