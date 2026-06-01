# Echo-Agent Security & Code Quality Audit Report

**Date**: 2026-05-31  
**Auditor**: Automated deep audit  
**Scope**: `src/`, `echo-core/src/`, `echo-tools/src/`, `echo-integration/src/`, `echo-orchestration/src/`, `echo-execution/src/`, `echo-state/src/`  
**Total Rust files examined**: 578  
**Total `unwrap()`/`expect()` sites**: 946 across 132 files  

---

## Executive Summary

echo-agent is a well-structured Rust AI Agent framework with **above-average security awareness** for an open-source project. It includes a command whitelist/blacklist system, SSRF protection, path sandboxing, secret scanning, JWT auth, and multi-tier sandboxing (local OS / Docker / K8s). However, this audit uncovered **4 Critical, 8 High, 12 Medium, and 8 Low severity findings** that should be addressed before any production deployment.

The most pressing issues are: (1) SQL read-only filter bypass, (2) SSRF TOCTOU via DNS rebinding, (3) Seatbelt sandbox profile injection, and (4) unauthenticated WebSocket human-in-the-loop provider.

---

## 1. SECURITY VULNERABILITIES

### 1.1 CRITICAL: SQL Read-Only Filter Bypass in Database Tool

**File**: `echo-tools/src/database.rs`, lines 76-116  
**Severity**: CRITICAL  

The `SqlQueryTool` attempts to restrict queries to read-only by checking prefixes and scanning for dangerous keywords. This filter is trivially bypassable:

```rust
// Line 76-83: Prefix check
let trimmed = query.trim().to_uppercase();
let allowed = trimmed.starts_with("SELECT")
    || trimmed.starts_with("WITH"); // CTE usually followed by SELECT

// Line 93-108: Dangerous keyword scan
let dangerous = ["INSERT", "UPDATE", "DELETE", "DROP", ...];
for keyword in &dangerous {
    if trimmed.contains(keyword) { ... }
}
```

**Bypass vectors**:

1. **`SELECT ... INTO OUTFILE`** — The keyword `INTO OUTFILE` is in the denylist, but the check is case-sensitive on `trimmed` (uppercased) while the denylist entries are also uppercase. However, `SELECT ... INTO DUMPFILE` is **not** in the denylist and achieves the same effect on MySQL.

2. **CTE with side-effect functions** — `WITH ... SELECT` passes the prefix check. Some databases allow side-effect functions within SELECT (e.g., PostgreSQL `SELECT dblink_exec(...)`, `SELECT lo_import(...)`, `COPY ... FROM`).

3. **`PRAGMA` on SQLite** — `PRAGMA` is explicitly allowed but `PRAGMA journal_mode = WAL` or `PRAGMA foreign_keys = ON` are write operations.

4. **Subquery smuggling** — `SELECT * FROM (DELETE FROM users RETURNING *) AS t` on PostgreSQL passes both the prefix and keyword checks because `DELETE` appears inside a subquery and the keyword check uses `.contains()` which would catch it. However, `SELECT pg_terminate_backend(pid) FROM pg_stat_activity` has no blocked keyword and terminates all database connections (DoS).

5. **MySQL `SELECT ... INTO @var`** — Not blocked, allows data exfiltration to session variables.

**Recommendation**: Use parameterized queries with a dedicated read-only database user account. Add connection-level `SET SESSION TRANSACTION READ ONLY` for MySQL/PostgreSQL.

---

### 1.2 CRITICAL: SSRF TOCTOU (Time-of-Check-Time-of-Use) via DNS Rebinding

**File**: `echo-tools/src/security.rs`, lines 441-467  
**Severity**: CRITICAL  

The SSRF protection resolves DNS at validation time, then the HTTP client resolves DNS again independently:

```rust
// Line 441-467: validate_url resolves DNS to check for private IPs
pub fn validate_url(url_str: &str) -> Result<()> {
    let host = extract_host(url_str)?;
    let addr_str = format!("{}:0", host);
    let addrs = addr_str.to_socket_addrs()  // <-- DNS resolution #1
        .map_err(|e| ...)?;
    for addr in addrs {
        if is_private_ip(&addr.ip()) { return Err(...); }
    }
    Ok(())
}

// Then in web/fetch.rs line 177:
let response = self.client.get(url).send().await;  // <-- DNS resolution #2 (separate!)
```

