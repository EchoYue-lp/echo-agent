# Security Guide

Echo Agent provides multiple layers of security mechanisms to protect your system and data. This guide covers the tool security model, sandbox configuration, secret management, and MCP trust boundaries.

---

## 1. Tool Security Model

### Permission Declarations

Each built-in tool declares its required permissions and risk level:

| Tool | Permissions | Risk Level | Notes |
|------|-------------|------------|-------|
| `read_file` | `Read` | `ReadOnly` | Read-only file access |
| `write_file` | `Write` | `Standard` | Write to file |
| `delete_file` | `Write` | `Dangerous` | Delete file |
| `edit_file` | `Read, Write` | `Standard` | Edit file (auto .bak backup) |
| `shell` | `Execute` | `Dangerous` | Execute shell commands (whitelist-restricted) |
| `web_fetch` | `Network` | `Standard` | HTTP requests (SSRF protected) |
| `web_search` | `Network` | `Standard` | Web search (DuckDuckGo/Brave/Tavily) |
| `git_commit` | `Write, Execute` | `Dangerous` | Create Git commits |
| `run_skill_script` | `Execute` | `Dangerous` | Execute skill scripts |
| `arxiv_search` | `Network` | `Standard` | ArXiv API (HTTPS, SSRF redirect policy) |
| `semantic_scholar_search` | `Network` | `Standard` | Semantic Scholar API (HTTPS, SSRF redirect) |
| `pdf_fetch` | `Network` | `Standard` | PDF download (SSRF protected, %PDF validation) |
| `bibtex_generate` | `Write` | `ReadOnly` | BibTeX text generation |
| `rag_index` | `Read, Write` | `Standard` | Document chunking + vector index |
| `rag_search` | `Read` | `ReadOnly` | Semantic search over indexed docs |
| `excel_read` / `excel_info` | `Read` | `ReadOnly` | Read Excel files |
| `excel_write` | `Write` | `Standard` | Write Excel files |
| `data_read` / `data_filter` / ... | `Read` | `ReadOnly` | Data analysis tools |
| `data_transform` | `Read, Write` | `Standard` | Data transformation |
| `generate_chart` | `Read, Write` | `Standard` | Chart image generation |
| `db_query` | `Network` | `Standard` | SQL database queries (blacklist protected) |

### Permission Types

- **`Read`** — Read files/directories
- **`Write`** — Write/modify files
- **`Network`** — Network access
- **`Execute`** — Execute commands/code
- **`Sensitive`** — Sensitive operations (keys, env vars, etc.)

### Approval Chain

Tool execution goes through the following approval chain:

```
Tool.metadata (permissions, risk_level)
    → PolicyEngine (RuleRegistry + PermissionPolicy)
    → PermissionRequest (risk_level, context)
    → HumanLoopProvider (approve/deny/ask)
    → AuditLogger (record decision)
    → SandboxExecutor (isolated execution)
```

### Read-Before-Edit Enforcement

Set `force_read_before_edit: true` in AgentConfig to require the model to read a file before modifying or deleting it:

```rust
let config = AgentConfig::new("qwen-plus", "agent", "system prompt")
    .force_read_before_edit(true);
```

---

## 2. Sandbox Configuration

### Security Levels

Framework-level `SecurityLevel` (4 isolation tiers):

| Level | Value | Description |
|-------|------|-------------|
| `Trusted` | 0 | No isolation, direct host execution |
| `Standard` | 1 | Process-level isolation |
| `Strict` | 2 | Container isolation (Docker) |
| `Maximum` | 3 | Orchestrated isolation (Kubernetes) |

### Docker Sandbox

```rust
let sandbox = DockerSandbox::new()
    .with_image("rust:latest")
    .with_network(false)
    .with_memory_limit("512m")
    .with_cpu_limit(1.0)
    .with_timeout_secs(30);
```

---

## 3. Secret Management

- Always use environment variables for API keys
- Never store secrets in `echo-agent.yaml`
- Add `.env` to `.gitignore`
- Enable JWT authentication for web server mode
- Default host should be `127.0.0.1` for local-only access

---

## 4. MCP Trust Boundaries

- Local MCP servers run on the same machine — can access local filesystem
- Remote MCP servers connect via HTTP/SSE — verify trustworthiness of the URL
- Apply strict permission rules for MCP-provided tools
- All MCP tool calls are logged in the audit trail

---

## 5. SSRF Protection

All network-capable tools share a unified SSRF (Server-Side Request Forgery) protection layer:

- **Private IP blocking**: 127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
- **Localhost blocking**: `localhost`, `*.local` domains
- **Protocol restriction**: Only `http://` and `https://` allowed (blocks `file://`, `gopher://`, etc.)
- **Safe redirect policy**: Redirects are re-validated against the same rules
- **Shared HTTP client**: `OnceLock<reqwest::Client>` with configurable timeout (30-60s)

This protection applies to: `web_fetch`, `web_search`, `arxiv_search`, `semantic_scholar_search`, `pdf_fetch`.

### SQL Injection Protection

Database tools (`db_query`) include additional protections:

- **SQL blacklist**: Blocks `DROP`, `DELETE`, `TRUNCATE`, `ALTER`, `CREATE`, `GRANT`, `EXECUTE`, `INTO OUTFILE`, `LOAD_FILE`
- **Table name validation**: Only alphanumeric, `_`, and `.` characters allowed
- **URL scheme validation**: Only `sqlite`, `mysql`, `postgresql`, `postgres` schemes accepted
- **MySQL escaping**: Single quotes escaped as `''` (not `\'`)

---

## 6. Security Checklist

- [ ] JWT authentication enabled (`AUTH_ENABLED=true`)
- [ ] Strong JWT secret (≥32 characters)
- [ ] Server host set to `127.0.0.1` (or TLS reverse proxy configured)
- [ ] Dangerous tools (shell, delete_file, git_commit) set to `ask` permission
- [ ] Sandbox execution enabled (Docker/K8s)
- [ ] API keys injected via environment variables only
- [ ] MCP servers only connect to trusted sources
- [ ] Audit logs reviewed regularly
