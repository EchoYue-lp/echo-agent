# Code Search

## Overview

`CodeSearchTool` is echo-agent's project-wide code search tool. Its primary backend is ripgrep (`rg`), leveraging `rg --json` for structured output parsing. When `rg` is not on PATH, it gracefully falls back to a built-in symbol search engine powered by language-aware regex patterns.

Tool name: `code_search`
Risk level: `ReadOnly`
Required permission: `Read`

---

## Problem It Solves

Agents working in large codebases need to quickly locate function definitions, type declarations, usage references, and more. Having the LLM read files one by one is prohibitively slow. `CodeSearchTool` provides:

- **Millisecond-scale search** — ripgrep is an industrial-grade Rust search tool that handles tens of thousands of files in milliseconds
- **Structured output** — parses JSON output to extract file paths, line numbers, matched text, and context
- **Zero-dependency fallback** — even without ripgrep installed, the built-in engine can search for function, class, and struct definitions

---

## Architecture

```
CodeSearchTool                        ← implements Tool trait
    │
    ├─ try_ripgrep_search()           ← primary: invoke `rg --json` and parse output
    │   ├─ --glob / --type            ← file filtering
    │   ├─ -i / -F / -w              ← match modes
    │   ├─ -C / -A / -B              ← context lines
    │   ├─ -m                         ← max matches per file
    │   └─ parse_rg_json()            ← JSON parser (match/context events)
    │
    └─ search_symbols()               ← fallback: built-in symbol search
        ├─ Rust (fn, struct, enum, trait, type)
        ├─ Python (def, class)
        ├─ JS/TS (function, class, interface, type, const)
        ├─ Go (func, struct, interface)
        ├─ Java (method, class, interface)
        └─ C/C++ (function, struct/class)

Output protection:
    └─ MAX_OUTPUT_BYTES = 50_000      ← 50KB cap to prevent token overflow
```

---

## Feature Gate

`CodeSearchTool` is gated behind the `files` feature:

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["files"] }
```

Registration is handled automatically by `echo_agent::tools::register_all_tools`:

```rust
#[cfg(feature = "files")]
tool_manager.register(Box::new(CodeSearchTool::new()));
```

---

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | Yes | Search pattern (regex by default, literal if `fixed_strings=true`) |
| `path` | string | No | Directory to search in (default: `.`) |
| `glob` | string | No | File name glob filter (e.g. `*.rs`, `*.py`) |
| `file_type` | string | No | File type recognized by ripgrep (e.g. `rust`, `python`, `js`) |
| `case_insensitive` | boolean | No | Case-insensitive matching (`-i`), default `false` |
| `fixed_strings` | boolean | No | Treat pattern as literal string, not regex (`-F`), default `false` |
| `word_regexp` | boolean | No | Only match whole words (`-w`), default `false` |
| `context` | integer | No | Context lines before and after each match (`-C N`) |
| `max_count` | integer | No | Maximum matches per file (`-m N`) |
| `max_results` | integer | No | Maximum total results to return (default: 50) |
| `symbol` | string | No | (Legacy) Symbol name or pattern — used as fallback `query` |
| `symbol_type` | string | No | (Legacy) Symbol type filter: `function`/`class`/`struct`/`enum`/`trait`/`interface`/`type`/`method`/`any` (default: `any`) |

---

## Backend Behavior

### Primary Backend: ripgrep

When `rg` is found on PATH, the tool invokes `rg --json <flags> <query> <path>` and parses JSON events line by line:

- `match` events — extract `path`, `line_number`, `lines.text`
- `context` events — extract context lines, attached to the preceding match result
- `begin`/`end`/`summary` events — ignored

Exit code handling:
- Exit code `0` — matches found
- Exit code `1` — no matches (not an error; returns "No matches found" normally)
- Exit code `2` — real error (e.g. invalid regex), returns `ToolError`

### Fallback Backend: Built-in Symbol Search

When `rg` is not on PATH (`io::ErrorKind::NotFound`), the engine switches automatically:

- Recursively walks directories, skipping hidden dirs and common build artifacts (`.git`, `target`, `node_modules`, `__pycache__`)
- Detects language by file extension and applies language-specific regex to match symbol definitions
- Supports wildcard patterns (`*` converted to regex `.*`)
- Results include file name, line number, symbol type, and context line

### 50KB Output Protection

Regardless of backend, output exceeding `MAX_OUTPUT_BYTES` (50,000 bytes) is automatically truncated with a notice:

```
... [output truncated to prevent overflow]
```

This prevents large search results from exhausting the LLM's context window.

---

## Quick Start

### Basic Usage

```rust
use echo_agent::prelude::*;
use echo_agent::tools::files::code_search::CodeSearchTool;

