# 插件系统

## 是什么

插件系统允许在不修改核心代码的前提下，通过声明式的 `manifest.yaml` 扩展 Agent 的能力。一个插件是一个自包含的目录，可以提供以下组件：

| 组件 | 当前状态 | 说明 |
|------|---------|------|
| Skills | 已接入 `SkillRegistry` | SKILL.md 文件，支持在线装卸 |
| Hooks | 已接入 `HookRegistry` | 生命周期和工具钩子，支持在线装卸 |
| MCP Servers | 已接入 `McpManager` | MCP 服务器及工具，支持在线装卸 |
| Agents | 仅发现 | 应用层尚未原子构造 definition + executable factory |
| LSP Servers | 仅发现 | `ReactAgent` 尚未持有 `LspManager` |
| Monitors | 仅发现 | 尚无运行时消费者 |
| Themes / Output Styles | 仅发现 | 由应用 UI / 输出层消费，尚未接入 |

```
核心框架:  提供 React Agent 循环、工具执行、上下文管理
插件:      以目录为单位打包能力，通过 manifest 声明组件，按需加载
```

---

## 解决的问题

没有插件系统时，扩展 Agent 需要：
- **修改核心代码**：每增加一个能力都要改动框架源码
- **紧耦合**：自定义工具、钩子、MCP 配置散落在各处
- **难以分发**：无法将一组能力打包分享给团队或社区

插件系统将扩展点统一为"一个目录 + 一份 manifest"，实现即插即用。

---

## 架构

```
┌─────────────────────────────────────────────────────────────────┐
│                     Plugin System                                │
│                                                                  │
│   PluginRegistry                                                 │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  scan_all() → 发现插件                                    │  │
│   │  install()  → 安装插件（本地/Git）                         │  │
│   │  enable() / disable() → 启停插件                          │  │
│   │  resolve_dependencies() → 拓扑排序                        │  │
│   └──────────────────────────────────────────────────────────┘  │
│       │                                                          │
│       ▼                                                          │
│   PluginIntegrator                                               │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  wire_all() → 按依赖顺序装配                              │  │
│   │  ┌──────────┐  ┌──────────┐  ┌──────────┐               │  │
│   │  │ Skills   │  │ Hooks    │  │ MCP      │               │  │
│   │  │ → Agent  │  │ → Agent  │  │ → Agent  │               │  │
│   │  └──────────┘  └──────────┘  └──────────┘               │  │
│   └──────────────────────────────────────────────────────────┘  │
│       │                                                          │
│       ▼                                                          │
│   PluginVariables                                                │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │  ${ECHO_PLUGIN_ROOT} → 插件安装目录                       │  │
│   │  ${ECHO_PLUGIN_DATA} → 持久化数据目录                     │  │
│   │  ${ECHO_PROJECT_DIR} → 项目根目录                         │  │
│   │  ${user_config.*}    → 用户配置值                         │  │
│   └──────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

---

## 插件 Manifest 格式

每个插件的根目录下必须有 `.echo-plugin/manifest.yaml`：

```yaml
name: data-analysis-pack
display_name: "Data Analysis Pack"
version: "1.2.0"
description: "增强数据分析能力，集成 polars 扩展"
author:
  name: "Echo Team"
  email: "team@echo.dev"
license: MIT
keywords: [data, analysis, polars]

components:
  skills: "./skills/"
  agents:
    - "./agents/reviewer.md"
    - "./agents/analyst.md"
  hooks: "./hooks/hooks.yaml"
  mcp_servers: "./.mcp.json"

config:
  api_endpoint:
    type: string
    title: "API 端点"
    description: "数据服务地址"
    default: "http://localhost:8080"
  api_token:
    type: string
    title: "API Token"
    sensitive: true
    required: true

dependencies:
  - name: base-tools
    version: ">=1.0.0"
  - simple-dep
```

### Manifest 字段说明

| 字段 | 必须 | 说明 |
|------|------|------|
| `name` | ✓ | 唯一标识符，kebab-case（小写字母、数字、连字符） |
| `display_name` | | 人类可读名称，省略时使用 `name` |
| `version` | | 语义化版本号，如 `"1.2.0"`，默认 `"0.0.0"` |
| `description` | | 简要描述插件用途 |
| `author` | | 作者信息（`name`、`email`、`url`） |
| `license` | | 许可证标识（如 `"MIT"`、`"Apache-2.0"`） |
| `keywords` | | 发现标签，用于搜索和过滤 |
| `components` | | 组件声明——相对于插件根目录的路径 |
| `config` | | 用户可配置项，安装时提示填写 |
| `dependencies` | | 依赖的其他插件 |
| `default_enabled` | | 是否默认启用（默认 `true`） |

### 组件路径规则

所有组件路径必须：
- 以 `./` 开头（如 `"./skills/"`）
- 不得包含 `..`（禁止路径遍历，防止逃逸出插件根目录）

路径值支持单字符串或字符串数组：

```yaml
components:
  skills: "./skills/"              # 单路径
  agents:                           # 多路径
    - "./agents/reviewer.md"
    - "./agents/analyst.md"
