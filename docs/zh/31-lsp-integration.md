# LSP 集成（Language Server Protocol）

## 是什么

LSP（Language Server Protocol）是微软提出的开放标准，将代码智能能力（诊断、补全、跳转定义、查找引用、悬浮提示）从编辑器中解耦出来。语言服务端针对特定编程语言实现这些能力，任何 LSP 客户端都可以连接并使用它们。

echo-agent 实现了完整的 LSP 客户端，将语言服务端的能力暴露为一等公民级别的 **Agent 工具**。AI Agent 可以对任何配置了语言服务端的项目执行 `lsp_diagnostics`、`lsp_goto_definition`、`lsp_find_references`、`lsp_hover` 等操作——将静态代码分析变为 Agent 在任务中可以主动推理的能力。

> **特性开关**：LSP 集成需要启用 `lsp` Cargo feature。详见[启用特性](#启用特性)。

---

## 解决什么问题

### 脱离 IDE 的代码感知

传统 Agent 编辑代码时通常是"盲人摸象"：它能读取文本，却看不到编译错误、找不到定义、也不知道谁在调用。LSP 弥补了这个鸿沟：

```
传统 Agent：
  读文件 → 编辑文件 → 祈祷能编译过

LSP 加持的 Agent：
  读文件 → 编辑文件 → lsp_diagnostics → 修复错误 → lsp_find_references → 验证调用方
```

### 统一的多语言支持

LSP 与具体语言无关。同一套 Agent 代码可以对接 `rust-analyzer`、`pyright`、`typescript-language-server`、`clangd`、`gopls` 或任何其他 LSP 服务端——只需切换配置文件。

```
                    ┌─────────────────────────────────────┐
                    │      echo-agent (LspManager)        │
                    │                                     │
                    │  ┌──────────┐ ┌──────────┐ ┌──────┐ │
                    │  │ rust-an. │ │ pyright  │ │ tss. │ │
                    │  └────┬─────┘ └────┬─────┘ └──┬───┘ │
                    └───────┼────────────┼──────────┼─────┘
                            │            │          │
                       .rs 文件      .py 文件   .ts/.js 文件
```

---

## 架构

echo-agent 将 LSP 支持分布在三个 crate 中，各司其职：

```
echo-core/src/lsp/              Trait + 类型（LspClient, Position, Diagnostic, ...）
       │
       ▼
echo-integration/src/lsp/       进程管理（LspManager, StdioLspClient, LspConfig）
       │
       ▼
src/tools/lsp.rs                工具封装（LspDiagnosticsTool, LspGotoDefinitionTool, ...）
```

核心 `LspClient` trait 定义契约；`StdioLspClient` 通过启动子进程并使用 JSON-RPC 在 stdin/stdout 上通信来实现该契约；工具封装层将每项能力适配为 Agent 可调用的标准 `Tool`。

### 组件职责

| 组件 | Crate | 职责 |
|------|-------|------|
| `LspClient` trait | `echo-core` | 面向语言服务端的对象安全接口 |
| `StdioLspClient` | `echo-integration` | 启动服务端进程，基于 stdio 的 JSON-RPC 通信 |
| `LspConfig` | `echo-integration` | 解析 `.lsp.yaml` 配置文件 |
| `LspManager` | `echo-integration` | 管理多个服务端，按文件扩展名路由请求 |
| LSP 工具 | `echo-agent` | 将能力适配为 `Tool` trait 供 Agent 使用 |

---

## 启用特性

LSP 支持受 `lsp` Cargo feature 控制。在 `Cargo.toml` 中启用：

```toml
[dependencies]
echo-agent = { version = "0.1", features = ["lsp"] }
```

或使用 `full` 启用全部功能：

```toml
echo-agent = { version = "0.1", features = ["full"] }
```

未启用该特性时，`lsp_*` 工具和 `register_lsp_tools` 函数不会被编译进二进制。

---

## 可用 LSP 工具

所有 LSP 工具都是 `ReadOnly`（只读），永远不会修改代码库。它们需要在 Agent 启动时传入已初始化的 `LspManager`。

### `lsp_diagnostics`

获取文件中的编译错误、警告和提示。

**参数**：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_path` | string | 是 | 文件的绝对路径 |

**示例**：
```json
{
  "file_path": "/home/user/project/src/main.rs"
}
```

**输出**：
```
Diagnostics for /home/user/project/src/main.rs (2 issues):

  [ERROR] line 12:5 — mismatched types: expected `i32`, found `String`
  [WARNING] line 45:1 — unused variable: `x`
```

诊断信息来自服务端的 `textDocument/publishDiagnostics` 通知流，由 `StdioLspClient` 本地缓存。严重级别映射为 `ERROR`、`WARNING`、`INFO`、`HINT`。

### `lsp_goto_definition`

查找符号（函数、类、变量、trait）的定义位置。

**参数**：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_path` | string | 是 | 文件的绝对路径 |
| `line` | integer | 是 | 行号（从 1 开始） |
| `column` | integer | 是 | 列号（从 1 开始） |

**示例**：
```json
{
  "file_path": "/home/user/project/src/main.rs",
  "line": 12,
  "column": 5
}
```

**输出**：
```
Definitions found:

  /home/user/project/src/lib.rs:42:1
```

注意：工具 API 使用从 1 开始的行列号（符合人类阅读习惯），内部会转换为 LSP 协议要求的从 0 开始的索引。

### `lsp_find_references`

查找符号在整个代码库中的所有使用位置。

**参数**：与 `lsp_goto_definition` 相同（file_path、line、column）。

**示例输出**：
```
References found (5 total):

  /home/user/project/src/main.rs:12:5
  /home/user/project/src/main.rs:45:10
  /home/user/project/src/lib.rs:88:3
  /home/user/project/tests/integration.rs:120:15
  /home/user/project/tests/integration.rs:205:8
```

默认包含声明本身（`includeDeclaration: true`）。

### `lsp_hover`

获取符号的类型信息、文档和签名。

**参数**：与 `lsp_goto_definition` 相同（file_path、line、column）。

**示例输出**：
```rust
pub fn read_to_string<P: AsRef<Path>>(path: P) -> Result<String>

Reads the entire contents of a file into a string.

# Errors
This function will return an error if the file does not exist
or if the contents are not valid UTF-8.
```

悬浮内容在服务端支持时以 Markdown 格式返回（大多数现代服务端都支持）。

### `lsp_status`

显示所有已配置和正在运行的语言服务端状态。

**参数**：无。

**示例输出**：
```
Language Servers (3 configured):

  rust [running] (pid: 12345)
  python [running] (pid: 12346)
  typescript [stopped]
    Error: Failed to initialize typescript server: spawn ENOENT
```

服务端状态：`running`（已初始化并接受请求）、`starting`（进程已启动，握手进行中）、`stopped`（未运行）。

---

## 配置 LSP 服务端

### `.lsp.yaml` 格式

语言服务端通过项目根目录下的 `.lsp.yaml` 文件配置：

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

### 配置字段

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `language` | string | 是 | 语言标识符 |
| `command` | string | 是 | 启动语言服务端的命令 |
| `args` | string[] | 否 | 命令行参数（默认：`[]`） |
| `extensions` | string[] | 否 | 该服务端处理的文件扩展名 |
| `env` | map | 否 | 传递给服务端进程的环境变量 |
| `initialization_options` | object | 否 | 自定义初始化选项（JSON） |
| `max_restarts` | integer | 否 | 放弃前的最大重启次数（默认：`3`） |

### 路由原理

`LspManager` 根据配置构建 **扩展名 → 语言** 的映射表。当 LSP 工具以某个文件路径被调用时，管理器：

1. 提取文件扩展名（如 `.rs`）
2. 查找语言名（如 `rust`）
3. 找到该语言正在运行的 `StdioLspClient`
4. 将请求路由到该客户端

如果该文件语言没有正在运行的服务端，工具会返回错误并建议用户启动对应服务端。

### 代码中加载配置

```rust
use echo_agent::lsp::{LspConfig, LspManager};
use std::path::Path;

let mut manager = LspManager::new();

// 从 .lsp.yaml 加载
let config = LspConfig::from_file(Path::new(".lsp.yaml"))?;
manager.load_config(&config);

// 设置工作区根目录（作为 initialize 时的 rootUri）
manager.set_project_root(Path::new("/home/user/project"));

// 启动特定语言的服务端
manager.start_server("rust").await?;
```

---

## LspManager 生命周期

`LspManager` 拥有所有语言服务端进程，并将请求路由到正确的服务端：

```
┌──────────────────────────────────────────────────────────────┐
│                       LspManager                             │
│                                                              │
│  configs: { "rust" → LspServerConfig, ... }                  │
│  extension_map: { ".rs" → "rust", ".py" → "python", ... }    │
│  clients: { "rust" → Arc<RwLock<StdioLspClient>>, ... }      │
│                                                              │
│  操作:                                                        │
│    load_config()       — 解析 .lsp.yaml                      │
│    set_project_root()  — 设置工作区根 URI                    │
│    start_server(lang)  — 启动 + 初始化一个服务端             │
│    stop_server(lang)   — 优雅关闭一个服务端                  │
│    restart_server(lang) — 停止 + 启动                        │
│    get_client_for_file(path) — 按扩展名路由                  │
│    status_all()        — 所有已配置服务端状态                │
│    shutdown_all()      — 停止所有运行中的服务端              │
└──────────────────────────────────────────────────────────────┘
```

### 服务端生命周期

```
new() ─> initialize(root_uri) ─> [请求 / 通知]* ─> shutdown()
         │                          │
         ├─ 启动子进程              ├─ textDocument/definition
         ├─ 发送 initialize         ├─ textDocument/references
         └─ 发送 initialized        ├─ textDocument/hover
                                    ├─ textDocument/publishDiagnostics（通知）
                                    └─ ...
```

---

## 示例：为 Rust 项目配置 rust-analyzer

### 1. 安装 rust-analyzer

```bash
rustup component add rust-analyzer
# 或：cargo install rust-analyzer
```

验证是否在 `PATH` 中：
```bash
rust-analyzer --version
```

### 2. 在项目根目录创建 `.lsp.yaml`

```yaml
languages:
  rust:
    language: rust
    command: rust-analyzer
    args: []
    extensions: [".rs"]
```

### 3. 将 LSP 接入 Agent

```rust
use echo_agent::prelude::*;
use echo_agent::lsp::{LspConfig, LspManager};
use std::sync::Arc;
use tokio::sync::RwLock;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    // 构建启用工具的 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("rust-reviewer")
        .system_prompt("你是一个 Rust 代码审查助手。使用 LSP 工具检查项目。")
        .enable_tools()
        .build()?;

    // 配置 LspManager
    let mut manager = LspManager::new();
    let config = LspConfig::from_file(std::path::Path::new(".lsp.yaml")).unwrap();
    manager.load_config(&config);
    manager.set_project_root(std::path::Path::new("/home/user/project"));

    // 启动 rust-analyzer
    manager.start_server("rust").await.expect("无法启动 rust-analyzer");

    // 封装并注册 LSP 工具
    let lsp_manager = Arc::new(RwLock::new(manager));
    register_lsp_tools(&mut agent, lsp_manager.clone());

    // 执行代码审查任务
    let answer = agent.execute(
        "审查 src/main.rs：检查诊断信息，然后找到 `process` 函数的所有引用。"
    ).await?;
    println!("{}", answer);

    // 优雅关闭
    lsp_manager.write().await.shutdown_all().await;
    Ok(())
}
```

### 4. Agent 看到的内容

在任务执行过程中，Agent 可以调用：
- `lsp_diagnostics` 获取 `/home/user/project/src/main.rs` 的编译错误
- `lsp_goto_definition` 在 `第 42 行, 第 10 列` 的符号上跳转到其定义
- `lsp_find_references` 在 `第 15 行, 第 4 列` 的 `process` 上找到每个调用点
- `lsp_hover` 获取完整的签名和文档注释
- `lsp_status` 验证 `rust-analyzer` 正在运行

---

## 示例：使用 LSP 工具进行代码导航

更复杂的场景：Agent 使用 LSP 理解一个陌生代码库：

```rust
// 系统提示词指导 Agent 使用 LSP 进行导航
let system_prompt = r#"
你是一个代码导航助手。帮助用户理解陌生的代码库。

当被问到某个符号时：
1. 使用 lsp_goto_definition 找到其定义位置。
2. 使用 lsp_find_references 找到其所有使用位置。
3. 使用 lsp_hover 获取其文档和类型签名。
4. 使用 lsp_diagnostics 标记你检查过的文件中的任何问题。

始终报告文件路径和行号，方便用户跳转。
"#;

let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .name("code-navigator")
    .system_prompt(system_prompt)
    .enable_tools()
    .build()?;

// ... LspManager 配置同上 ...

let answer = agent.execute(
    "追踪 /home/user/project/src/models/user.rs 中 `UserRecord` 结构体的数据流。\
     它在哪里定义？在哪里使用？这些文件中有编译错误吗？"
).await?;
```

Agent 会迭代调用 LSP 工具来构建完整的视图——定义位置、所有用法、类型签名以及任何错误——然后综合出一个可读的答案。

---

## StdioLspClient 内部原理

`StdioLspClient` 是与单个语言服务端通信的核心组件：

```
┌───────────────────┐                    ┌───────────────────┐
│   echo-agent      │                    │    语言服务端      │
│   StdioLspClient  │                    │    （子进程）      │
│                   │   stdin (JSON-RPC) │                   │
│                   │   ───────────────> │                   │
│                   │                    │                   │
│                   │   stdout (JSON-RPC)│                   │
│                   │   <─────────────── │                   │
│                   │                    │                   │
│                   │   stderr (日志)    │                   │
│                   │   <─────────────── │                   │
└───────────────────┘                    └───────────────────┘
```

关键实现细节：

- **分帧消息**：LSP 使用 `Content-Length: N\r\n\r\n<body>` 分帧格式，而非换行分隔的 JSON。`jsonrpc::encode_message` 负责编码；读取循环负责解析头部。
- **异步 I/O**：stdin 写入由专门的 writer 任务分发；stdout 读取在 reader 任务中运行。这避免了慢响应阻塞发送请求。
- **请求挂起表**：每个发送的请求都有唯一的 `id` 和一个 `oneshot` 通道。当响应到达时，reader 任务通过 `id` 查找并通知该通道。
- **诊断缓存**：服务端通过 `textDocument/publishDiagnostics` 通知推送诊断信息。reader 任务按文件 URI 缓存；`diagnostics()` 返回缓存的快照。
- **优雅关闭**：`shutdown()` 先发送 LSP `shutdown` 请求，再发送 `exit` 通知，最后在进程未自行退出时 kill 进程。

---

## 错误处理

LSP 操作可能以多种方式失败：

| 错误 | 说明 | 处理建议 |
|------|------|---------|
| `LspError::NotInitialized` | 在 `initialize()` 之前调用操作 | 确保 `start_server()` 已完成 |
| `LspError::NotRunning(lang)` | `lang` 的服务端未运行 | 调用 `start_server(lang)` |
| `LspError::SpawnError(msg)` | 无法启动服务端进程 | 检查 `command` 是否在 `PATH` 中且可执行 |
| `LspError::ServerError(msg)` | 服务端返回 JSON-RPC 错误 | 检查服务端日志（stderr）了解详情 |
| `LspError::CommunicationError(msg)` | stdin/stdout 通道已关闭 | 服务端可能已崩溃——检查 `status_all()` |
| `LspError::Timeout` | 请求超时 | 服务端可能在忙于索引；重试 |
| `LspError::InvalidUri(msg)` | 格式错误的文件 URI | 使用绝对路径 |

工具不会将错误传播到 Agent 运行时——它们返回包含人类可读消息的 `ToolResult::error(...)`，让 Agent 自行决定下一步操作。

---

## 支持的语言服务端

任何支持 stdio 传输的符合 LSP 规范的服务端都可以使用。常见选择：

| 语言 | 服务端 | 安装方式 |
|------|--------|---------|
| Rust | `rust-analyzer` | `rustup component add rust-analyzer` |
| Python | `pyright-langserver` | `npm i -g pyright` |
| TypeScript/JS | `typescript-language-server` | `npm i -g typescript-language-server` |
| Go | `gopls` | `go install golang.org/x/tools/gopls@latest` |
| C/C++ | `clangd` | 随 LLVM 发布 / `apt install clangd` |
| Java | `jdtls` | [Eclipse JDT LS](https://github.com/eclipse-jdtls/eclipse.jdt.ls) |
| Ruby | `solargraph` | `gem install solargraph` |
| Lua | `lua-language-server` | [lua-language-server](https://github.com/LuaLS/lua-language-server) |

---

## 参见

- `examples/` — LSP 集成示例
- [02-tools.md](02-tools.md) — 工具系统概览
- [08-mcp.md](08-mcp.md) — MCP 协议集成（外部工具服务）
- [28-config-reference.md](28-config-reference.md) — 完整配置参考
