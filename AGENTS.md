# echo-agent

## Workspace structure

7 crates, strict publish order (each depends on prior):

1. `echo-core` — traits: `Tool`, `Agent`, `LlmClient`, `Guard`, `Error`, `Retry`
2. `echo-macros` — proc macros: `#[tool]`, `#[callback]`, `#[guard]`, `#[handler]`
3. `echo-execution` — sandbox, skills, tool execution
4. `echo-integration` — LLM providers, MCP, IM channels
5. `echo-state` — memory, compression, audit
6. `echo-orchestration` — workflow, human-loop, DAG tasks
7. `echo_agent` (root `src/`) — facade: re-exports everything via `prelude::*`

`echo_agent` is the only crate users depend on. The 6 sub-crates exist for compile-time isolation.

## Commands

```bash
# Full CI (matches .github/workflows/rust-ci.yml)
cargo fmt && cargo clippy --workspace --all-targets && cargo check --workspace --all-targets && cargo test --workspace

# Format check only
cargo fmt -- --check

# Test single crate
cargo test -p echo_core
cargo test -p echo_agent --lib

# Test single file/module (use test name substring)
cargo test -p echo_agent --lib -- react_agent

# Run an example (most need API keys via .env)
ECHO_AGENT_CONFIG=echo-agent.yaml cargo run --example demo01_tools

# Doc build (local, simulating docs.rs)
RUSTDOCFLAGS="--cfg docsrs" cargo +nightly doc --no-deps --no-default-features --features "a2a,mcp,telemetry,plan-execute,human-loop,handoff,topology,tasks,workflow,self-reflection,subagent,web,media,sqlite,channels,git,database,rag,chart,content-guard,project-rules"
```

## Publishing to crates.io

**Order matters.** Sub-crates must be published before dependents:

```bash
cargo publish -p echo_core --registry crates-io
cargo publish -p echo_macros --registry crates-io
cargo publish -p echo_execution --registry crates-io
cargo publish -p echo_integration --registry crates-io
cargo publish -p echo_state --registry crates-io
cargo publish -p echo_orchestration --registry crates-io
cargo publish -p echo_agent --registry crates-io
```

Each must succeed and propagate to the index before the next one. Add `--allow-dirty` only if working tree is unclean.

## Known gotchas

- **`data` feature (polars) is excluded from docs.rs.** polars-ops `build.rs` auto-enables `feature="nightly"` on nightly compilers, then uses `core::unicode::{Case_Ignorable, Cased}` which is removed in nightly 1.97+. Upstream fix exists (https://github.com/pola-rs/polars/commit/67422be6) but not yet published. Do NOT add `data` to `[package.metadata.docs.rs]` features until polars releases a version with this fix.

- **`dotenv` is deprecated; use `dotenvy`.** All crates and examples use `dotenvy::dotenv()`. API is identical, only crate name differs.

- **`echo_agent::skills::external` is a re-export, not a local module.** It comes from `echo_execution::skills::external`. The `src/skills/mod.rs` creates `pub mod external { pub use echo_execution::skills::external::*; }` to make `crate::skills::external::...` paths work. Do not add local files under `src/skills/external/`.

- **`edition = "2024"`** requires `rust-version = "1.85"`. The `rust-toolchain.toml` pins `stable`.

- **`cargo fmt` syntax differs:** `cargo fmt -- --check` (not `cargo fmt --check`).

- **Examples need `ECHO_AGENT_CONFIG`** env var pointing to a YAML config file (usually `echo-agent.yaml`). API keys go in `.env`.

## Feature flags

`default = ["full"]` enables all features. Key non-trivial ones:

| Feature | Brings in | Notes |
|---------|-----------|-------|
| `data` | `polars 0.53` | Excluded from docs.rs (see gotchas) |
| `sqlite` | `rusqlite` (bundled) | Heavy C compile; `echo_state/sqlite` |
| `database` | `sqlx` | runtime-tokio + sqlite + mysql + postgres |
| `telemetry` | `opentelemetry` stack | Many transitive deps |
| `human-loop` | `echo_orchestration/websocket` | `tokio-tungstenite` |

Most features are marker flags that gate `#[cfg(feature = "...")]` modules with no extra deps.

## Code style

- Comments only when explicitly asked. No doc comments unless user requests.
- `#![doc = include_str!("../README.md")]` in root `lib.rs` — README IS the docs.rs landing page.
- Sub-crates use inline `//!` doc, not README includes.