```

省略时，部分组件使用默认路径：
- `skills` → `./skills/`
- `mcp_servers` → `./.mcp.json`

### 用户配置项类型

| 类型 | 说明 |
|------|------|
| `string` | 自由文本，可设置 `multiple: true` 允许数组 |
| `number` | 数值，可设置 `min` / `max` |
| `boolean` | 布尔开关 |
| `directory` | 目录路径（验证存在性） |
| `file` | 文件路径（验证存在性） |

配置项通用属性：
- `sensitive: true` — 遮蔽输入，存入安全存储
- `required: true` — 必填项
- `default` — 用户未提供时的默认值

---

## PluginScope：安装作用域

插件可以安装到三个不同的作用域：

| 作用域 | 路径 | 使用场景 |
|--------|------|---------|
| `User` | `~/.echo-agent/plugins/` | 个人插件，所有项目可用 |
| `Project` | `.echo-agent/plugins/` | 团队共享，通过 VCS 提交 |
| `Local` | `.echo-agent/plugins.local/` | 项目私有，gitignore 不提交 |

```rust
use echo_agent::plugin::{PluginScope, InstallSource};

// 解析作用域
let scope = PluginScope::from_arg("user").unwrap();    // "user" | "project" | "local"

// 获取文件系统路径
let dir = scope.resolve_dir(Some(Path::new("/home/user/my-project")));
// User    → /home/user/.echo-agent/plugins/
// Project → /home/user/my-project/.echo-agent/plugins/
// Local   → /home/user/my-project/.echo-agent/plugins.local/
```

---

## PluginRegistry API

`PluginRegistry` 是插件管理的核心枢纽，负责发现、安装、卸载、启停和依赖解析。

### 创建注册表

```rust
use echo_agent::plugin::PluginRegistry;

// 默认路径（~/.echo-agent/plugins/）
let mut registry = PluginRegistry::new(None);

// 指定项目根目录（解析 Project/Local 作用域）
let mut registry = PluginRegistry::new(Some(PathBuf::from("/home/user/my-project")));

// 自定义路径（测试用）
let mut registry = PluginRegistry::with_paths(
    PathBuf::from("/tmp/registry.json"),
    PathBuf::from("/tmp/data"),
    Some(PathBuf::from("/tmp/project")),
);
```

### 扫描发现

```rust
// 扫描所有作用域，加载已安装插件
let count = registry.scan_all().unwrap();
println!("发现 {} 个插件", count);
```

扫描逻辑：遍历每个作用域目录，查找包含 `.echo-plugin/manifest.yaml` 的子目录。

### 安装

```rust
use echo_agent::plugin::{InstallSource, PluginScope};

// 从本地目录安装
let source = InstallSource::Local(PathBuf::from("/path/to/my-plugin"));
let plugin_id = registry.install(&source, PluginScope::User)?;

// 从 Git 仓库安装（仅允许 https://）
let source = InstallSource::parse("https://github.com/echo/data-plugin.git");
let plugin_id = registry.install(&source, PluginScope::Project)?;

// 自动检测安装源
let source = InstallSource::parse("./my-plugin");       // → Local
let source = InstallSource::parse("https://...git");    // → Git
```

### 卸载

```rust
// 卸载并删除数据目录
registry.uninstall("data-analysis-pack", false)?;

// 卸载但保留数据目录
registry.uninstall("data-analysis-pack", true)?;
```

### 启用 / 禁用

```rust
// 禁用插件（不卸载，保留文件和数据）
registry.disable("data-analysis-pack")?;

// 重新启用
registry.enable("data-analysis-pack")?;
```

启停状态会持久化到 `registry.json`，重启后自动恢复。

### 查询

```rust
// 列出所有已安装插件
for entry in registry.list() {
    println!("{} v{} [{}]",
        entry.manifest.name,
        entry.manifest.version,
        if entry.enabled { "enabled" } else { "disabled" }
    );
}

// 仅列出已启用插件
for entry in registry.list_enabled() {
    println!("{}", entry.manifest.display_name());
}