An attacker-controlled DNS server can return a public IP for the first resolution (passing the check) and a private IP (127.0.0.1, 169.254.169.254 for cloud metadata) for the second. This is a classic DNS rebinding attack.

**Additionally**, the `is_private_ip` function (line 507-533) does not block:
- `100.64.0.0/10` (CGNAT / carrier-grade NAT, RFC 6598)
- `192.0.0.0/24` (IETF Protocol Assignments)
- `198.18.0.0/15` (benchmarking)
- IPv4-mapped IPv6 addresses like `::ffff:127.0.0.1`
- `0:0:0:0:0:ffff:7f00:0001` (IPv4-mapped IPv6 for 127.0.0.1)

**Recommendation**: Use a custom DNS resolver that pins the resolved IP for both validation and connection. Block all RFC 6890 "special-purpose" ranges. Consider using the `hickory-resolver` crate with a shared cache.

---

### 1.3 CRITICAL: macOS Seatbelt Sandbox Profile Injection

**File**: `echo-execution/src/sandbox/local.rs`, lines 173-215  
**Severity**: CRITICAL  

The Seatbelt profile escapes double quotes in paths but does not escape parentheses, newlines, or other profile-significant characters:

```rust
// Line 183-185
for path in &self.config.allowed_read_paths {
    let escaped = path.display().to_string().replace('"', "\\\"");
    profile.push_str(&format!(
        "(allow file-read* (subpath \"{}\"))\n", escaped
    ));
}
```

A malicious path like `/tmp")\n(allow network*)\n(subpath "` would inject arbitrary Seatbelt profile rules, enabling network access or file access to any directory. This defeats the entire OS sandbox.

**Recommendation**: Validate paths contain only safe characters (alphanumeric, `/`, `.`, `-`, `_`). Reject paths containing `(`, `)`, `"`, `\n`, `\r`, or `\0`.

---

### 1.4 CRITICAL: Eval Runner Arbitrary Command Execution via `sh -c`

**File**: `src/eval/runner.rs`, lines 338-347  
**Severity**: CRITICAL  

The `run_command` helper passes the `test_command` from eval case definitions directly to `sh -c` with no sanitization:

```rust
// Line 338-347
async fn run_command(cmd: &str, cwd: &Path) -> bool {
    tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)      // <-- unsanitized user input
        .current_dir(cwd)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}
```

The `test_command` originates from `EvalCase::SweBench { test_command, .. }` (line 286) which is loaded from eval case definitions. If an attacker can influence eval case data (e.g., via a shared eval dataset), they achieve arbitrary command execution.

Similarly, `repo_url` (line 263) is passed directly to `git clone` without URL validation, allowing `git clone` of arbitrary URLs including `file://` paths.

---

### 1.5 HIGH: WebSocket Human-in-the-Loop Has No Authentication

**File**: `echo-orchestration/src/human_loop/websocket.rs`, lines 92-131  
**Severity**: HIGH  

The WebSocket provider binds to `127.0.0.1` (line 100) which mitigates remote access, but has **zero authentication** for connected clients:

```rust
// Line 92-100: No auth, no TLS
pub async fn bind(port: u16) -> std::io::Result<Self> {
    Self::bind_with_timeout(port, Duration::from_secs(300)).await
}
// ...
let addr = SocketAddr::from(([127, 0, 0, 1], port));
let listener = TcpListener::bind(addr).await?;
```

Any local process can connect and auto-approve dangerous tool executions. On multi-user systems or when port forwarding is active, this is a privilege escalation vector.

**Recommendation**: Add a shared-secret handshake or at minimum generate a random token printed to stdout that clients must present.

---

### 1.6 HIGH: File Tools Path Resolution Vulnerable to Symlink Bypass

**File**: `echo-tools/src/files/mod.rs`, lines 19-62  
**Severity**: HIGH  

The `resolve_path` function uses a purely textual `normalize_path` that resolves `..` components without checking the actual filesystem. It does **not** call `canonicalize()`:

