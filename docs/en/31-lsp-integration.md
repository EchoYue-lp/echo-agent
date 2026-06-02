# LSP Integration (Language Server Protocol)

## What It Is

LSP (Language Server Protocol) is an open standard by Microsoft that decouples code intelligence (diagnostics, completion, go-to-definition, find-references, hover) from editors. A language server implements these capabilities for a specific programming language; any LSP client can connect and use them.

echo-agent implements a full LSP client, exposing language server capabilities as first-class **agent tools**. An AI agent can run `lsp_diagnostics`, `lsp_goto_definition`, `lsp_find_references`, and `lsp_hover` against any project with a configured language server — turning static code analysis into something the agent can reason over during a task.

> **Feature gate**: LSP integration requires the `lsp` Cargo feature. See [Enabling the Feature](#enabling-the-feature).

---

## Problem It Solves

### Code Awareness Without IDE

When an agent edits code, it typically works blind: it can read text, but it cannot see compiler errors, locate definitions, or find callers. LSP bridges that gap:

```
Traditional agent:
  read file → edit file → hope it compiles

LSP-enabled agent:
  read file → edit file → lsp_diagnostics → fix errors → lsp_find_references → verify callers
```

### Unified Multi-Language Support

LSP is language-agnostic. The same agent code works with `rust-analyzer`, `pyright`, `typescript-language-server`, `clangd`, `gopls`, or any other LSP server — just swap the configuration file.

```
                    ┌─────────────────────────────────────┐
                    │         echo-agent (LspManager)     │
                    │                                     │
                    │  ┌──────────┐ ┌──────────┐ ┌──────┐ │
                    │  │ rust-an. │ │ pyright  │ │ tss. │ │
                    │  └────┬─────┘ └────┬─────┘ └──┬───┘ │
                    └───────┼────────────┼──────────┼─────┘
                            │            │          │
                     .rs files     .py files    .ts/.js files
```

---

## Architecture

echo-agent splits LSP support across three crates, each with a clear role:

```
echo-core/src/lsp/              Traits + types (LspClient, Position, Diagnostic, ...)
       │
       ▼
echo-integration/src/lsp/       Process management (LspManager, StdioLspClient, LspConfig)
       │
       ▼
src/tools/lsp.rs                Tool wrappers (LspDiagnosticsTool, LspGotoDefinitionTool, ...)
```

The core `LspClient` trait defines the contract; `StdioLspClient` implements it by spawning a server process and speaking JSON-RPC over stdin/stdout; the tool wrappers adapt each capability into a standard `Tool` the agent can call.

### Component Responsibilities

| Component | Crate | Role |
|-----------|-------|------|
| `LspClient` trait | `echo-core` | Object-safe interface for talking to a language server |
| `StdioLspClient` | `echo-integration` | Spawns a server process, handles JSON-RPC over stdio |
| `LspConfig` | `echo-integration` | Parses `.lsp.yaml` configuration files |
| `LspManager` | `echo-integration` | Manages multiple servers, routes requests by file extension |
| LSP tools | `echo-agent` | Adapt capabilities to `Tool` trait for agent use |

---

## Enabling the Feature

LSP support is gated behind the `lsp` Cargo feature. Add it to your `Cargo.toml`:

```toml
[dependencies]
echo-agent = { version = "0.1", features = ["lsp"] }
```

Or use `full` to enable everything:

```toml
echo-agent = { version = "0.1", features = ["full"] }
```

If the feature is not enabled, the `lsp_*` tools and the `register_lsp_tools` function are not compiled into the binary.

---

## Available LSP Tools

All LSP tools are `ReadOnly` — they never modify the codebase. They require an `LspManager` initialized at agent startup.

### `lsp_diagnostics`

Get compiler errors, warnings, and hints for a file.

**Parameters**:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `file_path` | string | yes | Absolute path to the file |

**Example**:
```json
{
  "file_path": "/home/user/project/src/main.rs"
}
```

**Output**:
```
Diagnostics for /home/user/project/src/main.rs (2 issues):

  [ERROR] line 12:5 — mismatched types: expected `i32`, found `String`
  [WARNING] line 45:1 — unused variable: `x`
```

Diagnostics come from the server's `textDocument/publishDiagnostics` notification stream and are cached locally by `StdioLspClient`. Severity levels map to `ERROR`, `WARNING`, `INFO`, `HINT`.

### `lsp_goto_definition`

Find where a symbol (function, class, variable, trait) is defined.

**Parameters**:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `file_path` | string | yes | Absolute path to the file |
| `line` | integer | yes | Line number (1-indexed) |
| `column` | integer | yes | Column number (1-indexed) |

**Example**:
```json
{
  "file_path": "/home/user/project/src/main.rs",
  "line": 12,
  "column": 5
}
```

**Output**:
```
Definitions found:

  /home/user/project/src/lib.rs:42:1
```

Note: positions are 1-indexed in the tool API (matching how humans read code) but converted to 0-indexed internally for the LSP protocol.

### `lsp_find_references`

Find all locations where a symbol is used across the codebase.

**Parameters**: same as `lsp_goto_definition` (file_path, line, column).

**Example output**:
```
References found (5 total):

  /home/user/project/src/main.rs:12:5
  /home/user/project/src/main.rs:45:10
  /home/user/project/src/lib.rs:88:3
  /home/user/project/tests/integration.rs:120:15
  /home/user/project/tests/integration.rs:205:8
```

By default, the declaration itself is included in the results (`includeDeclaration: true`).

### `lsp_hover`

Get type information, documentation, and signatures for a symbol.

**Parameters**: same as `lsp_goto_definition` (file_path, line, column).

**Example output**:
```rust
pub fn read_to_string<P: AsRef<Path>>(path: P) -> Result<String>

Reads the entire contents of a file into a string.

# Errors
This function will return an error if the file does not exist
or if the contents are not valid UTF-8.
```

Hover content is returned as Markdown when the server supports it (most modern servers do).

### `lsp_status`

Show the status of all configured and running language servers.

**Parameters**: none.

**Example output**:
```
Language Servers (3 configured):

  rust [running] (pid: 12345)
  python [running] (pid: 12346)
  typescript [stopped]
    Error: Failed to initialize typescript server: spawn ENOENT
```

Server states: `running` (initialized and accepting requests), `starting` (process spawned, handshake in progress), `stopped` (not running).

---

## Configuring LSP Servers

### `.lsp.yaml` Format

Language servers are configured in a `.lsp.yaml` file placed at the project root:

```yaml
languages:
  rust:
    language: rust
    command: rust-analyzer
    args: []
    extensions: [".rs"]

  python:
    language: python
    command: pyright-langserver
    args: ["--stdio"]
    extensions: [".py", ".pyi"]

  typescript:
    language: typescript
    command: typescript-language-server
    args: ["--stdio"]
    extensions: [".ts", ".tsx", ".js", ".jsx"]

  go:
    language: go
    command: gopls
    args: ["serve"]
    extensions: [".go"]

  cpp:
    language: cpp
    command: clangd
    args: ["--background-index"]
    extensions: [".cpp", ".cc", ".h", ".hpp"]
```

### Configuration Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `language` | string | yes | Language identifier |
| `command` | string | yes | Command to start the language server |
| `args` | string[] | no | Command-line arguments (default: `[]`) |
| `extensions` | string[] | no | File extensions this server handles |
| `env` | map | no | Environment variables passed to the server process |
| `initialization_options` | object | no | Custom initialization options (JSON) |
| `max_restarts` | integer | no | Max restart attempts before giving up (default: `3`) |

### How Routing Works

`LspManager` builds an **extension → language** map from the configuration. When an LSP tool is called with a file path, the manager:

1. Extracts the file extension (e.g., `.rs`)
2. Looks up the language name (e.g., `rust`)
3. Finds the running `StdioLspClient` for that language
4. Routes the request to it

If no server is running for the file's language, the tool returns an error suggesting the user start one.

### Loading Configuration in Code

```rust
use echo_integration::lsp::{LspConfig, LspManager};
use std::path::Path;

let mut manager = LspManager::new();

// Load from .lsp.yaml
let config = LspConfig::from_file(Path::new(".lsp.yaml"))?;
manager.load_config(&config);

// Set the workspace root (used as rootUri during initialize)
manager.set_project_root(Path::new("/home/user/project"));

// Start a server for a specific language
manager.start_server("rust").await?;
```

---

## LspManager Lifecycle

`LspManager` owns all language server processes and routes requests to the right one:

```
┌──────────────────────────────────────────────────────────────┐
│                       LspManager                             │
│                                                              │
│  configs: { "rust" → LspServerConfig, ... }                  │
│  extension_map: { ".rs" → "rust", ".py" → "python", ... }    │
│  clients: { "rust" → Arc<RwLock<StdioLspClient>>, ... }      │
│                                                              │
│  Operations:                                                 │
│    load_config()       — parse .lsp.yaml                     │
│    set_project_root()  — set workspace root URI              │
│    start_server(lang)  — spawn + initialize one server       │
│    stop_server(lang)   — graceful shutdown of one server     │
│    restart_server(lang) — stop + start                       │
│    get_client_for_file(path) — route by extension            │
│    status_all()        — status of every configured server   │
│    shutdown_all()      — stop all running servers            │
└──────────────────────────────────────────────────────────────┘
```

### Server Lifecycle

```
new() ─> initialize(root_uri) ─> [requests / notifications]* ─> shutdown()
         │                          │
         ├─ spawn child process     ├─ textDocument/definition
         ├─ send initialize         ├─ textDocument/references
         └─ send initialized        ├─ textDocument/hover
                                    ├─ textDocument/publishDiagnostics (notification)
                                    └─ ...
```

---

## Example: Setting Up rust-analyzer for a Rust Project

### 1. Install rust-analyzer

```bash
rustup component add rust-analyzer
# or: cargo install rust-analyzer
```

Verify it is on `PATH`:
```bash
rust-analyzer --version
```

### 2. Create `.lsp.yaml` at the project root

```yaml
languages:
  rust:
    language: rust
    command: rust-analyzer
    args: []
    extensions: [".rs"]
```

### 3. Wire LSP into an agent

```rust
use echo_agent::prelude::*;
use echo_integration::lsp::{LspConfig, LspManager};
use std::sync::Arc;
use tokio::sync::RwLock;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    // Build the agent with tools enabled
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("rust-reviewer")
        .system_prompt("You are a Rust code reviewer. Use LSP tools to inspect the project.")
        .enable_tools()
        .build()?;

    // Set up LspManager
    let mut manager = LspManager::new();
    let config = LspConfig::from_file(std::path::Path::new(".lsp.yaml")).unwrap();
    manager.load_config(&config);
    manager.set_project_root(std::path::Path::new("/home/user/project"));

    // Start rust-analyzer
    manager.start_server("rust").await.expect("Failed to start rust-analyzer");

    // Wrap and register LSP tools
    let lsp_manager = Arc::new(RwLock::new(manager));
    register_lsp_tools(&mut agent, lsp_manager.clone());

    // Run a code review task
    let answer = agent.execute(
        "Review src/main.rs: check diagnostics, then find all references to the `process` function."
    ).await?;
    println!("{}", answer);

    // Clean shutdown
    lsp_manager.write().await.shutdown_all().await;
    Ok(())
}
```

### 4. What the agent sees

During the task, the agent can call:
- `lsp_diagnostics` on `/home/user/project/src/main.rs` to get compiler errors
- `lsp_goto_definition` on a symbol at `line 42, column 10` to jump to its definition
- `lsp_find_references` on `process` at `line 15, column 4` to find every call site
- `lsp_hover` to get full signatures and doc comments
- `lsp_status` to verify `rust-analyzer` is running

---

## Example: Using LSP Tools for Code Navigation

A more elaborate scenario where the agent uses LSP to understand an unfamiliar codebase:

```rust
// System prompt instructs the agent to use LSP for navigation
let system_prompt = r#"
You are a code navigation assistant. Help users understand unfamiliar codebases.

When asked about a symbol:
1. Use lsp_goto_definition to find where it is defined.
2. Use lsp_find_references to find where it is used.
3. Use lsp_hover to get its documentation and type signature.
4. Use lsp_diagnostics to flag any issues in files you inspect.

Always report file paths and line numbers so the user can jump there.
"#;

let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .name("code-navigator")
    .system_prompt(system_prompt)
    .enable_tools()
    .build()?;

// ... LspManager setup as above ...

let answer = agent.execute(
    "Trace the data flow of the `UserRecord` struct in /home/user/project/src/models/user.rs. \
     Where is it defined? Where is it used? Are there any compiler errors in those files?"
).await?;
```

The agent will iteratively call LSP tools to build a complete picture — definition site, every usage, type signatures, and any errors — then synthesize a human-readable answer.

---

## StdioLspClient Internals

`StdioLspClient` is the workhorse that speaks to a single language server:

```
┌───────────────────┐                    ┌───────────────────┐
│   echo-agent      │                    │  Language Server  │
│   StdioLspClient  │                    │  (child process)  │
│                   │   stdin (JSON-RPC) │                   │
│                   │   ───────────────> │                   │
│                   │                    │                   │
│                   │   stdout (JSON-RPC)│                   │
│                   │   <─────────────── │                   │
│                   │                    │                   │
│                   │   stderr (logs)    │                   │
│                   │   <─────────────── │                   │
└───────────────────┘                    └───────────────────┘
```

Key implementation details:

- **Framed messages**: LSP uses `Content-Length: N\r\n\r\n<body>` framing, not newline-delimited JSON. `jsonrpc::encode_message` handles encoding; the read loop parses headers.
- **Async I/O**: stdin writes are dispatched by a dedicated writer task; stdout reads run in a reader task. This prevents slow responses from blocking outgoing requests.
- **Pending request map**: Each outgoing request gets a unique `id` and a `oneshot` channel. When the response arrives, the reader task looks up the `id` and signals the channel.
- **Diagnostics cache**: The server pushes diagnostics via `textDocument/publishDiagnostics` notifications. The reader task caches them per file URI; `diagnostics()` returns the cached snapshot.
- **Graceful shutdown**: `shutdown()` sends the LSP `shutdown` request, then the `exit` notification, then kills the process if it has not already exited.

---

## Error Handling

LSP operations can fail in several ways:

| Error | Description | Handling |
|-------|-------------|----------|
| `LspError::NotInitialized` | Operation called before `initialize()` | Ensure `start_server()` has completed |
| `LspError::NotRunning(lang)` | Server for `lang` is not running | Call `start_server(lang)` |
| `LspError::SpawnError(msg)` | Failed to spawn server process | Check that `command` is on `PATH` and executable |
| `LspError::ServerError(msg)` | Server returned a JSON-RPC error | Check server logs (stderr) for details |
| `LspError::CommunicationError(msg)` | stdin/stdout channel closed | Server may have crashed — check `status_all()` |
| `LspError::Timeout` | Request did not respond in time | Server may be busy indexing; retry |
| `LspError::InvalidUri(msg)` | Malformed file URI | Use absolute paths |

Tools never propagate errors to the agent runtime — they return `ToolResult::error(...)` with a human-readable message so the agent can decide what to do next.

---

## Supported Language Servers

Any LSP-compliant server that supports stdio transport works. Common choices:

| Language | Server | Install |
|----------|--------|---------|
| Rust | `rust-analyzer` | `rustup component add rust-analyzer` |
| Python | `pyright-langserver` | `npm i -g pyright` |
| TypeScript/JS | `typescript-language-server` | `npm i -g typescript-language-server` |
| Go | `gopls` | `go install golang.org/x/tools/gopls@latest` |
| C/C++ | `clangd` | Bundled with LLVM / `apt install clangd` |
| Java | `jdtls` | [Eclipse JDT LS](https://github.com/eclipse-jdtls/eclipse.jdt.ls) |
| Ruby | `solargraph` | `gem install solargraph` |
| Lua | `lua-language-server` | [lua-language-server](https://github.com/LuaLS/lua-language-server) |

---

## See Also

- `examples/` — demo for LSP integration
- [02-tools.md](02-tools.md) — Tool system overview
- [08-mcp.md](08-mcp.md) — MCP integration (external tool services)
- [28-config-reference.md](28-config-reference.md) — Full configuration reference