// 按关键词搜索（匹配 name、description、keywords）
let results = registry.search("polars");

// 获取单个插件详情
if let Some(entry) = registry.get("data-analysis-pack") {
    println!("安装路径: {}", entry.root.display());
    println!("作用域: {}", entry.scope);
}

// 总数
println!("已安装 {} 个插件", registry.count());
```

### 依赖解析

```rust
// 拓扑排序：依赖在前，被依赖者在后
let ordered = registry.resolve_dependencies()?;
// 例如 A 依赖 B，B 依赖 C → 返回 [C, B, A]

// 错误情况：
// - 缺少依赖："Plugin 'a' depends on 'b' which is not installed"
// - 循环依赖："Circular dependency detected among plugins"
```

---

## 插件生命周期

插件从安装到卸载经历以下阶段：

```
安装 → 扫描发现 → 解析组件 → 装配到 Agent → 启用/禁用 → 卸载
  │                                        │
  │  install()                             │  enable() / disable()
  │  scan_all()                            │
  │  resolve_components()                  │
  │  PluginIntegrator::wire_all()          │
  ▼                                        ▼
```

### 生命周期回调（PluginLifecycle trait）

对于需要代码级生命周期管理的插件，实现 `PluginLifecycle` trait：

```rust
use echo_agent::plugin::PluginLifecycle;

struct MyPluginLifecycle;

impl PluginLifecycle for MyPluginLifecycle {
    /// 插件加载后调用一次，执行初始化
    fn init(&self) -> Result<(), String> {
        // 启动后台进程、建立连接、初始化缓存
        Ok(())
    }

    /// 插件启用时调用（或 default_enabled: true 时启动即调用）
    fn activate(&self) -> Result<(), String> {
        // 启动监控器等激活时逻辑
        Ok(())
    }

    /// 插件禁用时调用
    fn deactivate(&self) -> Result<(), String> {
        // 停止后台进程、释放资源
        Ok(())
    }

    /// Agent 关闭时调用
    fn shutdown(&self) -> Result<(), String> {
        // 刷缓冲、关闭连接、保存状态到 ${ECHO_PLUGIN_DATA}
        Ok(())
    }
}
```

生命周期流转：

```text
load → init → activate ⇄ deactivate → shutdown
                   ↑          ↓
                   └──────────┘  (可循环，如 reload)
```

---

## 组件装配

`PluginIntegrator` 负责将插件的组件接入 Agent 的各子系统：

```rust
use echo_agent::plugin::PluginIntegrator;

let integrator = PluginIntegrator::new();
let result = integrator.wire_all(&mut agent, &mut registry).await;

println!("加载 {} 个 Skills", result.skills_loaded.len());
println!("注册 {} 个 Hook 源", result.hooks_registered.len());
println!("连接 {} 个 MCP 服务器", result.mcp_connected.len());

if !result.is_ok() {
    for err in &result.errors {
        eprintln!("装配错误: {}", err);
    }
}

println!("共装配 {} 个组件", result.total_wired());
```

装配顺序：

1. `resolve_dependencies()` 确定插件加载顺序
2. 对每个已启用插件调用 `resolve_components()` 解析路径
3. 按类型分别接入：

| 组件 | 装配方式 |
|------|---------|
| Skills | `agent.load_skills_from_dir()` |
| Hooks | `hook_registry.register("plugin:{name}", ...)` |
| MCP Servers | `agent.load_mcp_from_file()` |
| Agents / LSP / Monitors / Themes / Output Styles | 仅返回 `*_discovered` 路径，不计入已装配组件 |

也可以只装配部分组件：

```rust
// 仅装配 Skills
integrator.wire_skills(&mut agent, &skill_dirs).await;

// 仅装配 Hooks
integrator.wire_hooks(&agent, &hooks_defs).await;

// 仅装配 MCP Servers（需要 mcp feature）
integrator.wire_mcp(&mut agent, &mcp_files).await;
```

---

## 变量替换

插件配置中可使用变量占位符，运行时自动替换为实际路径或值。

### 内置变量

| 变量 | 值 |
|------|-----|
| `${ECHO_PLUGIN_ROOT}` | 插件安装目录的绝对路径 |
| `${ECHO_PLUGIN_DATA}` | 插件持久化数据目录（跨更新保留） |
| `${ECHO_PROJECT_DIR}` | 项目根目录 |

### 用户配置变量

通过 `${user_config.KEY}` 引用 manifest 中 `config` 声明的用户配置值：

```yaml
# manifest.yaml
config:
  api_endpoint:
    type: string
    title: "API Endpoint"
    default: "http://localhost:8080"
