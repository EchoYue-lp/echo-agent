# 安全与权限

echo-agent 提供多层安全机制：工具权限控制、沙箱隔离、密钥管理、SSRF 防护和审计日志。

---

## 1. 工具权限模型

### 权限类型（ToolPermission）

每个工具声明所需权限：

| 权限 | 说明 |
|------|------|
| `Read` | 读取文件/目录 |
| `Write` | 写入/修改文件 |
| `Network` | 网络访问 |
| `Execute` | 执行命令/代码 |
| `Sensitive` | 敏感操作（密钥、环境变量） |

```rust
use echo_core::tools::permission::ToolPermission;

impl Tool for FileReadTool {
    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Read]
    }
}

impl Tool for ShellTool {
    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Execute, ToolPermission::Sensitive]
    }
}
```

### 风险级别（ToolRiskLevel）

| 级别 | 说明 |
|------|------|
| `ReadOnly` | 只读操作，无副作用 |
| `Standard` | 标准操作，有限副作用 |
| `Dangerous` | 危险操作，不可逆副作用 |

工具通过 `risk_level()` 声明风险级别。`ToolRiskClassifier` 可自动按工具名分类：

```rust
use echo_execution::risk::ToolRiskClassifier;

let category = ToolRiskClassifier::classify("shell");  // ShellExec, level 3
let category = ToolRiskClassifier::classify("read_file");  // ReadOnly, level 0

// 生成安全提示
let notice = ToolRiskClassifier::safety_notice("shell", &params);
// "Running: rm -rf / — risk: arbitrary command execution"
```

### 内置工具权限一览

| 工具 | 权限 | 风险 |
|------|------|------|
| `read_file` | `Read` | `ReadOnly` |
| `write_file` | `Write` | `Standard` |
| `delete_file` | `Write` | `Dangerous` |
| `edit_file` | `Read, Write` | `Standard` |
| `shell` | `Execute` | `Dangerous` |
| `web_fetch` / `web_search` | `Network` | `Standard` |
| `git_commit` | `Write, Execute` | `Dangerous` |
| `run_skill_script` | `Execute` | `Dangerous` |
| `arxiv_search` / `semantic_scholar_search` | `Network` | `Standard` |
| `db_query` | `Network` | `Standard` |

---

## 2. 权限模式（PermissionMode）

控制权限检查的行为：

| 模式 | 允许写入 | 需要交互 | 使用分类器 | 适用场景 |
|------|---------|---------|-----------|---------|
| `Default` | ❌ | ✅ | ❌ | 常规交互 |
| `Plan` | ❌ | ✅ | ❌ | 规划/只读阶段 |
| `AcceptEdits` | ✅ | 部分 | ❌ | 信任文件编辑 |
| `BypassPermissions` | ✅ | ❌ | ❌ | 完全信任 |
| `Auto` | ❌ | ❌ | ✅ | AI 自动决策 |
| `Bubble` | ❌ | ✅ | ❌ | 子代理冒泡 |
| `DontAsk` | ❌ | ❌ | ❌ | CI/CD 无人值守 |

```rust
let config = AgentConfig::new("qwen3.7-max", "agent", "system prompt")
    .permission_mode(PermissionMode::Plan);       // 只读
    // .permission_mode(PermissionMode::DontAsk);  // CI/CD
    // .permission_mode(PermissionMode::AcceptEdits); // 自动接受编辑
```

---

## 3. 权限规则系统

### 规则结构

```rust
pub struct PermissionRule {
    pub matcher: RuleMatcher,    // 匹配哪些工具
    pub behavior: RuleBehavior,  // 允许/拒绝/询问
    pub source: RuleSource,      // 来源优先级
}
```

### 匹配器（RuleMatcher）

```rust
RuleMatcher::Tool { name: "read_file".into() }         // 精确匹配
RuleMatcher::Pattern { pattern: "Bash(git:*)".into() }  // 通配符匹配
RuleMatcher::Permission { permission: ToolPermission::Execute } // 按权限匹配
RuleMatcher::All                                        // 匹配所有
```

### 行为（RuleBehavior）

```rust
RuleBehavior::Allow
RuleBehavior::Deny { reason: "安全策略禁止".into() }
RuleBehavior::Ask { suggestions: vec!["确认执行".into()] }
```