let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("You are a code assistant.")
    .enable_tools()
    .build()?;

agent.add_tool(Box::new(CodeSearchTool::new()));

let answer = agent
    .execute("Find all implementations of the Tool trait in the project")
    .await?;
```

### Restrict Search Directory

```rust
use echo_agent::tools::files::code_search::CodeSearchTool;

// Constrain all searches to a specific directory
agent.add_tool(Box::new(CodeSearchTool::with_base_dir("/workspace/my-project")));
```

When `with_base_dir` is set, all relative paths resolve under that directory, and path sandboxing is enforced (prevents `../` traversal and symlink bypass).

---

## Agent Invocation Example

The agent might call `code_search` during reasoning like this:

```json
{
  "query": "fn execute",
  "path": "src",
  "glob": "*.rs",
  "context": 2,
  "max_results": 20
}
```

Example output:

```
Found 5 match(es) for 'fn execute' in src:

src/tools/code_search.rs:134:     fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
  132-     }
  133-
  134:     fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
  135-         Box::pin(async move {
  136-             let query = parameters
src/tools/grep.rs:91:     fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
  89-     }
  90-
  91:     fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
  92-         Box::pin(async move {
  93-             let pattern_str = parameters
```

---

## Comparison with GrepTool

The project also provides `GrepTool` (tool name `grep`). They serve different roles:

| Aspect | `code_search` (CodeSearchTool) | `grep` (GrepTool) |
|--------|-------------------------------|-------------------|
| **Backend** | ripgrep (`rg --json`), fallback to built-in engine | Pure Rust `regex` crate, reads files sequentially |
| **Performance** | Very fast (ripgrep parallelism + smart ignores) | Slower (serial walk + per-file reads) |
| **Scope** | Project-wide search with `--type` and `--glob` | Single directory/file content search, `glob` only |
| **Context** | `-C` / `-A` / `-B` (native ripgrep support) | `-C` (manual line-before/after implementation) |
| **Output protection** | 50KB byte cap | `max_results` line cap (default 100) |
| **Symbol search** | Language-aware symbol definitions in fallback mode | Not supported |
| **Best for** | Large project global search, code navigation | Narrow precise matches, regex in known directories |

**When to use which**:
- Searching code patterns across the project → `code_search`
- Finding specific text in known files → `grep`
- Searching for function/class/struct definitions → `code_search` (leverages fallback symbol search)

---

## Languages Supported in Fallback Mode

When `rg` is unavailable, the built-in symbol search recognizes definitions in these languages:

| Language | Extensions | Recognized Symbol Types |
|----------|-----------|------------------------|
| Rust | `.rs` | `function`, `struct`, `enum`, `trait`, `type` |
| Python | `.py` | `function`, `class` |
| JavaScript/TypeScript | `.js`, `.jsx`, `.ts`, `.tsx` | `function`, `class`, `interface`, `type` |
| Go | `.go` | `function`, `struct`, `interface` |
| Java | `.java` | `method`, `class`, `interface` |
| C/C++ | `.c`, `.h`, `.cpp`, `.hpp` | `function`, `class` (struct/class) |

---

## Security Considerations

- **Read-only** — risk level is `ReadOnly`; no files are modified
- **Path sandboxing** — when `base_dir` is set, `resolve_path` prevents directory traversal (including symlink detection)
- **Output truncation** — 50KB cap prevents search results from exhausting the LLM context window
- **Permission declaration** — requests only `Read` permission

---

## Full Example

```rust
use echo_agent::prelude::*;
use echo_agent::tools::files::code_search::CodeSearchTool;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .system_prompt("You are a senior Rust developer assisting with code exploration.")
        .enable_tools()
        .build()?;

    // Register CodeSearchTool, scoped to project root
    agent.add_tool(Box::new(CodeSearchTool::with_base_dir("./my-project")));

    // The agent will automatically invoke code_search to locate relevant code
    let answer = agent
        .execute("Find all async functions that return Result in the src/ directory, \
                   and explain the error handling patterns used.")
        .await?;

    println!("{}", answer);
    Ok(())
}
```