```

在组件配置中使用：

```json
{
  "server": {
    "url": "${user_config.api_endpoint}/api/v1"
  }
}
```

### 环境变量

`${ENV_VAR}` 形式的 OS 环境变量也会被替换。未找到的变量保持原样不删除。

### 编程使用

```rust
use echo_agent::plugin::PluginVariables;
use std::collections::HashMap;

let vars = PluginVariables::new(
    "my-plugin",
    PathBuf::from("/home/user/.echo-agent/plugins/my-plugin"),
    PathBuf::from("/home/user/my-project"),
);

// 添加用户配置
let mut config = HashMap::new();
config.insert("api_endpoint".into(), "http://localhost:9090".into());
let vars = vars.with_user_config(config);

// 替换变量
let result = vars.substitute("run ${ECHO_PLUGIN_ROOT}/scripts/start.sh");
// → "run /home/user/.echo-agent/plugins/my-plugin/scripts/start.sh"

let result = vars.substitute("connect to ${user_config.api_endpoint}");
// → "connect to http://localhost:9090"

// 解析相对路径
let abs = vars.resolve_path("./skills/my-skill");
// → /home/user/.echo-agent/plugins/my-plugin/skills/my-skill

// 确保数据目录存在
vars.ensure_data_dir()?;
```

### 导出为环境变量

将插件变量导出为进程环境变量（供 hook 脚本等子进程使用）：

```rust
use echo_agent::plugin::variables::export_to_env;

// ⚠️ 必须在单线程初始化阶段调用（set_var 非线程安全）
export_to_env(&vars);
// 设置: ECHO_PLUGIN_ROOT, ECHO_PLUGIN_DATA, ECHO_PROJECT_DIR
// 用户配置: ECHO_PLUGIN_OPTION_{KEY} (大写)
```

---

## 安全性

### Git 克隆限制

从 Git 安装插件时，仅允许 `https://` 协议：

```rust
// ✅ 允许
InstallSource::parse("https://github.com/echo/plugin.git")

// ❌ 拒绝
InstallSource::parse("file:///etc/passwd")        // SSRF 防护
InstallSource::parse("ssh://git@host/repo")        // 非 HTTPS
InstallSource::parse("git://host/repo")            // 非 HTTPS
InstallSource::parse("http://host/repo")           // 非加密 HTTP
```

此外，私有 IP 地址（`127.x`、`10.x`、`172.16-31.x`、`192.168.x`、`0.x`）也会被拒绝，防止 SSRF 攻击。

### 路径遍历防护

Manifest 验证器拒绝所有包含 `..` 的组件路径：

```yaml
# ❌ 验证失败
components:
  skills: "../shared-skills/"   # 路径遍历！

# ✅ 正确
components:
  skills: "./skills/"
```

### 变量名校验

导出环境变量时，用户配置的 key 仅允许 `[A-Z0-9_]` 字符，防止环境变量注入攻击：

```rust
// 合法: "API_ENDPOINT" → ECHO_PLUGIN_OPTION_API_ENDPOINT
// 非法: "api;rm -rf /" → 跳过并记录警告
```

### Manifest 验证

`PluginManifest::validate()` 执行全面检查：

```rust
let manifest = PluginManifest::from_file(&path)?;
let errors = manifest.validate();

// 检查项：
// - name 非空且为 kebab-case
// - version 为合法 semver
// - 所有组件路径以 ./ 开头，不含 ..
// - config key 为合法标识符
// - multiple 仅用于 string 类型
// - 依赖名称为 kebab-case

if !errors.is_empty() {
    for e in &errors {
        eprintln!("{}: {}", e.field, e.message);
    }
}
```

---

## 示例：创建插件 Manifest

创建一个简单的代码审查插件：

### 目录结构

```
code-review-plugin/
├── .echo-plugin/
│   └── manifest.yaml
├── skills/
│   └── code-review/
│       ├── SKILL.md
│       └── references/
│           └── checklist.md
├── hooks/
│   └── hooks.yaml
└── .mcp.json
```

### .echo-plugin/manifest.yaml

```yaml
name: code-review-plugin
display_name: "Code Review Plugin"
version: "1.0.0"
description: "自动化代码审查，支持自定义检查清单和安全扫描"
author:
  name: "My Team"
license: MIT
keywords: [code-review, security, quality]

components:
  skills: "./skills/"
  hooks: "./hooks/hooks.yaml"
  mcp_servers: "./.mcp.json"

config:
  strict_mode:
    type: boolean
    title: "严格模式"
    description: "启用后任何警告都会阻止提交"
    default: false
  exclude_patterns:
    type: string
    title: "排除模式"
    description: "跳过检查的文件 glob 模式"
    multiple: true
    default: ["*.generated.*", "vendor/**"]

default_enabled: true
```