### 来源优先级（RuleSource，高→低）

| 优先级 | 来源 | 说明 |
|--------|------|------|
| 6 | `Session` | 会话临时规则 |
| 5 | `CliArg` | CLI 参数 |
| 4 | `Managed` | 管理员策略（企业部署） |
| 3 | `UserSettings` | 用户设置 `~/.echo/settings.json` |
| 2 | `ProjectSettings` | 项目设置 `.echo/settings.json` |
| 1 | `LocalSettings` | 本地设置 `.echo/settings.local.json` |
| 0 | `Default` | 默认规则 |

### RuleRegistry — deny-first 评估

```rust
use echo_core::tools::permission::*;

let mut registry = RuleRegistry::new();

// 默认拒绝所有
registry.add_rule(PermissionRule::deny(RuleMatcher::All, "默认拒绝".into(), RuleSource::Default));

// 允许读取
registry.add_rule(PermissionRule::allow(
    RuleMatcher::Permission { permission: ToolPermission::Read }, RuleSource::UserSettings,
));

// 会话临时允许 git 命令（最高优先级）
registry.add_rule(PermissionRule::allow(
    RuleMatcher::Pattern { pattern: "Bash(git:*)".into() }, RuleSource::Session,
));

// 检查权限
let decision = registry.check("read_file", &[ToolPermission::Read]);
```

**评估顺序：** Deny 规则立即拒绝 → Ask 规则按来源优先级选择 → Allow 规则按来源优先级选择。

### YAML 配置

```yaml
# echo-agent.yaml
permissions:
  mode: "prompt"            # default | prompt | auto_allow | auto_deny
  rules:
    - matcher: "tool:shell"
      behavior: "ask"
    - matcher: "perm:execute"
      behavior: "ask"
    - matcher: "*"
      behavior: "allow"
```

---

## 4. 权限服务（PermissionService）

统一权限检查入口，整合规则注册表、会话缓存、拒绝跟踪和人类审批流程：

```rust
use echo_agent::human_loop::PermissionService;

// 从 HumanLoopProvider 创建（推荐）
let service = PermissionService::from_provider(provider);

// 或通过 Builder 细粒度配置
let service = PermissionServiceBuilder::new()
    .mode(PermissionMode::Default)
    .rule(PermissionRule::new(
        RuleMatcher::Permission { permission: ToolPermission::Read },
        RuleBehavior::Allow,
        RuleSource::Default,
    ))
    .build();
```

### 8 阶段检查管线

```
check(tool, input) → check_with_permissions(tool, input, permissions):
  1. BypassPermissions → Allow
  2. Plan 模式 → 按 permissions 过滤
  3. 受保护路径 → .git/.env/.ssh 始终受保护
  4. RuleRegistry → deny-first 评估 (Allow/Deny/Ask)
  5. SessionApprovalCache → 缓存命中 = AutoApprove
  6. DenialTracker → 连续拒绝超限升级
  7. 模式分发: Auto→Classifier / Default→Handler / DontAsk→静默拒绝
  8. 后处理: 缓存写入、审计记录
```

### 与 Agent 集成

```rust
let agent = ReactAgentBuilder::new()
    .model("qwen3.6-plus")
    .permission_service(Arc::new(service))
    .build()?;
```

`force_read_before_edit: true` 要求模型在修改文件前必须先读取：

```rust
let config = AgentConfig::new("qwen3.6-plus", "agent", "system prompt")
    .force_read_before_edit(true);
```

---

## 5. 沙箱配置

### 安全级别（SecurityLevel）

| 级别 | 隔离方式 |
|------|---------|
| `Trusted` (0) | 无隔离，直接宿主执行 |
| `Standard` (1) | 进程级隔离 |
| `Strict` (2) | 容器隔离（Docker） |
| `Maximum` (3) | 编排隔离（Kubernetes） |

### Docker 沙箱

```rust
let sandbox = DockerSandbox::new()
    .with_image("rust:latest")
    .with_network(false)
    .with_memory_limit("512m")
    .with_cpu_limit(1.0)
    .with_timeout_secs(30);

let agent = ReactAgent::builder(config)
    .with_sandbox(sandbox)
    .build()?;
```

---

## 6. 密钥管理

通过环境变量注入，**永远不要硬编码在配置文件中**：