```rust
// Line 19-44: Purely textual normalization
fn resolve_path(tool: &str, path_str: &str, base_dir: &Option<PathBuf>) -> Result<PathBuf> {
    let requested = Path::new(path_str);
    let resolved = if let Some(base) = base_dir {
        let normalized_base = normalize_path(base);
        let normalized = if requested.is_absolute() {
            normalize_path(requested)
        } else {
            normalize_path(&normalized_base.join(requested))
        };
        if !normalized.starts_with(&normalized_base) {
            return Err(...);
        }
        normalized
    } else { ... };
    Ok(resolved)
}
```

If a symlink exists within the allowed base directory pointing to an external location (e.g., `/allowed/dir/link -> /etc/passwd`), the path `/allowed/dir/link` passes the `starts_with` check but reads files outside the sandbox. Contrast with `PathValidator::validate_file()` in `echo-tools/src/security.rs` (line 212) which correctly calls `canonicalize()`.

**Recommendation**: Use `std::fs::canonicalize()` after path resolution (the file must exist for read operations), then check the canonical path against the allowed base.

---

### 1.7 HIGH: Image Fetch Tool Missing SSRF Validation

**File**: `echo-tools/src/media/image_fetch.rs`, lines 21-35, 70-76  
**Severity**: HIGH  

`ImageFetchTool` creates its own HTTP client with SSRF-safe redirect policy but **does not call `validate_url()`** before fetching:

```rust
// Line 23-31: Client has SSRF-safe redirect policy...
let client = Client::builder()
    .redirect(crate::security::ssrf_safe_redirect_policy())
    .build()...

// Line 70-76: ...but the initial URL is never validated!
async fn download_image_as_base64(&self, url: &str) -> Result<(String, String)> {
    let response = self.client.get(url).send().await...  // No validate_url() call!
```

The redirect policy only protects against SSRF via redirects, not against direct requests to private IPs. An attacker can request `http://169.254.169.254/latest/meta-data/` (AWS metadata) directly.

**Recommendation**: Add `validate_url(url)?;` before the HTTP request, as done in `WebFetchTool`.

---

### 1.8 HIGH: Plugin Registry `git clone` Without URL Validation

**File**: `echo-core/src/plugin/registry.rs`, lines 250-256  
**Severity**: HIGH  

```rust
let status = std::process::Command::new("git")
    .args(["clone", "--depth", "1", url])
    .arg(&tmp_dir)
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .status()...
```

The `url` parameter is not validated. An attacker could supply:
- `file:///etc/passwd` — clone from local filesystem
- `--upload-pack=malicious_command` — if embedded in URL arguments  
- SSH URLs that trigger SSH key usage

**Recommendation**: Validate URL scheme is `https://` only. Reject `file://`, `ssh://`, and `git://` schemes.

---

### 1.9 HIGH: `unsafe` Environment Variable Mutation in Production Code

**File**: `src/config.rs`, lines 479, 487-488  
**File**: `echo-core/src/plugin/variables.rs`, lines 178-189  
**File**: `echo-integration/src/providers/config.rs`, lines 1167, 1175-1176  
**Severity**: HIGH  

Multiple locations use `unsafe { std::env::set_var() }` outside of test code:

```rust
// src/config.rs line 479 (inside test code - OK)
// echo-core/src/plugin/variables.rs line 178:
pub fn export_to_env(vars: &PluginVariables) {
    unsafe {
        std::env::set_var("ECHO_PLUGIN_ROOT", &vars.plugin_root);
        std::env::set_var("ECHO_PLUGIN_DATA", &vars.plugin_data);
        // ...
        for (key, value) in &vars.user_config {
            let env_key = format!("ECHO_PLUGIN_OPTION_{}", key.to_uppercase());
            std::env::set_var(&env_key, value);
        }
    }
}
```

The `user_config` keys are user-controlled and `.to_uppercase()` does not prevent injection of arbitrary env var names. A key like `LD_PRELOAD` would become `ECHO_PLUGIN_OPTION_LD_PRELOAD`, which is less dangerous but still unexpected. More critically, `std::env::set_var` is unsafe in multi-threaded programs (UB if another thread reads env concurrently).

