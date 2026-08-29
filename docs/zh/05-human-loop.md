# 人工介入（Human-in-the-Loop）

## 是什么

Human-in-the-Loop（HIL）在 Agent 自动执行流程中插入人工决策点。在执行高风险操作（删除文件、发送邮件、转账）前，Agent 暂停并请求人工确认后再继续。

echo-agent 支持三种介入场景：

| 场景 | 说明 |
|------|------|
| **审批（Approval）** | 工具执行前弹出 y/n 确认，由用户决定是否允许 |
| **输入（Input）** | Agent 需要额外信息时，向用户请求自由文本输入 |
| **选择（Selection）** | 任务 checkpoint 暂停，用户从预定义选项中选择后继续 |

---

## 架构概览

```
┌─────────────────────────────────────────────────────────────────────┐
│                       工具调用请求                                     │
│                   (Agent 发起 tool_use)                              │
└─────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────┐
│              PermissionService 7 阶段管线                             │
│                                                                     │
│  1. BypassPermissions → Allow（若未被管理员禁用）                      │
│  2. Plan 模式 → 按 permissions 过滤（只读）                           │
│  3. RuleRegistry → deny-first 评估 (Allow/Deny/Ask)                 │
│  3.5. ProtectedPathChecker → .git/.env/.ssh 始终受保护               │
│  4. SessionApprovalCache（带 TTL）→ 缓存命中 = AutoApprove           │
│  5. DenialTracker → 连续/总拒绝数超限回退                             │
│  6. 模式分发：                                                       │
│     - Auto → Classifier (LLM / Rule / Composite)                   │
│     - Default → PermissionRequestHandler                            │
│     - AcceptEdits → 写操作自动允许                                    │
│     - DontAsk → 未匹配规则静默拒绝                                    │
│  7. 后处理：缓存写入、拒绝跟踪、审计记录                                │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 权限模式

| 模式 | 行为 |
|------|------|
| `Default` | 危险操作需要用户确认 |
| `Plan` | 只读模式，禁止 Write/Execute/Sensitive |
| `Auto` | AI Classifier 自动决策 |
| `AcceptEdits` | 文件编辑自动通过，其他需确认 |
| `BypassPermissions` | 跳过所有检查（可被管理员禁用） |
| `DontAsk` | 匹配 allow 规则的直接通过，未匹配的静默拒绝 |
| `Bubble` | 子 Agent 权限上溯到父级处理 |

---

## 规则系统 (RuleRegistry)

### Deny-First 评估

规则按 deny-first 顺序评估——**来自任何来源的 Deny 规则始终优先于所有 Allow 规则**：

```
第 1 轮：Deny 规则（所有来源）
第 2 轮：Ask 规则（按来源优先级）
第 3 轮：Allow 规则（按来源优先级）
```

这确保低优先级的 Deny 不会被高优先级的 Allow 覆盖。

### 规则来源优先级

数值越大优先级越高（同一行为类型内）：

| 来源 | 优先级 |
|------|--------|
| `Default` | 0 (最低) |
| `LocalSettings` | 1 |
| `ProjectSettings` | 2 |
| `UserSettings` | 3 |
| `Managed` | 4 (企业管理员设置，用户不可覆盖) |
| `CliArg` | 5 |
| `Session` | 6 (最高) |

---

## 审批缓存（带 TTL）

会话级审批缓存避免对相同工具 + 参数的重复审批请求。

```rust
use echo_agent::human_loop::SessionApprovalCache;
use std::time::Duration;

// 带 30 分钟 TTL（条目自动过期）
let cache = SessionApprovalCache::with_ttl(Duration::from_secs(30 * 60));

// 无 TTL（永不过期，向后兼容）
let cache = SessionApprovalCache::new();
```

三种缓存范围：
- `Once` — 不缓存
- `Session` — 按 tool_name + args_hash 缓存
- `SessionTool` — 按 tool_name 缓存（所有参数匹配）

---

## 受保护路径检查器

阻止 Agent 操作关键系统路径。所有模式下均生效（包括 Bypass）。

默认受保护模式：`.git`、`.env`、`.env.*`、`.ssh`、`.claude`、`.vscode`、shell 配置文件、SSH 密钥。

```rust
use echo_agent::human_loop::ProtectedPathChecker;

let checker = ProtectedPathChecker::new()
    .with_pattern("secrets/");

