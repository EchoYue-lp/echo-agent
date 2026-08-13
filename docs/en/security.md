# Security & Permissions

echo-agent provides multi-layer security: tool permissions, sandbox isolation, secret management, SSRF protection, and audit logging.

---

## 1. Tool Permission Model

### Permission Types (ToolPermission)

Each tool declares its required permissions:

| Permission | Description |
|------------|-------------|
| `Read` | Read files/directories |
| `Write` | Write/modify files |
| `Network` | Network access |
| `Execute` | Execute commands/code |
| `Sensitive` | Sensitive operations (secrets, env vars) |

```rust
use echo_agent::tools::permission::ToolPermission;

impl Tool for FileReadTool {
    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }
}
```

### Risk Levels (ToolRiskLevel)

| Level | Description |
|-------|-------------|
| `ReadOnly` | No side effects |
| `Standard` | Limited side effects |
| `Dangerous` | Irreversible side effects |

`ToolRiskClassifier` auto-classifies by tool name:

```rust
use echo_agent::tools::risk::ToolRiskClassifier;

let category = ToolRiskClassifier::classify("shell");  // ShellExec, level 3
let category = ToolRiskClassifier::classify("read_file");  // ReadOnly, level 0
```

### Built-in Tool Permissions

| Tool | Permissions | Risk |
|------|------------|------|
| `read_file` | `Read` | `ReadOnly` |
| `write_file` | `Write` | `Standard` |
| `delete_file` | `Write` | `Dangerous` |
| `edit_file` | `Read, Write` | `Standard` |
| `shell` | `Execute` | `Dangerous` |
| `web_fetch` / `web_search` | `Network` | `Standard` |
| `git_commit` | `Write, Execute` | `Dangerous` |
| `run_skill_script` | `Execute` | `Dangerous` |
| `db_query` | `Network` | `Standard` |

---

## 2. Permission Modes

| Mode | Allows Write | Interactive | Classifier | Use Case |
|------|-------------|-------------|------------|----------|
| `Default` | ❌ | ✅ | ❌ | Normal interaction |
| `Plan` | ❌ | ✅ | ❌ | Read-only planning |
| `AcceptEdits` | ✅ | Partial | ❌ | Trusted file edits |
| `BypassPermissions` | ✅ | ❌ | ❌ | Full trust |
| `Auto` | ❌ | ❌ | ✅ | AI auto-decides |
| `Bubble` | ❌ | ✅ | ❌ | Sub-agent bubbling |
| `DontAsk` | ❌ | ❌ | ❌ | CI/CD unattended |

```rust
let config = AgentConfig::new("qwen3.7-max", "agent", "system prompt")
    .permission_mode(PermissionMode::Plan);
```

---

## 3. Permission Rules System

### Rule Structure

```rust
pub struct PermissionRule {
    pub matcher: RuleMatcher,    // which tools to match
    pub behavior: RuleBehavior,  // allow/deny/ask
    pub source: RuleSource,      // priority source
}
```

### Matchers

```rust
RuleMatcher::Tool { name: "read_file".into() }         // exact
RuleMatcher::Pattern { pattern: "Bash(git:*)".into() }  // wildcard
RuleMatcher::Permission { permission: ToolPermission::Execute } // by permission
RuleMatcher::All                                        // match all
```

### Source Priority (high → low)

| Priority | Source | Description |
|----------|--------|-------------|
| 6 | `Session` | Temporary session rules |
| 5 | `CliArg` | CLI arguments |
| 4 | `Managed` | Admin policies (enterprise) |
| 3 | `UserSettings` | `~/.echo/settings.json` |
| 2 | `ProjectSettings` | `.echo/settings.json` |
| 1 | `LocalSettings` | `.echo/settings.local.json` |
| 0 | `Default` | Default rules |

### RuleRegistry — deny-first evaluation

```rust
use echo_agent::tools::permission::*;

let mut registry = RuleRegistry::new();

// Deny all by default
registry.add_rule(PermissionRule::deny(RuleMatcher::All, "Default deny".into(), RuleSource::Default));

// Allow reads
registry.add_rule(PermissionRule::allow(
    RuleMatcher::Permission { permission: ToolPermission::Read }, RuleSource::UserSettings));

// Session: allow git commands (highest priority)
registry.add_rule(PermissionRule::allow(
    RuleMatcher::Pattern { pattern: "Bash(git:*)".into() }, RuleSource::Session));

let decision = registry.check("read_file", &[ToolPermission::Read]);
```

**Evaluation order:** Deny rules reject immediately → Ask rules by source priority → Allow rules by source priority.

### YAML Configuration

```yaml
permissions:
  mode: "prompt"
  rules:
    - matcher: "tool:shell"
      behavior: "ask"
    - matcher: "*"
      behavior: "allow"
```

---

## 4. Permission Service

Unified permission check entry point, integrating rule registry, session cache, denial tracking, and human approval:

```rust
use echo_agent::human_loop::PermissionService;

// Create from HumanLoopProvider (recommended)
let service = PermissionService::from_provider(provider);

// Or configure via Builder for fine-grained control
let service = PermissionServiceBuilder::new()
    .mode(PermissionMode::Default)
    .rule(PermissionRule::new(
        RuleMatcher::Permission { permission: ToolPermission::Read },
        RuleBehavior::Allow,
        RuleSource::Default,
    ))
    .build();
```

### 8-Stage Check Pipeline

```
check(tool, input) → check_with_permissions(tool, input, permissions):
  1. BypassPermissions → Allow
  2. Plan mode → filter by permissions
  3. Protected paths → .git/.env/.ssh always protected
  4. RuleRegistry → deny-first evaluation (Allow/Deny/Ask)
  5. SessionApprovalCache → cache hit = AutoApprove
  6. DenialTracker → consecutive denials exceed threshold
  7. Mode dispatch: Auto→Classifier / Default→Handler / DontAsk→silent deny
  8. Post-processing: cache write, audit logging
```

### Agent Integration

```rust
let agent = ReactAgentBuilder::new()
    .model("qwen3.6-plus")
    .permission_service(Arc::new(service))
    .build()?;
```

`force_read_before_edit: true` requires reading a file before modifying:

```rust
let config = AgentConfig::new("qwen3.6-plus", "agent", "...")
    .force_read_before_edit(true);
```

---

## 5. Sandbox

| Level | Isolation |
|-------|-----------|
| `Trusted` (0) | None, direct host execution |
| `Standard` (1) | Process-level isolation |
| `Strict` (2) | Container (Docker) |
| `Maximum` (3) | Orchestrated (Kubernetes) |

```rust
let sandbox = DockerSandbox::new()
    .with_image("rust:latest")
    .with_network(false)
    .with_memory_limit("512m")
    .with_timeout_secs(30);

let agent = ReactAgent::builder(config).with_sandbox(sandbox).build()?;
```

---

## 6. Secret Management

Always inject via environment variables, **never hardcode in config files**:

```bash
export OPENAI_API_KEY="sk-..."
export ANTHROPIC_API_KEY="sk-ant-..."
```

### JWT Authentication (Web Server mode)

```bash
export AUTH_ENABLED=true
export JWT_SECRET="your-secret-at-least-32-characters-long"
```

---

## 7. MCP Trust Boundaries

**Local MCP servers** run on the same machine. **Remote MCP servers** require: URL verification, tool whitelisting, network isolation.

```json
{
  "mcpServers": {
    "remote-search": {
      "transport": "sse",
      "url": "https://trusted-server.example.com/sse",
      "headers": { "Authorization": "Bearer ${MCP_TOKEN}" }
    }
  }
}
```

---

## 8. SSRF & Injection Protection

### SSRF Protection

All network tools share unified protection:
- **Private IP blocking**: 127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
- **Localhost blocking**: `localhost`, `*.local`
- **Protocol restriction**: only `http://` and `https://`
- **Safe redirects**: re-validate redirect targets
- **Response limit**: 10MB

Applies to: `web_fetch`, `web_search`, `arxiv_search`, `semantic_scholar_search`, `pdf_fetch`.

### SQL Injection Protection

`db_query` protections: SQL blacklist (`DROP`, `DELETE`, `TRUNCATE`, etc.), table name validation, URL scheme validation.

---

## 9. Audit Logging

All tool calls, permission decisions, and Guard interceptions are logged:

```rust
let logs = state.get_audit_logs().await;
// Each: tool_name, decision, reason, source, timestamp, duration
```

---

## 10. Usage Scenarios

### CI/CD (strict)

```rust
let config = AgentConfig::new("qwen3.7-max", "agent", "...")
    .permission_mode(PermissionMode::DontAsk);

let mut registry = RuleRegistry::new();
registry.add_rule(PermissionRule::allow(
    RuleMatcher::Tool { name: "read_file".into() }, RuleSource::Default));
registry.add_rule(PermissionRule::deny(
    RuleMatcher::All, "CI/CD whitelist mode".into(), RuleSource::Default));
```

### Enterprise (admin policy)

```rust
registry.add_rule(PermissionRule::deny(
    RuleMatcher::Tool { name: "shell".into() },
    "Enterprise policy".into(), RuleSource::Managed));
```

---

## 11. Security Checklist

- [ ] Enable JWT authentication
- [ ] Strong JWT secret (≥32 characters)
- [ ] Server host set to `127.0.0.1` (or TLS reverse proxy)
- [ ] Dangerous tools configured as `ask`
- [ ] Sandbox execution enabled
- [ ] API keys via environment variables only
- [ ] MCP servers trusted sources only
- [ ] Audit logs reviewed regularly

---

## See Also

- [Tools](./02-tools.md)
- [Human-in-the-Loop](./05-human-loop.md)
- [Guard System](./18-guard-system.md)
- [Hooks](./23-hooks.md)