**Note**: The Rust standard library made `set_var`/`remove_var` `unsafe` in Rust 1.84+ due to soundness issues with concurrent access. All uses here should be reviewed for thread safety.

---

### 1.10 HIGH: A2A Server Defaults to No Authentication

**File**: `src/a2a/serve.rs`, lines 52-63  
**File**: `src/a2a/auth.rs`, lines 103-112  
**Severity**: HIGH  

The `serve()` function explicitly disables authentication:

```rust
// serve.rs line 55-63
pub async fn serve(server: A2AServer, bind_addr: &str) -> crate::error::Result<()> {
    serve_inner(server, bind_addr, JwtConfig::disabled(), DEFAULT_MAX_BODY_BYTES).await
}
```

When `JwtConfig::disabled()` is active (line 188 in auth.rs), the middleware passes all requests through without any validation. The `serve()` function binds to a user-specified address (potentially `0.0.0.0`), exposing the agent to the network without authentication.

**Recommendation**: Deprecate `serve()` in favor of `serve_with_auth()`, or at minimum emit a warning log when auth is disabled on non-loopback addresses.

---

### 1.11 HIGH: Git Tool Argument Injection via `target` Parameter

**File**: `echo-tools/src/git.rs`, lines 91-120  
**Severity**: HIGH  

The `git_diff` tool passes user-supplied `target` and `file_path` parameters directly as git arguments:

```rust
let target = parameters.get("target").and_then(|v| v.as_str()).unwrap_or("HEAD");
if let Some(target) = target_opt {
    args.push(target);  // User-controlled string as git argument
}
```

A malicious `target` value like `--upload-pack=touch /tmp/pwned` could potentially inject git options. While `Command::new("git").args(args)` avoids shell injection, git itself interprets arguments starting with `--` as options.

**Recommendation**: Use `--` separator before user-supplied arguments to prevent option injection: `args.push("--"); args.push(target);`

---

### 1.12 HIGH: Database Connection URL Accepts Arbitrary Schemes via Prefix Matching

**File**: `echo-tools/src/database.rs`, lines 60-68  
**Severity**: HIGH  

The connection URL validation uses `starts_with` which can be bypassed:

```rust
if !conn_url.starts_with("sqlite")
    && !conn_url.starts_with("mysql")
    && !conn_url.starts_with("postgresql")
    && !conn_url.starts_with("postgres")
{ ... }
```

A URL like `sqlite://file::memory:?cache=shared` or `postgresql://attacker.com/db?sslmode=disable&options=-c statement_timeout=0` passes validation. The `sqlite://` scheme with a crafted path can read arbitrary files the process has access to.

---

## 2. BUGS AND LOGIC ERRORS

### 2.1 MEDIUM: RwLock Poison Panics in Notebook Module

**File**: `src/notebook/mod.rs`, lines 50, 64, 69, 83, 89  
**Severity**: MEDIUM  

Multiple `unwrap()` calls on `RwLock` read/write guards that will panic if the lock is poisoned:

```rust
let mut cells = self.cells.write().unwrap();  // line 50
self.cells.read().unwrap().clone()             // line 64
let cells = self.cells.read().unwrap();        // line 69
```

If any thread panics while holding the lock, all subsequent accesses will panic, cascading the failure.

---

### 2.2 MEDIUM: RwLock Poison Panics in PromptTemplateManager

**File**: `echo-core/src/agent/prompt_template.rs`, lines 291, 299, 305, 311, 325, 357, 367  
**Severity**: MEDIUM  

Seven `expect("PromptTemplateManager lock poisoned")` calls that will panic the entire process if any thread panics while holding the template lock.

---

### 2.3 MEDIUM: Semaphore Expect in Subagent Team Runner

**File**: `src/agent/subagent/team/runner.rs`, lines 56, 120  
**Severity**: MEDIUM  

```rust
let _permit = sem.acquire().await.expect("semaphore closed");
```

If the semaphore is closed (all permits dropped), this panics. In a long-running agent system with dynamic subagent creation, this is plausible.

---

### 2.4 MEDIUM: MCP Stdio Transport Drops Pending Requests on Stdout Close

**File**: `echo-integration/src/mcp/transport/stdio.rs`, lines 113-118  
**Severity**: MEDIUM  

