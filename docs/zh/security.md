# 安全指南

Echo Agent 提供了多层安全机制来保护你的系统和数据。本指南涵盖工具安全模型、沙箱配置、密钥管理和 MCP 信任边界。

---

## 1. 工具安全模型

### 权限声明

每个内置工具都声明了其所需的权限和风险等级：

| 工具 | 权限 | 风险等级 | 说明 |
|------|------|----------|------|
| `read_file` | `Read` | `ReadOnly` | 只读文件 |
| `write_file` | `Write` | `Standard` | 写入文件 |
| `delete_file` | `Write` | `Dangerous` | 删除文件 |
| `edit_file` | `Read, Write` | `Standard` | 编辑文件（自动创建 .bak 备份） |
| `shell` | `Execute` | `Dangerous` | 执行 shell 命令（白名单限制） |
| `web_fetch` | `Network` | `Standard` | 网络请求（SSRF 防护） |
| `web_search` | `Network` | `Standard` | Web 搜索（DuckDuckGo/Brave/Tavily） |
| `git_commit` | `Write, Execute` | `Dangerous` | 创建 Git 提交 |
| `run_skill_script` | `Execute` | `Dangerous` | 执行技能脚本 |
| `arxiv_search` | `Network` | `Standard` | ArXiv API（HTTPS，SSRF 重定向策略） |
| `semantic_scholar_search` | `Network` | `Standard` | Semantic Scholar API（HTTPS，SSRF 重定向） |
| `pdf_fetch` | `Network` | `Standard` | PDF 下载（SSRF 防护，%PDF 验证） |
| `bibtex_generate` | `Write` | `ReadOnly` | BibTeX 文本生成 |
| `rag_index` | `Read, Write` | `Standard` | 文档分块 + 向量索引 |
| `rag_search` | `Read` | `ReadOnly` | 索引文档的语义搜索 |
| `excel_read` / `excel_info` | `Read` | `ReadOnly` | 读取 Excel 文件 |
| `excel_write` | `Write` | `Standard` | 写入 Excel 文件 |
| `data_read` / `data_filter` / ... | `Read` | `ReadOnly` | 数据分析工具 |
| `data_transform` | `Read, Write` | `Standard` | 数据转换 |
| `generate_chart` | `Read, Write` | `Standard` | 图表图片生成 |
| `db_query` | `Network` | `Standard` | SQL 数据库查询（黑名单防护） |

### 权限类型

- **`Read`** — 读取文件/目录
- **`Write`** — 写入/修改文件
- **`Network`** — 网络访问
- **`Execute`** — 执行命令/代码
- **`Sensitive`** — 敏感操作（密钥、环境变量等）

### 审批链

工具执行经过以下审批链：

```
Tool.metadata (permissions, risk_level)
    → PolicyEngine (RuleRegistry + PermissionPolicy)
    → PermissionRequest (risk_level, context)
    → HumanLoopProvider (approve/deny/ask)
    → AuditLogger (记录决策)
    → SandboxExecutor (隔离执行)
```

### 自定义权限规则

通过 CLI 的 `/api/permissions` 端点或 `echo-agent.yaml` 配置：

```yaml
# echo-agent.yaml
permissions:
  mode: "prompt"            # default | prompt | auto_allow | auto_deny
  rules:
    - matcher: "tool:shell"
      behavior: "ask"
      source: "config"
    - matcher: "perm:execute"
      behavior: "ask"
      source: "config"
    - matcher: "*"
      behavior: "allow"
      source: "config"
```

匹配模式：
- `tool:<name>` — 精确匹配工具名
- `perm:<flag>` — 匹配权限标志（read/write/network/execute/sensitive）
- `*` — 匹配所有工具

### 读取后编辑强制

AgentConfig 中的 `force_read_before_edit: true` 要求模型在修改/删除文件之前必须先用 `read_file` 读取文件内容。这防止模型基于幻觉去修改文件。

```rust
let config = AgentConfig::new("qwen-plus", "agent", "system prompt")
    .force_read_before_edit(true);
```

---

## 2. 沙箱配置

### 安全级别

框架层 `SecurityLevel`（4 级隔离策略）：

| 级别 | 值 | 描述 |
|------|-----|------|
| `Trusted` | 0 | 无隔离，直接宿主执行 |
| `Standard` | 1 | 进程级隔离 |
| `Strict` | 2 | 容器隔离（Docker） |
| `Maximum` | 3 | 编排隔离（Kubernetes） |

### Docker 沙箱

```rust
use echo_agent::prelude::*;

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

### 网络隔离

```yaml
# echo-agent.yaml
sandbox:
  network_enabled: false      # 默认禁用网络
  max_memory_mb: 512
  max_cpu_seconds: 30
```

---

## 3. 密钥管理

### API Key 配置

通过环境变量注入，永远不要硬编码在配置文件中：

```bash
# OpenAI / Qwen / 兼容提供商
export OPENAI_API_KEY="sk-..."
export DASHSCOPE_API_KEY="sk-..."
export DEEPSEEK_API_KEY="sk-..."

