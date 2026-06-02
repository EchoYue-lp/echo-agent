# Human-in-the-Loop

## What It Is

Human-in-the-Loop (HIL) inserts human decision points into the Agent's automatic execution flow. Before performing a high-risk operation (deleting files, sending emails, making payments), the Agent pauses and requests human confirmation before proceeding.

echo-agent supports two intervention scenarios:

| Scenario | Description |
|----------|-------------|
| **Approval** | Show a y/n prompt before a tool executes; the user decides whether to allow it |
| **Input** | When the Agent needs additional information, request free-text input from the user |

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                     Tool Call Request                                │
│                  (Agent initiates tool_use)                          │
└─────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────┐
│              PermissionService 7-Stage Pipeline                     │
│                                                                     │
│  1. BypassPermissions → Allow (if not disabled by admin)            │
│  2. Plan mode → filter by permissions (read-only)                   │
│  3. RuleRegistry → deny-first evaluation (Allow/Deny/Ask)           │
│  3.5. ProtectedPathChecker → .git/.env/.ssh always protected        │
│  4. SessionApprovalCache (with TTL) → cache hit = AutoApprove       │
│  5. DenialTracker → consecutive/total denial fallback               │
│  6. Mode dispatch:                                                  │
│     - Auto → Classifier (LLM / Rule / Composite)                   │
│     - Default → PermissionRequestHandler                            │
│     - AcceptEdits → auto-allow writes                               │
│     - DontAsk → silent deny unmatched                               │
│  7. Post-processing: cache write, denial tracking, audit recording  │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Permission Modes

| Mode | Behavior |
|------|----------|
| `Default` | Dangerous operations require user confirmation |
| `Plan` | Read-only mode, Write/Execute/Sensitive denied |
| `Auto` | AI Classifier makes decisions automatically |
| `AcceptEdits` | File edits auto-approved, others need confirmation |
| `BypassPermissions` | Skip all checks (can be disabled by admin) |
| `DontAsk` | Matched allow rules pass, unmatched silently denied |
| `Bubble` | Sub-agent permissions escalate to parent |

---

## Rule System (RuleRegistry)

### Deny-First Evaluation

Rules are evaluated in deny-first order — **any Deny rule from any source always wins over all Allow rules**:

```
Pass 1: Deny rules (all sources)
Pass 2: Ask rules (by source priority)
Pass 3: Allow rules (by source priority)
```

This ensures a low-priority Deny can never be overridden by a high-priority Allow.

### Rule Source Priority

Higher value = higher priority within the same behavior type:

| Source | Priority |
|--------|----------|
| `Default` | 0 (lowest) |
| `LocalSettings` | 1 |
| `ProjectSettings` | 2 |
| `UserSettings` | 3 |
| `Managed` | 4 (enterprise admin, cannot be overridden by users) |
| `CliArg` | 5 |
| `Session` | 6 (highest) |

---

## Approval Cache (with TTL)

Session-level approval cache avoids repeated approval requests for the same tool + arguments.

```rust
use echo_agent::human_loop::SessionApprovalCache;
use std::time::Duration;

// With 30-minute TTL (entries auto-expire)
let cache = SessionApprovalCache::with_ttl(Duration::from_secs(30 * 60));

// Without TTL (never expires, backward compatible)
let cache = SessionApprovalCache::new();
```

Three cache scopes:
- `Once` — no caching
- `Session` — cache by tool_name + args_hash
- `SessionAllTools` — cache by tool_name (all args match)

---

## Protected Path Checker

Prevents Agent from operating on critical system paths. Enforced in all modes (including Bypass).

Default protected patterns: `.git`, `.env`, `.env.*`, `.ssh`, `.claude`, `.vscode`, shell configs, SSH keys.

```rust
use echo_agent::human_loop::ProtectedPathChecker;

let checker = ProtectedPathChecker::new()
    .with_pattern("secrets/");

// Check before tool execution
match checker.check("Write", &serde_json::json!({"path": ".env"})) {
    ProtectedPathResult::Protected { matched_pattern, path } => { /* deny */ }
    ProtectedPathResult::Safe => { /* proceed */ }
}
```