### hooks/hooks.yaml

```yaml
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: prompt
          prompt: "执行命令前请确认不会影响代码仓库状态"
  PostToolUse:
    - matcher: "*"
      hooks:
        - type: command
          command: "${ECHO_PLUGIN_ROOT}/scripts/log_tool_usage.sh"
          timeout: 3
```

---

## 示例：编程安装和使用插件

```rust
use echo_agent::prelude::*;
use echo_agent::plugin::{
    PluginRegistry, PluginScope, InstallSource,
    PluginIntegrator, PluginVariables,
};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. 创建 Agent
    let config = AgentConfig::new("qwen3-max", "assistant", "你是一个有帮助的助手")
        .enable_tool(true);
    let mut agent = ReactAgent::new(config);

    // 2. 创建插件注册表
    let project_root = PathBuf::from("/home/user/my-project");
    let mut registry = PluginRegistry::new(Some(project_root.clone()));

    // 3. 扫描已安装插件
    let count = registry.scan_all()?;
    println!("发现 {} 个已安装插件", count);

    // 4. 从本地目录安装新插件
    let source = InstallSource::Local(PathBuf::from("/path/to/code-review-plugin"));
    let plugin_id = registry.install(&source, PluginScope::User)?;
    println!("已安装: {}", plugin_id);

    // 5. 从 Git 安装
    let source = InstallSource::parse("https://github.com/echo/data-plugin.git");
    let plugin_id = registry.install(&source, PluginScope::Project)?;
    println!("已安装: {}", plugin_id);

    // 6. 查看已启用插件
    for entry in registry.list_enabled() {
        println!("- {} v{}: {}",
            entry.manifest.name,
            entry.manifest.version,
            entry.manifest.description
        );
    }

    // 7. 装配所有插件到 Agent
    let integrator = PluginIntegrator::new();
    let result = integrator.wire_all(&mut agent, &mut registry).await;
    println!("装配完成: {} 个组件", result.total_wired());

    if !result.is_ok() {
        for err in &result.errors {
            eprintln!("警告: {}", err);
        }
    }

    // 8. 使用变量替换
    if let Some(entry) = registry.get("code-review-plugin") {
        let vars = PluginVariables::new(
            "code-review-plugin",
            entry.root.clone(),
            project_root,
        );
        let cmd = vars.substitute("${ECHO_PLUGIN_ROOT}/scripts/run.sh");
        println!("执行: {}", cmd);
    }

    // 9. 禁用不需要的插件
    registry.disable("data-plugin")?;

    // 10. 卸载插件
    registry.uninstall("data-plugin", false)?;

    // 11. 现在 Agent 已具备所有插件能力，正常使用
    let response = agent.execute("请审查这段代码").await?;
    println!("{}", response);

    Ok(())
}
```

---

## NativePlugin（遗留接口）

对于需要注入自定义 Rust 逻辑的代码级扩展，保留 `NativePlugin` trait：

```rust
use echo_agent::plugin::NativePlugin;

struct MyNativePlugin;

impl NativePlugin for MyNativePlugin {
    fn id(&self) -> &str { "my-native-plugin" }
    fn name(&self) -> &str { "My Native Plugin" }
    fn version(&self) -> &str { "1.0.0" }
    fn capabilities(&self) -> Vec<PluginCapability> {
        vec![PluginCapability::Tool]
    }
    fn init(&mut self) -> Result<(), String> {
        // 自定义初始化逻辑
        Ok(())
    }
    fn shutdown(&mut self) -> Result<(), String> {
        // 自定义清理逻辑
        Ok(())
    }
}
```

> **提示**：大多数场景推荐使用基于 `manifest.yaml` 的文件插件。仅在需要运行时 Rust 逻辑时使用 `NativePlugin`。

---

## 插件数据目录

每个插件拥有独立的持久化数据目录，跨更新保留：

```
~/.echo-agent/plugins/data/
├── code-review-plugin/     ← code-review-plugin 的数据
├── data-analysis-pack/     ← data-analysis-pack 的数据
└── my-plugin/              ← my-plugin 的数据
```

数据目录名称由插件名自动清理（非 `[a-zA-Z0-9_-]` 字符替换为 `-`）。

卸载时可选择保留数据目录：

```rust
registry.uninstall("my-plugin", true)?;  // keep_data = true
```