# Anthropic
export ANTHROPIC_API_KEY="sk-ant-..."

# Web 搜索
export BRAVE_SEARCH_API_KEY="BSA..."
export TAVILY_API_KEY="tvly-..."
```

### JWT 认证

Web Server 模式支持 JWT 认证：

```bash
export AUTH_ENABLED=true
export JWT_SECRET="your-secret-at-least-32-characters-long"
export ADMIN_USERNAME="admin"
export ADMIN_PASSWORD="your-strong-password"
```

### 安全建议

- 永远不要在 `echo-agent.yaml` 中存储密钥
- 使用 `.env` 文件时确保已加入 `.gitignore`
- 生产环境必须启用 JWT 认证
- Server host 应设为 `127.0.0.1`（仅本地访问）
- 如需远程访问，使用 Nginx/Caddy 反向代理 + TLS

---

## 4. MCP 信任边界

### 本地 MCP Server

本地 MCP server 运行在与 Agent 相同的机器上。它们可以访问本地文件系统和网络。

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

### 远程 MCP Server

远程 MCP server 通过 HTTP/SSE 连接。需要额外的安全考量：

- 验证 server URL 是否为受信任的来源
- 远程 server 返回的工具可能与预期不符
- 建议为远程 MCP tool 设置严格的权限规则

```json
{
  "mcpServers": {
    "remote-search": {
      "transport": "sse",
      "url": "https://trusted-server.example.com/sse",
      "headers": {
        "Authorization": "Bearer ${MCP_TOKEN}"
      }
    }
  }
}
```

### MCP 安全最佳实践

1. **最小权限原则** — 每个 MCP server 只应访问必需的资源
2. **网络隔离** — 远程 server 应在独立网络环境中运行
3. **工具白名单** — 为 MCP 提供的工具设置 `ask` 或 `deny` 规则
4. **审计日志** — 所有 MCP tool 调用都会被记录

---

## 5. WebFetch SSRF 防护

WebFetch 工具内置 SSRF（Server-Side Request Forgery）防护：

- 禁止访问内网地址（127.0.0.1、10.0.0.0/8、172.16.0.0/12、192.168.0.0/16）
- 禁止访问 `localhost` 和 `.local` 域名
- 禁止访问 `file://`、`gopher://` 等非 HTTP(S) 协议
- 响应体硬限制 10MB（防止内存耗尽）
- 30 秒超时

---

## 6. 审计日志

所有工具调用、权限决策、Guard 拦截都会被记录：

```rust
// 查看审计日志（内存中，最多 10000 条）
let logs = state.get_audit_logs().await;

// 每条日志包含：
// - tool_name, decision (allow/deny/ask)
// - reason, source, timestamp
// - duration, elapsed time
```

可通过环境变量调整：
```bash
export ECHO_AUDIT_MAX_ENTRIES=20000  # 默认 10000
```

---

## 7. SSRF 防护

所有支持网络访问的工具共享统一的 SSRF（服务端请求伪造）防护层：

- **私有 IP 阻止**：127.0.0.0/8、10.0.0.0/8、172.16.0.0/12、192.168.0.0/16
- **Localhost 阻止**：`localhost`、`*.local` 域名
- **协议限制**：仅允许 `http://` 和 `https://`（阻止 `file://`、`gopher://` 等）
- **安全重定向策略**：重定向目标重新验证相同规则
- **共享 HTTP 客户端**：`OnceLock<reqwest::Client>`，可配置超时（30-60 秒）

此防护适用于：`web_fetch`、`web_search`、`arxiv_search`、`semantic_scholar_search`、`pdf_fetch`。

### SQL 注入防护

数据库工具（`db_query`）包含额外防护：

- **SQL 黑名单**：阻止 `DROP`、`DELETE`、`TRUNCATE`、`ALTER`、`CREATE`、`GRANT`、`EXECUTE`、`INTO OUTFILE`、`LOAD_FILE`
- **表名验证**：仅允许字母数字、`_` 和 `.` 字符
- **URL scheme 验证**：仅接受 `sqlite`、`mysql`、`postgresql`、`postgres`
- **MySQL 转义**：单引号转义为 `''`（非 `\'`）

---

## 8. 安全检查清单

部署前检查：

- [ ] 启用 JWT 认证（`AUTH_ENABLED=true`）
- [ ] 设置强 JWT 密钥（≥32 字符）
- [ ] Server host 设为 `127.0.0.1`（或配置 TLS 反向代理）
- [ ] 为危险工具（shell、delete_file、git_commit）配置 `ask` 权限
- [ ] 启用沙箱执行（Docker/K8s）
- [ ] API key 仅通过环境变量注入，不在配置文件中
- [ ] MCP server 仅连接受信任的来源
- [ ] 定期查看审计日志