---

## Audit Trail

Record every permission decision for security auditing and compliance.

```rust
use echo_agent::human_loop::{
    PermissionService, InMemoryPermissionAuditSink, LoggingPermissionAuditSink,
    CompositePermissionAuditSink,
};
use std::sync::Arc;

// In-memory sink (ring buffer, last 1000 entries)
let mem_sink = Arc::new(InMemoryPermissionAuditSink::new(1000));

// Logging sink (tracing)
let log_sink = Arc::new(LoggingPermissionAuditSink::info());

// Composite: write to both
let composite = Arc::new(CompositePermissionAuditSink::new(vec![
    Box::new(mem_sink.clone()) as _,
    Box::new(log_sink) as _,
]));

let service = PermissionService::new()
    .with_audit_sink(composite);

// After execution, query audit records
let recent = mem_sink.recent(10);
for entry in recent {
    println!("{} {} → {} ({}, {}us)",
        entry.tool_name, entry.args_hash, entry.decision, entry.reason, entry.duration_us);
}
```

### Audit Entry Fields

| Field | Description |
|-------|-------------|
| `tool_name` | Tool that was checked |
| `args_hash` | DefaultHasher hex hash (8 chars) of tool input |
| `decision` | "allow" / "deny" / "require_approval" / "ask" |
| `reason` | Pipeline stage: "bypass" / "plan_mode" / "rule_match" / "protected_path" / "cache_hit" / "denial_tracker" / "classifier" / "handler" / "dont_ask" / "no_handler" |
| `source` | Which stage made the decision |
| `duration_us` | Decision time in microseconds |

---

## AI Classifier

### Enhanced ClassifierContext

Classifiers receive rich context for better decisions:

```rust
use echo_agent::human_loop::{ClassifierContext, RiskContext};

let context = ClassifierContext::new("agent".to_string(), "session-123".to_string())
    .with_workspace_path("/project/myapp".to_string())
    .with_project_type("rust".to_string())
    .with_recent_files(vec!["src/main.rs".to_string(), "Cargo.toml".to_string()])
    .with_risk_context(RiskContext {
        has_sensitive_files: true,
        is_destructive: false,
        directory_depth: 3,
        repetition_count: 0,
    });
```

### Built-in Classifiers

```rust
use echo_agent::human_loop::{RuleClassifier, LlmClassifier, CompositeClassifier};

// Rule-based (no LLM needed)
let rule_clf = RuleClassifier::new()
    .with_allow_patterns(vec!["Read".to_string(), "Glob".to_string()])
    .with_deny_patterns(vec!["Bash(rm:*)".to_string()]);

// LLM-based
let llm_clf = LlmClassifier::new(client, "qwen3-max".to_string());

// Composite: try rule-based first, fall back to LLM
let composite = CompositeClassifier::new()
    .with_classifiers(vec![
        Arc::new(rule_clf),
        Arc::new(llm_clf),
    ]);
```

### Denial Tracker

Prevents infinite denial loops in Auto mode:

```rust
// Default: fallback after 3 consecutive or 20 total denials
let tracker = DenialTracker::new();

tracker.record_denial();
if tracker.should_fallback() {
    // Escalate to human approval
}
```

---

## PermissionService

Unified permission check entry point with builder pattern:

```rust
use echo_agent::human_loop::{PermissionService, PermissionServiceBuilder};
use echo_agent::human_loop::permission::{PermissionMode, PermissionRequestHandler};

let service = PermissionServiceBuilder::new()
    .mode(PermissionMode::Auto)
    .classifier(Arc::new(composite_classifier))
    .request_handler(Arc::new(handler))
    .audit_sink(Arc::new(audit_sink))
    .build();

// Check permission
let decision = service.check("Bash", &serde_json::json!({"command": "ls"})).await?;
```

---

## Provider Implementations

### ConsoleHumanLoopProvider (terminal, default)

```
🔔 Tool [delete_file] requires human approval
   Args: {"path": "/important/data.csv"}
   Approve? (y/n): _
```

### WebhookHumanLoopProvider (HTTP callback)