// 在工具执行前检查
match checker.check("Write", &serde_json::json!({"path": ".env"})) {
    ProtectedPathResult::Protected { matched_pattern, path } => { /* 拒绝 */ }
    ProtectedPathResult::Safe => { /* 继续 */ }
}
```

---

## 审计追踪

记录每个权限决策用于安全审计和合规。

```rust
use echo_agent::human_loop::{
    PermissionService, InMemoryPermissionAuditSink, LoggingPermissionAuditSink,
    CompositePermissionAuditSink,
};
use std::sync::Arc;

// 内存 sink（环形缓冲区，保留最近 1000 条）
let mem_sink = Arc::new(InMemoryPermissionAuditSink::new(1000));

// 日志 sink（tracing）
let log_sink = Arc::new(LoggingPermissionAuditSink::info());

// 组合：同时写入两者
let composite = Arc::new(CompositePermissionAuditSink::new(vec![
    Box::new(mem_sink.clone()) as _,
    Box::new(log_sink) as _,
]));

let service = PermissionService::new()
    .with_audit_sink(composite);

// 执行后查询审计记录
let recent = mem_sink.recent(10);
for entry in recent {
    println!("{} {} → {} ({}, {}us)",
        entry.tool_name, entry.args_hash, entry.decision, entry.reason, entry.duration_us);
}
```

### 审计条目字段

| 字段 | 说明 |
|------|------|
| `tool_name` | 被检查的工具 |
| `args_hash` | 工具输入的 DefaultHasher 哈希（8 位十六进制） |
| `decision` | "allow" / "deny" / "require_approval" / "ask" |
| `reason` | 管线阶段："bypass" / "plan_mode" / "rule_match" / "protected_path" / "cache_hit" / "denial_tracker" / "classifier" / "handler" / "dont_ask" / "no_handler" |
| `source` | 做出决策的阶段 |
| `duration_us` | 决策耗时（微秒） |

---

## AI 分类器

### 增强 ClassifierContext

分类器接收丰富上下文以做出更好的决策：

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

### 内置分类器

```rust
use echo_agent::human_loop::{RuleClassifier, LlmClassifier, CompositeClassifier};

// 基于规则（无需 LLM）
let rule_clf = RuleClassifier::new()
    .with_allow_patterns(vec!["Read".to_string(), "Glob".to_string()])
    .with_deny_patterns(vec!["Bash(rm:*)".to_string()]);

// 基于 LLM
let llm_clf = LlmClassifier::new(client, "qwen3-max".to_string());

// 组合：先尝试规则，再回退到 LLM
let composite = CompositeClassifier::new()
    .with_classifiers(vec![
        Arc::new(rule_clf),
        Arc::new(llm_clf),
    ]);
```

### 拒绝跟踪器

防止 Auto 模式下无限拒绝循环：

```rust
// 默认：连续 3 次或总共 20 次拒绝后回退
let tracker = DenialTracker::new();

tracker.record_denial();
if tracker.should_fallback() {
    // 升级为人工审批
}
```

---

## PermissionService

统一的权限检查入口，支持构建器模式：

```rust
use echo_agent::human_loop::{PermissionService, PermissionServiceBuilder};
use echo_agent::human_loop::permission::{PermissionMode, PermissionRequestHandler};

let service = PermissionServiceBuilder::new()
    .mode(PermissionMode::Auto)
    .classifier(Arc::new(composite_classifier))
    .request_handler(Arc::new(handler))
    .audit_sink(Arc::new(audit_sink))
    .build();

// 权限检查
let decision = service.check("Bash", &serde_json::json!({"command": "ls"})).await?;
```

---

## Provider 实现

### ConsoleHumanLoopProvider（终端，默认）

```
🔔 工具 [delete_file] 需要人工审批
   参数: {"path": "/important/data.csv"}
   是否批准？(y/n): _
```

### WebhookHumanLoopProvider（HTTP 回调）

```rust
let provider = WebhookHumanLoopProvider::new("https://your-approval-service/approve")
    .with_timeout(Duration::from_secs(30));
agent.set_human_loop_provider(Arc::new(provider));
```

### WebSocketHumanLoopProvider（WebSocket 推送）

```rust
let provider = WebSocketHumanLoopProvider::bind(9000).await?;
agent.set_human_loop_provider(Arc::new(provider));
```

### 自定义 Provider

```rust
use echo_agent::prelude::*;
use async_trait::async_trait;

struct SlackApprovalProvider;

