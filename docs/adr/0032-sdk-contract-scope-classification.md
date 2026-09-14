# ADR 0032: SDK Contract Scope Classification

- Status: Accepted
- Date: 2026-09-13
- Owners: `echo-agent` framework, SDK contract maintainers

## Status

Accepted.

## Context

ADR 0031 retained all 9,682 canonical Rust facade identities as deterministic
drift telemetry, but the manifest still lacked a direct answer to a different
question: which identities make up the current cross-language SDK contract?

The existing fields answer adjacent questions. `classification` describes an
item's wire or shape role, `route` describes how the Host or native SDK reaches
it, and `languages.*.status` records implementation evidence. An `intrinsic`
route can contain a delivered language-native value, a Rust-only construct, a
process-local Host resource, a test helper, or an externally useful capability
that has not yet been accepted into the SDK contract. Treating those cases as
one backlog recreated the per-identity completion problem.

Mature interface systems separate a language-neutral contract from generated
or idiomatic language artifacts:

- [Smithy service types](https://smithy.io/2.0/spec/service-types.html) define a
  service closure and operation input, output, and error shapes; generated
  artifacts may adapt names and shapes to a language.
- [gRPC core concepts](https://grpc.io/docs/what-is-grpc/core-concepts/) define
  remote methods and payload messages in an IDL before generating language
  clients and servers.
- [OpenAPI](https://spec.openapis.org/oas/latest.html) defines a programming
  language-agnostic API description that code generators project into multiple
  languages.

The shared pattern is that service operations and messages define the portable
contract. Every public symbol in one implementation language is not
automatically part of that contract.

## Options Considered

1. Continue using `route.surface == intrinsic` as the SDK backlog boundary.
   This is simple but mixes delivered native values with Rust/Host-only and
   undecided capabilities.
2. Hand-curate a second list of external SDK identities. This can express
   intent but creates a parallel authority and returns governance to thousands
   of manual row decisions.
3. Add a deterministic, identity-level scope to the existing manifest while
   keeping route and implementation status independent.

## Decision

Choose option 3. `ManifestEntry.sdk_scope` is the only consumer-facing scope
classification and has five values:

| Scope | Meaning |
| --- | --- |
| `external_contract` | Current accepted cross-language contract; TypeScript, Python, and Java implementation evidence is checked separately. |
| `host_or_rust_only` | Process-local or Host-owned runtime authority that is not a language-local facade value. |
| `language_intrinsic` | Rust syntax or trait/callback machinery represented idiomatically, not symbol-for-symbol, in other languages. |
| `internal_helper` | Public testing support retained in the Rust inventory but not an SDK product surface. |
| `deferred` | Potentially useful capability that requires capability-level product and contract review. |

The generator applies one fixed precedence independent from implementation
status:

1. a non-intrinsic Host route, or an intrinsic identity registered in a named
   accepted capability contract group -> `external_contract`;
2. an identity outside the external contract under `echo_agent::testing::*` ->
   `internal_helper`;
3. a Rust language surface, trait implementation, or callback type
   alias -> `language_intrinsic`;
4. a process-local, Host-owned, or language-local opaque resource
   -> `host_or_rust_only`;
5. every other identity outside the accepted contract -> `deferred`.

Language status is evaluated afterwards. An accepted external identity that is
`not_implemented` or `in_progress` stays external and fails the language gate;
it cannot silently move to `deferred`. Intrinsic capability groups are accepted
by their stable named contract group, not by observing a `done` status.

Aliases inherit the canonical identity's scope and never count independently.
The current canonical distribution is:

| Scope | Count |
| --- | ---: |
| `external_contract` | 5,607 |
| `host_or_rust_only` | 1,765 |
| `language_intrinsic` | 780 |
| `internal_helper` | 90 |
| `deferred` | 1,441 |

The additional external identity is
`TaskGraphCommit::expected_executions`, a typed precondition on the existing
`value:task` route. The 551 identities that use an intrinsic route but already have three
language implementations remain `external_contract`. Route mechanics do not
downgrade delivered behavior.

`deferred` is not a rejection or a set of 1,441 tasks. It is a capability
backlog. An item moves into the external contract only when an externally
useful capability, Host/native authority, behavior contract, and language
evidence exist. A scope field must never be edited to bypass those steps.

`ParityManifest.schema_version` advances from 1 to 2 because `sdk_scope` is a
new required field. The extension protocol version and facade operation catalog
do not change: this ADR changes governance metadata, not wire methods, handlers,
or runtime behavior.

## Consequences

- SDK completeness is measured against `external_contract`, not every Rust
  facade identity and not every non-intrinsic route.
- `languages.*.status` remains implementation evidence and cannot be replaced
  by scope.
- The complete Rust inventory and its signatures remain the drift authority;
  no public Rust API is removed or declared dead because an application does
  not consume it.
- Deferred work is reviewed and delivered by capability or Finding, never by
  identity count.
- The operation catalog remains language-neutral and byte-stable because scope
  belongs to identities, not aggregate routes.

## Verification

- Generate the manifest and schema through `export_schema --update`; never edit
  the generated manifest manually.
- Assert the five exact canonical counts, alias inheritance, and representative
  identities in Rust and all three language catalog tests.
- Require every `external_contract` mapping to be `done` with a named contract
  test.
- Verify the facade operation catalog hash is unchanged.
- Run SDK contract, language source, and semantic change-evidence gates.
