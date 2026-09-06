# Source-built standard ACP Host

`echo-agent-sdk-host` is the product-neutral executable for the supported
standard ACP v1 profile. The repository publishes its source, not a binary or
a language runtime. Developers build the Host with their own Rust toolchain
from the same Git revision as the clients and contracts they use.

## Build and validate

The workspace requires the Rust version declared in `Cargo.toml` (currently
1.95 or newer):

```bash
cargo build -p echo-sdk-host --bin echo-agent-sdk-host --locked
target/debug/echo-agent-sdk-host \
  --config echo-sdk-host/config.example.json \
  --check-config
```

Successful validation writes nothing and exits with status 0. It parses the
configuration, applies profile validation, resolves the selected credential
source and constructs the model client without opening ACP stdio.

An ACP Client launches the same executable with only the explicit config path:

```text
target/debug/echo-agent-sdk-host --config /absolute/path/to/host.json
```

The checked-in [`config.example.json`](../../echo-sdk-host/config.example.json)
targets a local OpenAI-compatible endpoint and is parsed by the Host unit test.
Adjust the endpoint and model name for the provider running on your machine.

## Configuration schema v1

The JSON document is limited to 1 MiB and has these top-level fields:

| Field | Meaning |
|---|---|
| `schema_version` | Required; must be `1`. |
| `default_agent` | Required `FrameworkConfig` containing `model` and `agent`. |
| `api_key_env` | Optional environment variable name read by the Host process. |
| `sdk_profile` | Optional core-profile section (`state_root` + `limits`); Hosts built without the `sdk-core-profile` feature reject it. |

`default_agent.model.provider`, `name`, `base_url` and `api_protocol` are
required. `default_agent.agent.name` and `system_prompt` must be non-empty,
`max_iterations` must be positive, and `enable_tools` must be true. The
standard Host currently rejects memory and human-loop settings because their
persistence and Client callback semantics are not advertised by this profile.

For credentials, set either `default_agent.model.auth_token` or `api_key_env`,
never both. Omitting both is valid for a local provider without authentication.
The Host does not search the current directory, home directory, `.env` files or
EKO product configuration. It does not log or echo the resolved credential or
the complete configuration.

## ACP and MCP boundary

The Host exposes the adapter's stable v1 initialize, `session/new`,
`session/prompt`, `session/update`, `session/cancel` and request-cancellation
surface. Text and ResourceLink Prompt content are supported. Other content and
optional Session/lifecycle methods are rejected or left unadvertised exactly as
documented in [acp-agent-adapter.md](acp-agent-adapter.md).

ACP Clients may declare stdio MCP servers during `session/new`. Each server
needs a unique non-empty name and an absolute UTF-8 command path; arguments and
environment entries are preserved, and each process starts in that ACP
Session's cwd. All servers connect before the Session is returned. HTTP/SSE MCP
declarations and additional working directories are not part of this Host
profile and fail explicitly.

While ACP is active, every stdout line belongs to the official JSON-RPC
transport. Logs and bounded startup failures use stderr. Stdin EOF triggers the
official transport close, cancellation of active Turns and the adapter's
bounded concurrent Agent cleanup.

## Core profile (optional)

With `sdk_profile` configured and the feature enabled
(`cargo build -p echo-sdk-host --features sdk-core-profile`), the same
executable negotiates the [`_echo_agent/*` core profile](sdk-core-profile.md)
over a bounded stdin transport: explicit absolute `state_root`, generation
counters, run journals and the run index all live under that root. Without
the section the Host is standard-only and never advertises the extension.
See [`config.sdk.example.json`](../../echo-sdk-host/config.sdk.example.json)
for a complete tested example.

## Current status

The real subprocess suite launches this source-built binary through the
official ACP Rust Client and covers success, updates, cancellation, clean EOF,
invalid configuration, protocol-only stdout and credential non-disclosure.
This evidence supports **ACP conformant** for the documented standard profile.
The core-profile suite additionally covers valid-hello lifecycle, the
fail-closed matrix, restart recovery, crash interruption and the stdin frame
limiter for the negotiated profile. TypeScript, Python and Java clients do
not exist yet, so **Runnable**, **Parity complete** and **Published** remain
unreached.