#[async_trait]
impl HumanLoopProvider for SlackApprovalProvider {
    async fn request(&self, req: HumanLoopRequest) -> echo_agent::error::Result<HumanLoopResponse> {
        match req.kind {
            HumanLoopKind::Approval => {
                let approved = send_slack_and_wait(&req.prompt).await;
                if approved {
                    Ok(HumanLoopResponse::Approved)
                } else {
                    Ok(HumanLoopResponse::Rejected { reason: Some("Rejected".to_string()) })
                }
            }
            HumanLoopKind::Input => {
                let text = ask_slack_for_input(&req.prompt).await;
                Ok(HumanLoopResponse::Text(text))
            }
            HumanLoopKind::Selection => {
                let options = req.options.unwrap_or_default();
                let choice = ask_slack_for_choice(&req.prompt, &options).await;
                Ok(HumanLoopResponse::Selection {
                    selection: choice,
                    instructions: None,
                })
            }
        }
    }
}
```

---

## 执行流程

```
Agent 准备执行工具 "delete_file"
    │
    ├─ 1. BypassPermissions? → Allow（若未被禁用）
    │
    ├─ 2. Plan 模式? → 拒绝写入/执行操作
    │
    ├─ 3. RuleRegistry（deny-first）
    │       ├─ Deny 规则匹配 → Deny
    │       ├─ Ask 规则匹配 → Ask
    │       └─ Allow 规则匹配 → Allow
    │
    ├─ 3.5. 受保护路径 (.git/.env/.ssh)? → Deny
    │
    ├─ 4. 审批缓存命中（带 TTL）? → Allow
    │
    ├─ 5. DenialTracker 超限? → RequireApproval
    │
    ├─ 6. 模式分发：
    │       ├─ Auto → Classifier → Allow/Deny
    │       ├─ Default → Handler → Allow/Deny/Ask
    │       ├─ AcceptEdits → 写操作自动允许
    │       └─ DontAsk → 静默拒绝
    │
    └─ 7. 后处理：缓存写入、拒绝跟踪、审计记录
```

---

## 与业界框架对比

| 特性 | LangGraph | AutoGen | Claude Code | echo-agent |
|------|-----------|---------|-------------|------------|
| interrupt_before/after | ✅ | ❌ | ❌ | ✅ |
| Checkpoint 持久化 | ✅ | ❌ | ❌ | ✅ |
| 权限模式 | ❌ | human_input_mode | PermissionMode | ✅ |
| AI 分类器 | ❌ | ❌ | YoloClassifier | ✅ |
| Deny-first 规则评估 | ❌ | ❌ | ✅ | ✅ |
| 审批缓存（带 TTL） | ❌ | ❌ | ✅ (1h/5min) | ✅ (可配置) |
| 受保护路径 | ❌ | ❌ | ✅ | ✅ |
| 审计追踪 | ❌ | ❌ | ✅ | ✅ |
| 拒绝跟踪 + 回退 | ❌ | ❌ | ✅ | ✅ |
| Hooks 拦截 | ❌ | ❌ | ✅ | ✅ |
| Managed 规则（企业） | ❌ | ❌ | ✅ | ✅ |

---

## 示例文件

- `echo-agent-learning/examples/demo03_approval.rs` — 双场景示例：既覆盖 LLM 主动调用 `human_in_loop` 请求额外输入，也覆盖敏感工具触发权限确认
- `echo-agent-learning/examples/demo05_compressor.rs` — `add_need_appeal_tool()` 真实路径示例：通过需要审批的工具验证 `PermissionService` / 审批事件链路
- `echo-agent-learning/examples/demo20_audit.rs` — 审计日志 + 权限模型示例

## `add_need_appeal_tool()` 行为说明

`ReactAgent::add_need_appeal_tool()` 是一个同步注册 API，适合在 Agent 初始化阶段直接调用。

当 Agent 已配置 `PermissionService` 时，它不会在当前线程里同步 `block_on` 写规则，而是：

1. 先注册工具本身；
2. 把“该工具需要人工审批”的 `PermissionRule` 暂存到内部缓冲区；
3. 在后续第一次异步权限检查（如 `tool_needs_approval` / `check_tool_approval`）前，再安全地刷入 `PermissionService`。

这样做是为了避免在运行中的 Tokio runtime 内部发生嵌套阻塞。

如果 Agent 没有配置 `PermissionService`，则工具注册后无需审批检查，直接无条件执行。
