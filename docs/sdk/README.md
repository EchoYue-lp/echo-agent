# echo-agent SDK (standard ACP Host available)

This is the single external entry point for the echo-agent multilingual SDK
program. The SDK's goal is full **functional and semantic parity** between
the Rust framework's public facade and TypeScript, Python and Java — without
rewriting the agent framework in any of those languages.

> **Current status: ACP conformant (standard profile) + core extension profile
> delivered in the Rust Host.** The source-built `echo-agent-sdk-host` passes
> initialize/new/prompt/update/cancel and shutdown scenarios through the
> official v1 Client, and the negotiated `_echo_agent/*` **core profile**
> (Agent/Session/Run handles, full events, ACK/replay, restart recovery) passes
> real-process E2E ([sdk-core-profile.md](sdk-core-profile.md)). Language SDKs
> do not exist yet, so the program still does not claim **Runnable** or full
> parity. See [Status ladder](#status-ladder) for the exact claims.

## What the SDK program is

- **Source-first delivery.** The repository ships source only. Developers
  compile everything (Host, SDK, tests) from the same Git revision. No
  precompiled binaries, npm packages, wheels or JARs are published, and the
  build never downloads prebuilt project artifacts.
- **ACP-first wire.** The stable [Agent Client Protocol
  v1](https://agentclientprotocol.com/protocol/overview) is the only base
  client↔agent protocol. This project never forks or re-declares the
  official JSON-RPC envelope, `initialize`, Session, Prompt, ContentBlock,
  updates or stop reasons — they come from the official
  [`agent-client-protocol`](https://crates.io/crates/agent-client-protocol)
  ecosystem artifacts pinned in
  [`contracts/sdk/acp-baseline.json`](../../contracts/sdk/acp-baseline.json).
- **Two profiles, one authority.** A standard ACP v1 client can use the
  Host's standard profile without knowing anything about echo-agent. The
  full echo-agent SDK profile negotiates `_echo_agent/*` extension methods
  (via the official `_meta` capability mechanism) to cover the complete
  public facade losslessly. Both profiles project the same Rust execution
  and state authority — the wire never becomes a second framework.

Details of the two profiles, the extension namespace and the error/lossless
scalar rules live in [protocol.md](protocol.md). The delivered core profile —
negotiation, handles, events/ACK/replay and recovery semantics — is specified
in [sdk-core-profile.md](sdk-core-profile.md).

The implemented Rust adapter and its current method/content boundary are
documented in [acp-agent-adapter.md](acp-agent-adapter.md).
Build, configuration and lifecycle instructions for the executable are in
[acp-standard-host.md](acp-standard-host.md).

## Contract artifacts

| Artifact | Purpose |
|---|---|
| `contracts/sdk/acp-baseline.json` | Pinned official ACP wire version (1), crate and schema artifact versions; tests assert the lockfile matches. |
| `contracts/sdk/toolchain.json` | The exact nightly toolchain used for rustdoc-JSON inventory generation. Contributors only; normal builds never need it. |
| `contracts/sdk/public-api.txt` | Deterministic root-facade snapshot with expanded workspace re-exports, members, fields, variants and API-shape digests. |
| `contracts/sdk/parity-manifest.schema.json` | Machine schema for facade identities, signatures, feature availability, adapter obligations and language mappings. |
| `contracts/sdk/parity-manifest.json` | Every facade item classified by an explicit semantic rule, ACP relationship, feature condition, adapter operation and per-language mapping/test status. Entries use one JSON line each so diffs remain reviewable. |
| `contracts/sdk/schema/echo-agent-extension-v1.schema.json` | Generated JSON Schema of the `_echo_agent/*` extension DTOs and method catalog. |
| `contracts/sdk/fixtures/extension/v1/` | Golden fixtures: valid samples must round-trip losslessly, invalid samples must be rejected deterministically. |
| `contracts/sdk/source-contract.json` | Small generated source-compatibility digest (Cargo.lock + facade inventory + parity manifest) embedded by the Host and matched by the Client hello. |

The generating code lives in the workspace member crate
[`echo-sdk-protocol`](../../echo-sdk-protocol/) (`publish = false`). All
artifacts are machine-generated; regeneration is deterministic and
byte-stable:

```bash
# regenerate after an intentional facade or contract change
cargo run -p echo-sdk-protocol --bin export_schema --locked -- --update

# read-only drift check (also run by scripts/verify.sh and CI)
./scripts/check-sdk-contracts.sh
```

## Status ladder

The SDK program distinguishes the following statuses; each implies the
previous ones.

| Status | Meaning | Reached |
|---|---|---|
| **Design** | The design document is agreed | ✅ |
| **Contract** | Protocol contracts, schema, parity manifest exist and pass drift gates | ✅ |
| **ACP conformant** | A standard ACP v1 client passes the supported profile against a real source-built Host | ✅ |
| **Core extension profile** | The negotiated `_echo_agent/*` core families run against a real Host with typed lifecycle, events, replay and recovery | ✅ (Rust Host only) |
| **Runnable** | A real Host plus at least one language's full SDK extension path executes end-to-end | ❌ language extension path not started |
| **Parity complete** | TypeScript, Python and Java all pass the full facade/all-features parity suite | ❌ not started |
| **Published** | Registry/binary publication — **explicitly out of scope**; this design ships source only | never (by design) |

Only *Parity complete* justifies claiming "all public Rust capabilities are
available from the SDK". Until then, language coverage rows in the parity
manifest honestly read `not_implemented`.

## For contributors

- The inventory toolchain is pinned in
  [`toolchain.json`](../../contracts/sdk/toolchain.json); install it with
  `rustup toolchain install <toolchain>` if you intend to regenerate
  contracts. Nothing in a normal build installs it for you.
- Any new public facade item or signature change appears in the inventory;
  the parity manifest check then **blocks CI** until its semantic mapping and
  generated artifacts are reviewed. Cross-crate glob re-exports are expanded
  from matching workspace rustdoc documents instead of stored as `::*`
  placeholders. Public registry re-exports use the exact locked dependency's
  rustdoc JSON; procedural and declarative macros carry behavior-source
  digests so helper/body changes cannot bypass drift detection.
- Extension versioning, digest and compatibility rules: see
  [protocol.md](protocol.md#versioning-and-compatibility).

## Related reading

- Design document: `docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md`
  (repository-internal, the authoritative design source)
- ADR 0028: `docs/adr/0028-source-first-multilanguage-sdk-runtime.md`
- Official ACP documentation: <https://agentclientprotocol.com/>
- Official Rust SDK: <https://github.com/agentclientprotocol/rust-sdk>
