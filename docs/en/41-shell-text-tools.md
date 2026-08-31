# Shell & Text Tools

## ShellTool — Secure Shell Command Execution

**Feature**: `shell` | **Risk Level**: ShellExec (Level 3) | **Permission**: Execute

### Security Architecture (Three-Layer Model)

| Layer | Behavior | Example Commands |
|-------|----------|-----------------|
| ✅ Whitelist | Direct execution | `ls`, `cat`, `head`, `tail`, `git`, `cargo`, `grep`, `echo`, `find`, `wc` |
| ⚠️ Approval Queue | Requires human confirmation | `rm`, `curl`, `npm`, `pip`, `bash`, `python`, `sed`, `awk` |
| 🚫 Blocklist | Always rejected | `dd`, `sudo`, `chmod`, `chown`, `reboot`, `shutdown`, `nmap` |

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `command` | string | ✅ | Shell command to execute |
| `timeout` | integer | ❌ | Timeout in seconds (default 60s, max 300s) |

### Timeout Mechanism

- **Default**: 60 seconds
- **Builder config**: `ShellTool::new().with_timeout(120)` — set default timeout
- **Per-call**: Override via `timeout` parameter, hard-capped at 300 seconds
- **Implementation**: `tokio::time::timeout` async timeout (non-blocking)
- **Timeout message**: `⏱️ Command timeout after N seconds`

### Shell Injection Protection

- Detects metacharacters: `| ; & $ \` > < ( ) \n`
- Uses `shlex::split` for strict argv parsing
- Default strict mode: commands not in whitelist are rejected
- Sandbox mode: commands with metacharacters run via `sh -c`

### Deterministic Background-Cell Watch

When `ShellTool` launches with `background=true`, consumers can retain and
observe the cell without a model-driven polling Agent:

```rust,no_run
use echo_agent::tasks::{
    CommandCellWatchConfig, CommandCellWatcher,
};
use echo_agent::agent::CancellationToken;

# async fn observe(
#   registry: std::sync::Arc<dyn echo_agent::tools::cell::CommandCellRegistry>
# ) -> echo_agent::error::Result<()> {
let watcher = CommandCellWatcher::acquire(
    registry,
    "cell-id",
    CommandCellWatchConfig::default(),
)?;
let terminal = watcher
    .wait_terminal(&CancellationToken::new())
    .await?;
assert!(terminal.snapshot.phase.is_terminal());
# Ok(())
# }
```

The watcher owns a retention lease, reuses the returned byte cursor, and exits
only after the typed terminal and available output are drained. Cancellation
behavior is explicit through `CommandCellWatchCancellation`; it never stops the
underlying command implicitly.

---

## TextSearchTool — Text File Search

**Feature**: `media` | **Risk Level**: ReadOnly | **Permission**: Read

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `file_path` | string | ✅ | Absolute path to text file |
| `pattern` | string | ✅ | Search pattern (supports regex) |
| `context` | integer | ❌ | Context lines before/after matches (default 0) |
| `ignore_case` | boolean | ❌ | Case-insensitive search (default false) |

### Output Format

```json
{
  "file": "/path/to/file.txt",
  "pattern": "error:\\d+",
  "match_count": 3,
  "truncated": false,
  "max_matches": 200,
  "matches": ["  123 | error: 404 not found"]
}
```

---

## TextStatsTool — Text Statistics

**Feature**: `media` | **Risk Level**: ReadOnly | **Permission**: Read

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `file_path` | string | ✅ | Absolute path to text file |

### Output Format

```json
{
  "file": "/path/to/file.txt",
  "lines": 150,
  "chars": 3200,
  "words": 450,
  "chinese_chars": 120,
  "english_words": 330,
  "file_size_kb": 3.2,
  "avg_line_len": 21.3,
  "max_line_len": 120
}
```

---

## TextProcessTool — Text Processing

**Feature**: `media` | **Permission**: Read

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `file_path` | string | ✅ | Absolute path to text file |
| `operation` | string | ✅ | Operation type (see table below) |
| `count` | integer | ❌ | Number of lines for head/tail (default 10) |

### Supported Operations

| Operation | Description |
|-----------|-------------|
| `unique` | Deduplicate (preserves first occurrence order) |
| `sort` | Lexicographic sort |
| `reverse` | Reverse line order |
| `trim` | Remove blank lines |
| `head` | Take first N lines |
| `tail` | Take last N lines |

### Output Format

```json
{
  "file": "/path/to/file.txt",
  "operation": "unique",
  "original_lines": 100,
  "result_lines": 75,
  "preview": ["line1", "line2", "..."],
  "truncated": false
}
```

---

## TextExportTool — Text Export

**Feature**: `media` | **Permission**: Write

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `input_file` | string | ✅ | Input text file path |
| `output_file` | string | ✅ | Output file path |
| `operation` | string | ❌ | Optional processing: `unique`, `sort`, `trim` |