```bash
export OPENAI_API_KEY="sk-..."
export DASHSCOPE_API_KEY="sk-..."
export DEEPSEEK_API_KEY="sk-..."
export ANTHROPIC_API_KEY="sk-ant-..."
export BRAVE_SEARCH_API_KEY="BSA..."
```

### JWT 认证（Web Server 模式）

```bash
export AUTH_ENABLED=true
export JWT_SECRET="your-secret-at-least-32-characters-long"
```

---

## 7. MCP 信任边界

**本地 MCP Server：** 运行在同机器上，可访问本地文件和网络。

```json
{
  "mcpServers": {
    "filesystem": {
      "transport": "stdio",
      "command": "npx",
      "args": ["-y", "@anthropic/mcp-filesystem", "/allowed/path"]
    }
  }
}
```

**远程 MCP Server：** 需要额外安全考量 — 验证 URL、限制工具白名单、网络隔离。

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

## 8. SSRF 与注入防护

### SSRF 防护

所有网络工具共享统一防护层：

- **私有 IP 阻止**：127.0.0.0/8、10.0.0.0/8、172.16.0.0/12、192.168.0.0/16
- **Localhost 阻止**：`localhost`、`*.local`
- **协议限制**：仅 `http://` 和 `https://`
- **安全重定向**：重定向目标重新验证
- **响应体限制**：10MB

适用于：`web_fetch`、`web_search`、`arxiv_search`、`semantic_scholar_search`、`pdf_fetch`。

### SQL 注入防护

`db_query` 工具额外防护：
- SQL 黑名单：`DROP`、`DELETE`、`TRUNCATE`、`ALTER`、`CREATE` 等
- 表名验证：仅允许字母数字、`_`、`.`
- URL scheme 验证：仅 `sqlite`、`mysql`、`postgresql`

---

## 9. 审计日志

所有工具调用、权限决策、Guard 拦截均被记录：

```rust
let logs = state.get_audit_logs().await;
// 每条：tool_name, decision, reason, source, timestamp, duration
```

```bash
export ECHO_AUDIT_MAX_ENTRIES=20000  # 默认 10000
```

---

## 10. 应用场景

### 开发模式（宽松）

```rust
let config = AgentConfig::new("qwen3.7-max", "agent", "...")
    .permission_mode(PermissionMode::AcceptEdits);

let policy = DefaultPermissionPolicy::new()
    .grant(ToolPermission::Read)
    .grant(ToolPermission::Write)
    .grant(ToolPermission::Network)
    .require_approval(ToolPermission::Execute);
```

### CI/CD 模式（严格）

```rust
let config = AgentConfig::new("qwen3.7-max", "agent", "...")
    .permission_mode(PermissionMode::DontAsk);

let mut registry = RuleRegistry::new();
registry.add_rule(PermissionRule::allow(
    RuleMatcher::Tool { name: "read_file".into() }, RuleSource::Default));
registry.add_rule(PermissionRule::allow(
    RuleMatcher::Tool { name: "web_search".into() }, RuleSource::Default));
registry.add_rule(PermissionRule::deny(
    RuleMatcher::All, "CI/CD 白名单模式".into(), RuleSource::Default));
```

### 企业部署（管理员策略）

```rust
// 管理员策略（最高优先级，用户无法覆盖）
registry.add_rule(PermissionRule::deny(
    RuleMatcher::Tool { name: "shell".into() },
    "企业策略禁止 shell".into(), RuleSource::Managed));
```

---

## 11. 安全检查清单

部署前检查：

- [ ] 启用 JWT 认证（`AUTH_ENABLED=true`）
- [ ] 设置强 JWT 密钥（≥32 字符）
- [ ] Server host 设为 `127.0.0.1`（或配置 TLS 反向代理）
- [ ] 危险工具（shell、delete_file、git_commit）配置 `ask` 权限
- [ ] 启用沙箱执行
- [ ] API key 仅通过环境变量注入
- [ ] MCP server 仅连接受信任来源
- [ ] 定期查看审计日志

---

## 参考

- [工具系统](./02-tools.md)
- [Human-in-the-Loop](./05-human-loop.md)
- [Guard 系统](./18-guard-system.md)
- [Hooks 系统](./23-hooks.md)