```rust
let provider = WebhookHumanLoopProvider::new("https://your-approval-service/approve")
    .with_timeout(Duration::from_secs(30));
agent.set_human_loop_provider(Arc::new(provider));
```

### WebSocketHumanLoopProvider (WebSocket push)

```rust
let provider = WebSocketHumanLoopProvider::bind(9000).await?;
agent.set_human_loop_provider(Arc::new(provider));
```

### Custom Provider

```rust
use echo_agent::prelude::*;
use async_trait::async_trait;

struct SlackApprovalProvider;

#[async_trait]
impl HumanLoopProvider for SlackApprovalProvider {
    async fn request(&self, req: HumanLoopRequest) -> echo_agent::error::Result<HumanLoopResponse> {
        let approved = send_slack_and_wait(&req.prompt).await;
        if approved {
            Ok(HumanLoopResponse::Approved)
        } else {
            Ok(HumanLoopResponse::Rejected { reason: Some("Rejected".to_string()) })
        }
    }
}
```

---

## Execution Flow

```
Agent about to execute tool "delete_file"
    │
    ├─ 1. BypassPermissions? → Allow (if not disabled)
    │
    ├─ 2. Plan mode? → Deny writes/executes
    │
    ├─ 3. RuleRegistry (deny-first)
    │       ├─ Deny rule matches → Deny
    │       ├─ Ask rule matches → Ask
    │       └─ Allow rule matches → Allow
    │
    ├─ 3.5. Protected path (.git/.env/.ssh)? → Deny
    │
    ├─ 4. Approval cache hit (with TTL)? → Allow
    │
    ├─ 5. DenialTracker threshold? → RequireApproval
    │
    ├─ 6. Mode dispatch:
    │       ├─ Auto → Classifier → Allow/Deny
    │       ├─ Default → Handler → Allow/Deny/Ask
    │       ├─ AcceptEdits → Auto-allow writes
    │       └─ DontAsk → Silent deny
    │
    └─ 7. Post-process: cache write, denial tracking, audit
```

---

## Comparison with Industry Frameworks

| Feature | LangGraph | AutoGen | Claude Code | echo-agent |
|---------|-----------|---------|-------------|------------|
| interrupt_before/after | ✅ | ❌ | ❌ | ✅ |
| Checkpoint persistence | ✅ | ❌ | ❌ | ✅ |
| Permission modes | ❌ | human_input_mode | PermissionMode | ✅ |
| AI classifier | ❌ | ❌ | YoloClassifier | ✅ |
| Deny-first rule evaluation | ❌ | ❌ | ✅ | ✅ |
| Approval cache with TTL | ❌ | ❌ | ✅ (1h/5min) | ✅ (configurable) |
| Protected paths | ❌ | ❌ | ✅ | ✅ |
| Audit trail | ❌ | ❌ | ✅ | ✅ |
| Denial tracking + fallback | ❌ | ❌ | ✅ | ✅ |
| Hooks interception | ❌ | ❌ | ✅ | ✅ |
| Managed rules (enterprise) | ❌ | ❌ | ✅ | ✅ |

---

## Example Files

- `examples/demo03_approval.rs` — dual-path example: covers both LLM-triggered `human_in_loop` input requests and permission confirmation for a sensitive tool
- `examples/demo05_compressor.rs` — real `add_need_appeal_tool()` path: validates the approval-tool flow through `PermissionService` and approval events
- `examples/demo20_audit.rs` — Audit logging + permission example

## `add_need_appeal_tool()` Semantics

`ReactAgent::add_need_appeal_tool()` is intentionally a synchronous registration API so it can be
used during agent setup.

When the agent has a `PermissionService`, it does not call `block_on()` to mutate rules inline.
Instead it:

1. registers the tool immediately;
2. buffers the corresponding approval `PermissionRule` internally;
3. flushes the buffered rule into `PermissionService` right before the next async permission check
   (for example in `tool_needs_approval` or `check_tool_approval`).

This design avoids nested blocking inside an active Tokio runtime.

If no `PermissionService` is configured, the tool is registered without approval
checks and will execute unconditionally.
