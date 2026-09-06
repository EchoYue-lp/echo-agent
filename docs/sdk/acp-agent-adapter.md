# Stable ACP v1 Agent adapter

The root `echo_agent` crate provides a transport-neutral ACP Agent adapter under
the optional `acp` feature. It is a framework integration point: applications
supply one independent framework Agent per ACP Session and choose an official
ACP transport. The source-built [`echo-agent-sdk-host`](acp-standard-host.md)
provides the product-neutral default factory and official stdio composition.

## Enable the adapter

```toml
[dependencies]
echo-agent = { path = "../echo-agent", default-features = false, features = ["acp"] }
agent-client-protocol = "=2.1.0"
```

`AcpSessionFactory` receives `AcpSessionContext` for every `session/new`. The
context contains the generated Session ID, absolute cwd, additional directories,
MCP server declarations, initialize-time Client capabilities and request
metadata. The factory must apply the Session prerequisites it supports and
return a fresh `Box<dyn Agent>`; it may delegate ordinary Agent construction to
the existing framework `AgentFactory`.

```rust,no_run
use agent_client_protocol::{ConnectTo as _, Stdio};
use echo_agent::acp::{AcpAgentAdapter, AcpSessionContext};
use echo_agent::agent::Agent;

# async fn serve() -> Result<(), Box<dyn std::error::Error>> {
let adapter = AcpAgentAdapter::new(|context: AcpSessionContext| async move {
    let agent: Box<dyn Agent> = build_agent_for_session(context).await?;
    Ok(agent)
});

adapter.connect_to(Stdio::new()).await?;
# Ok(())
# }
# async fn build_agent_for_session(
#     _context: AcpSessionContext,
# ) -> echo_agent::error::Result<Box<dyn Agent>> {
#     Err(echo_agent::error::ReactError::Other("example factory".to_string()))
# }
```

The compiled learning example
[`demo72_acp_agent_adapter.rs`](../../echo-agent-learning/examples/demo72_acp_agent_adapter.rs)
uses a deterministic Agent and needs no model credentials.

## Current stable profile

The adapter accepts the following stable ACP v1 methods and notifications:

| Direction | Method | Behavior |
|---|---|---|
| Client to Agent | `initialize` | Selects protocol v1, records Client capabilities and reports only implemented Agent capabilities. |
| Client to Agent | `session/new` | Validates absolute paths, calls the Session factory and returns a unique ID. |
| Client to Agent | `session/prompt` | Maps Text and typed ResourceLink blocks, then runs the Session Agent in Chat mode through `AgentTurnDriver`. |
| Agent to Client | `session/update` | Projects message/thought chunks and tool call lifecycle from accepted framework events. |
| Client to Agent | `session/cancel` | Cancels the active Prompt for that Session. |
| Client to Agent | `$/cancel_request` | Cancels the matching Prompt request through the same framework token. |

The Prompt handler runs in an official connection task so the dispatch loop can
process cancellation while the Agent is working. A Session rejects a second
concurrent Prompt; separate Sessions may run concurrently. Completed and
cancelled framework receipts become `end_turn` and `cancelled`. Other framework
failures return a bounded standard ACP internal error, and the connection stays
available for later requests.

## Shared connection runtime and extension profiles

`session/prompt` no longer owns a private execution path: the adapter builds
one `echo_agent::acp::AcpConnectionServices` per connection — a single
`SessionRegistry`, a single Run authority (`start`/`get`/`wait`/`cancel`/
`steer` over framework `RunEntry` values), and a ledger-first event path that
commits every accepted `EventEnvelope` to a bounded per-run ledger (optionally
through a durable `EventJournal` hook) before rendering the standard
`session/update` projection and forwarding to extension observers. Exactly
one framework terminal per run is preserved; a journal or projection failure
fails the run instead of reporting success.

Extension profiles plug in through the `AcpConnectionProfile` trait:
`with_profile` wraps the adapter, the profile publishes its capability
advertisement under `agentCapabilities._meta`, decides Extended vs Standard
from the Client hello, contributes per-run observers, annotates standard
responses with extension handles for `_meta` bridging, and registers typed
handlers that the adapter merges via the official
`Builder::with_connection_builder` — the initialize/stdio/reader-writer/
standard-handler/close chain is still built exactly once. The delivered
profile is the [`_echo_agent/*` core profile](sdk-core-profile.md); a
standard-only consumer never depends on it.

`AcpAdapterConfig` bounds Session count, Prompt text, each serialized update,
the update count and cumulative serialized update size for one Turn, and total
shutdown wait. Connection teardown first cancels every active Turn, then waits
and attempts Agent closes concurrently within the configured global timeout.

## Boundaries

- The adapter is transport-neutral and does not install or spawn a Host binary;
  the separate non-published Host crate composes it with official stdio.
- Each Session owns a new Agent. The adapter does not maintain a second
  transcript, task graph, retry policy or terminal state.
- Text and ResourceLink are the current Prompt surface. ResourceLink becomes a
  provider-neutral `LinkedResource` content part with all ACP fields preserved;
  a Session factory must return an Agent that implements structured chat for
  such prompts. Image, audio and embedded resource blocks are rejected and are
  not advertised.
- MCP declarations are passed without loss to the Session factory. The factory
  is responsible for establishing them before returning the Agent.
- Optional stable methods such as `session/load`, `session/resume`,
  `session/close`, list/delete/config/mode and authentication are not advertised
  or handled in this increment.
- The core `_echo_agent/*` families (agent/session/run lifecycle, events,
  ACK/replay, recovery) are delivered behind the optional
  [`sdk-core-profile`](sdk-core-profile.md) composition; without an attached
  profile the adapter never publishes the extension capability. Task graph,
  subagent, extension-bridge, structured-output and feature operations stay
  unadvertised until their plans land.
- ACP conformance and full multilingual SDK parity remain separate gates. The
  adapter has typed in-process coverage and the source-built Host passes its
  supported standard and core profiles through the official Client; no
  language extension path or full facade parity is implied.