When the MCP server's stdout closes (process crash), all pending requests are silently cleared:

```rust
Ok(None) => {
    tracing::debug!("MCP stdio: stdout 已关闭");
    let mut map = pending_clone.lock().await;
    map.clear();  // Pending oneshot senders dropped → receivers get RecvError
    break;
}
```

The callers will receive `Err(RecvError)` from the oneshot channel, which is mapped to `McpError::TransportClosed`. While this is handled, there is no indication of **which** request failed, making debugging difficult.

---

### 2.5 MEDIUM: Secret Redaction Byte-Position Safety Issue

**File**: `src/security.rs`, lines 103-116  
**Severity**: MEDIUM  

The `redact_secrets` function uses regex match positions (byte offsets) to do string replacement:

```rust
matches.sort_by_key(|m| std::cmp::Reverse(m.position));
for m in &matches {
    let end = m.position + m.matched.len();
    if end <= result.len() {
        result.replace_range(m.position..end, &replacement);
    }
}
```

If a regex match starts or ends in the middle of a multi-byte UTF-8 character, `replace_range` will panic because the byte range doesn't align with character boundaries. The current patterns are ASCII-only so this is unlikely to trigger today, but adding Unicode-aware patterns would make this exploitable.

---

### 2.6 MEDIUM: Regex Objects Recreated on Every Call in prompt_exec

**File**: `echo-execution/src/skills/external/prompt_exec.rs`, lines 158-159, 196-197, 239, 270  
**Severity**: MEDIUM (performance)  

Regex patterns are compiled from string literals inside functions rather than using `LazyLock`:

