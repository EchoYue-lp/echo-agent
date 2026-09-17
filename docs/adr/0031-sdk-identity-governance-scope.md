# ADR 0031: SDK Identity Inventory Is Not Framework Semantic Completion

- Status: Accepted under external SDK ownership; superseded as a local framework implementation
- Original date: 2026-09-13
- Current owner: `echo-agent-sdk` contract maintainers

## Status

Accepted.

## Context

The inventory decision is maintained in the independent SDK repository's
[ADR 0031](https://github.com/EchoYue-lp/echo-agent-sdk/blob/b80cf068b2fb69b62913f23260980b4ce2ebf941/docs/adr/0031-sdk-identity-governance-scope.md).
This framework file remains a stable pointer and does not duplicate the SDK
manifest or its generated identity rows.

Historical PR #23 retired source-dependency fingerprints while preserving the
Rust facade and language SDK behavior. The complete inventory decision and
contract scope now live in the independent SDK repository.

## Options

The framework could duplicate the inventory, delete its historical evidence, or
keep this short pointer. The pointer preserves continuity without creating a
second editable SDK authority.

## Decision

Keep the complete identity inventory and its external-contract classification in
the SDK repository. The historical continuity obligations are
`evidence.sdk-contracts#source:1239a1b718dbf0fbbacb6295516cc8a2532a5b3c3ab81957411462fe558b0d9d`,
`evidence.sdk-contracts#source:22d1578ba412e215fd4debfd0a28a4989e2248db7482688db26e021712935e9d`,
`evidence.sdk-contracts#source:98fa802a0bee147cd5bb2ddc7560a6a7f50da6e11f429d196663ebb1d6d87c27`,
`evidence.sdk-contracts#source:c39feedc79e0030a353c011d217882a8e4d5ddfad0f830a5dcec7b26bb4cdf99`,
`evidence.sdk-contracts#source:cb3f0313fe66a8635b44b30c120ad5af1501c5795582e9d036ce09d37bcd26d7`,
`evidence.sdk-contracts#source:ce8db8a82970ce189656158ad4ad70c651823774b00d7f98f9b5082e7505fa8f`, and
`evidence.sdk-contracts#source:db076592749cf914d34bb3989dcd15d1441ce6149788041bbb086faee05df5f5`.
Their historical predecessor revisions are
`37313cd5303ca21b4a232335f342b2b59da554df` and
`f12563c33de96b89baf9312807182f9500baa159`.

## Consequences

Framework semantic governance no longer treats SDK identity count as a local
runtime or compatibility gate. The SDK repository owns the full source and
future drift checks; this pointer remains only for historical continuity.
The product reason is to keep one SDK inventory owner while allowing framework
runtime evolution. Compatibility impact is limited to source-dependency
provenance; runtime, wire, and language behavior are unchanged. Rollback
restores the historical SDK source references and removes the continuity
resolution before rerunning the predecessor comparison.

## Current Boundary

`echo-agent` remains the Rust facade and runtime authority. The SDK repository
stores the complete public inventory as non-blocking drift telemetry; identity
count does not decide framework semantic completion or SDK compatibility.

## Verification

Recompute the inventory and classify accepted contract scope in the independent
SDK repository. Framework changes use framework tests and do not install or
run SDK language gates.