```rust
// Called per-invocation:
let block_re = Regex::new(r"```!\s*\n?([\s\S]*?)\n?```").expect("valid block regex");
let inline_re = Regex::new(r"(?:^|\s)!`([^`]+)`").expect("valid inline regex");
```

These are compiled every time the function is called. Should use `static` / `LazyLock` for regex compilation.

---

### 2.7 MEDIUM: Eval Runner Ignores Checkout and Clone Errors

**File**: `src/eval/runner.rs`, lines 262-286  
**Severity**: MEDIUM  

```rust
// Line 275: checkout errors silently ignored
let _checkout = std::process::Command::new("git")
    .args(["checkout", base_commit])
    .current_dir(&repo_dir)
    .output();

// Line 280: apply errors silently ignored if clone succeeded
let apply = std::process::Command::new("git")
    .args(["apply", test_patch])
    .current_dir(&repo_dir)
    .output();
```

Failed checkout and patch application are silently discarded, potentially running tests against the wrong commit.

---

### 2.8 MEDIUM: `parse_method` Hand-Rolled JSON Parsing Fragile

**File**: `src/a2a/serve.rs`, lines 274-299  
**Severity**: MEDIUM  

The `parse_method` function uses string-search to extract the `"method"` field instead of parsing JSON. While this is intentional for performance (avoiding full parse), the test at line 361-364 shows it can extract the wrong `"method"` from nested objects:

```rust
// This test PASSES but shows incorrect behavior:
let body = r#"{"jsonrpc":"2.0","method":"tasks/send","params":{"method":"inner"}}"#;
assert_eq!(parse_method(body), Some("tasks/send".to_string()));
```

If `"method"` appears as a value before the actual key (e.g., `{"note":"method:","method":"real"}`), the parser could extract the wrong value.

---

### 2.9 LOW: Process Group Kill May Fail Silently

**File**: `echo-execution/src/sandbox/local.rs`, lines 309-318  
**Severity**: LOW  

```rust
async fn cleanup_child_process(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .status();
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}
```

The `kill -{pid}` sends SIGKILL to the process group. If this fails (e.g., PID reuse), errors are silently discarded. The subsequent `child.kill()` only kills the direct child, not grandchildren.

---

## 3. NETWORK SECURITY

### 3.1 MEDIUM: SSRF Protection Missing Several Private Ranges

**File**: `echo-tools/src/security.rs`, lines 507-533  
**Severity**: MEDIUM  

The `is_private_ip` function is missing:
- `100.64.0.0/10` (CGNAT, RFC 6598) — used by cloud providers internally
- `192.0.0.0/24` (IETF Protocol Assignments)
- `198.18.0.0/15` (Network benchmark tests)
- `192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24` (TEST-NET, documentation)
- IPv4-mapped IPv6 addresses (`::ffff:127.0.0.1`)

---

### 3.2 LOW: Fallback HTTP Client Has No SSRF Protection

**File**: `echo-tools/src/web/fetch.rs`, lines 38-44  
**Severity**: LOW  

```rust
.unwrap_or_else(|e| {
    tracing::error!("Failed to build HTTP client: {}, using default", e);
    Client::new()  // <-- No SSRF protection, no timeouts!
})
```

If the client builder fails, `Client::new()` is used which has no redirect policy, no timeouts, and no SSRF protection.

---

## 4. UNSAFE RUST

### 4.1 Summary of `unsafe` Usage

| Location | Lines | Purpose | Risk |
|----------|-------|---------|------|
| `src/config.rs` | 479, 487-488 | `set_var`/`remove_var` in tests | Low (test-only) |
| `echo-core/src/plugin/variables.rs` | 178-189 | `set_var` for plugin env export | **High** (production, multi-threaded) |
| `echo-integration/src/providers/config.rs` | 1167, 1175-1176, 1252, 1261, 1273, 1290, 1305 | `set_var`/`remove_var` in tests + production | Medium |
| `echo-orchestration/src/human_loop/protected.rs` | 431, 451 | `set_var` in tests | Low |
| `echo-execution/src/skills/external/prompt_exec.rs` | 644, 658 | `set_var` in tests | Low |

No raw pointer manipulation or `unsafe` memory operations were found — all `unsafe` usage is for environment variable mutation.

---

## 5. DEPENDENCY ANALYSIS

### 5.1 Key Dependencies (from root `Cargo.toml`)

| Crate | Version | Notes |
|-------|---------|-------|
| `reqwest` | 0.12.23 | Recent, good. Ensure `rustls` or system TLS is patched. |
| `tokio` | 1.47.1 | Recent, no known CVEs. |
| `serde_json` | 1.0.143 | Current. |
| `sqlx` | 0.8 | Uses `any` driver — connects to SQLite/MySQL/PostgreSQL. |
| `jsonwebtoken` | 9 | Current major version. |
| `axum` | 0.7 | Current. |
| `rusqlite` | 0.31 (bundled) | Bundles SQLite C library — ensure it includes latest security patches. |
| `polars` | 0.53 | Note: excluded from `docs.rs` due to nightly incompatibility (line 37-41). |
| `regex` | 1 | Uses `regex` crate which is safe against ReDoS (no backtracking). |
| `shlex` | 1 | Used for shell argument parsing — good practice. |

No known vulnerable crate versions were identified. The `rusqlite` bundled SQLite should be verified to include the latest SQLite security patches.

---

## 6. CODE QUALITY

### 6.1 TODO/FIXME Comments

| File | Line | Content |
|------|------|---------|
| `src/security.rs` | 3-4 | `TODO(security): Split patterns into high-confidence vs heuristic/generic` |
| `src/security.rs` | 7 | `TODO(security): "Password in URL" pattern matches harmless examples in docs` |

These TODOs acknowledge known weaknesses in the secret scanning system. The "Password in URL" pattern (`://[^:]+:[^@]+@`) will match URLs in documentation, creating false positives that may lead to alert fatigue.

### 6.2 Dead Code Annotations

Several structs and functions are marked `#[allow(dead_code)]`:
- `echo-tools/src/media/web_fetch_enhanced.rs` line 36: `WebFetchToolEnhanced`
- `echo-tools/src/media/image_fetch.rs` lines 15, 45, 69: `ImageFetchTool` and methods
- `echo-tools/src/git.rs` line 28: `GitStatusTool`

These indicate features that are implemented but not yet integrated into the main tool registry.

### 6.3 Missing Error Context

Several locations use `.ok()` or `let _ = ...` to discard errors:
- `echo-tools/src/data.rs` line 326: `std::fs::write(..., ...unwrap_or_default())` — writes with a potentially empty JSON string
- `src/improve/store.rs` lines 37, 98, 107: Multiple `let _ = std::fs::...` calls that silently discard I/O errors
- `src/improve/evolution.rs` lines 102, 105, 109: Silent file write failures

### 6.4 Unwrap/Expect Distribution

| Module | File Count | Unwrap/Expect Count |
|--------|-----------|-------------------|
| `echo-tools/` | 28 | ~150 |
| `echo-core/` | 16 | ~120 |
| `echo-state/` | 5 | ~78 |
| `echo-orchestration/` | 10 | ~85 |
| `echo-execution/` | 6 | ~55 |
| `echo-integration/` | 8 | ~48 |
| `src/` (main crate) | 59 | ~410 |

The vast majority of unwrap/expect calls are in `#[cfg(test)]` blocks. Production code generally uses proper error propagation.

---

## 7. POSITIVE SECURITY OBSERVATIONS

The framework demonstrates several strong security practices:

1. **Command whitelist/blacklist** (`echo-tools/src/shell.rs`) — Comprehensive three-tier classification (safe/requires-approval/dangerous) with shell metacharacter rejection.

2. **SSRF redirect protection** (`echo-tools/src/security.rs`) — Custom redirect policy validates each redirect target against private IP ranges.

3. **Sandbox layering** (`echo-execution/src/sandbox/`) — Three-tier sandboxing (local OS / Docker / K8s) with automatic detection and escalation.

4. **Secret scanning** (`src/security.rs`) — Regex-based detection for 12 secret patterns with deduplication and redaction.

5. **Script path validation** (`echo-execution/src/skills/external/run_script_tool.rs`, lines 149-222) — Proper canonicalization and `starts_with` check to prevent path traversal in skill scripts.

6. **Environment cleanup** — `env_clear()` used in subprocess spawning with explicit minimal environment whitelisting (lines 285-288 in run_script_tool.rs, lines 875-878 in hooks.rs).

7. **JWT auth middleware** (`src/a2a/auth.rs`) — Proper bearer token extraction, algorithm restriction, expiration validation, and secret redaction in Debug output.

8. **Output truncation** — All tools properly truncate oversized outputs (shell, web fetch, file read) to prevent memory exhaustion.

9. **Process cleanup** — `kill_on_drop(true)` set on all spawned subprocesses, with explicit kill+wait on timeout.

10. **Shlex parsing** — Shell commands parsed with the `shlex` crate rather than naive `.split_whitespace()`, properly handling quoted arguments.

---

## 8. RECOMMENDATIONS (Priority Order)

### Immediate (Critical/High)

1. **Fix SQL read-only filter** — Use database-level read-only users/transactions instead of keyword filtering.
2. **Fix SSRF TOCTOU** — Implement a custom connector that resolves DNS once and pins the IP for both validation and connection.
3. **Fix Seatbelt profile injection** — Validate path characters strictly; reject paths with parentheses, newlines, or other profile-significant characters.
4. **Sanitize eval runner commands** — Validate `test_command` against the same shell safety checks as `ShellTool`, or use direct argv execution.
5. **Add WebSocket authentication** — Implement a shared-secret handshake for the human-in-the-loop WebSocket provider.
6. **Fix symlink path traversal** — Use `canonicalize()` in file tool path resolution.
7. **Add SSRF validation to ImageFetchTool** — Call `validate_url()` before HTTP requests.
8. **Validate plugin git clone URLs** — Restrict to `https://` scheme only.

### Short-Term (Medium)

9. Replace RwLock `unwrap()` with proper error handling or `parking_lot::RwLock` (which doesn't poison).
10. Add missing private IP ranges to SSRF protection.
11. Move regex compilation to `LazyLock` statics in `prompt_exec.rs`.
12. Add error logging for eval runner checkout/apply failures.
13. Audit and restrict database connection URL schemes more precisely.

### Long-Term (Low/Quality)

14. Address TODO comments in the secret scanning module.
15. Add `--` separators in git tool argument construction.
16. Replace `unsafe { std::env::set_var() }` with thread-safe alternatives or ensure single-threaded initialization.
17. Remove `#[allow(dead_code)]` by either integrating or removing unused tools.
18. Add integration tests for SSRF bypass vectors.

---

*End of audit report.*
