# 插件系统

EchoAgent 复用 Agent Plugins 1.0 的根清单、Skills 和 MCP 约定，并针对本地个人助理采用扁平插件包。每类组件只有一个固定位置，不引入客户端扩展 namespace，也不在清单中重复声明组件路径。

## 包结构

```text
my-plugin/
├── plugin.json
├── skills/
│   └── code-review/
│       └── SKILL.md
├── mcp.json
├── agents/
│   └── reviewer.md
├── hooks/
│   └── hooks.yaml
├── lsp.yaml
├── monitors.yaml
├── themes/
├── output-styles/
├── scripts/
└── README.md
```

`plugin.json` 直接放在插件根目录。旧的 `.echo-plugin/manifest.yaml` 不再兼容，也不会形成第二套解析路径。

## 清单

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "review.tools",
  "version": "1.0.0",
  "description": "Review workflows",
  "author": { "name": "Example Team" },
  "license": "MIT",
  "keywords": ["review"],
  "displayName": "Review Tools",
  "defaultEnabled": true,
  "config": {
    "endpoint": {
      "type": "string",
      "title": "Endpoint",
      "default": "https://example.com"
    }
  },
  "dependencies": [
    { "name": "base.tools", "version": ">=1.0.0" }
  ]
}
```

插件身份字段遵循 Agent Plugins 1.0。EchoAgent 额外读取根级 `displayName`、`defaultEnabled`、`config` 和 `dependencies`；未知顶层字段会被报告后忽略。

插件名长度为 1-64 个字符，只能包含小写 ASCII 字母、数字、连字符和句点；首尾必须为字母或数字，且不能包含 `--` 或 `..`。

## 标准 Skills

Skills 使用固定根目录，每个一级子目录表示一个 Skill：

```text
skills/<skill-name>/SKILL.md
```

插件 Skills 不递归扫描分类目录。单个无效 Skill 只会跳过自身，不影响同包的其他 Skills 或插件组件。

## 标准 MCP

MCP 使用根目录固定文件 `mcp.json`：

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
  "mcpServers": {
    "local-review": {
      "type": "stdio",
      "command": "node",
      "args": ["${PLUGIN_ROOT}/server.js"],
      "env": { "CACHE": "${PLUGIN_DATA}/cache" },
      "cwd": "${PLUGIN_ROOT}"
    },
    "remote-review": {
      "type": "streamable-http",
      "url": "https://example.com/mcp",
      "headers": { "X-Tenant": "public" }
    }
  }
}
```

支持 `stdio`、`streamable-http` 和旧版 `sse`。stdio 的 `command` 是一个裸可执行文件名，或以 `./` 开头的插件相对路径；框架不会把它交给 shell 解释。

EchoAgent 向 stdio 子进程提供 `PLUGIN_ROOT` 和 `PLUGIN_DATA`。`${PLUGIN_ROOT}`、`${PLUGIN_DATA}` 只在 `args`、环境变量值和 `cwd` 中进行一次非递归展开；不会在环境变量名、`command`、远程 URL 或 HTTP headers 中展开。插件不能覆盖这两个保留环境变量。

顶层 `mcp.json` 无效时，只禁用该插件的 MCP；单个服务配置无效、不可连接或重名时，只跳过该服务。

## 固定本地组件

其余组件从固定根位置发现：

| 位置 | 消费方 |
|---|---|
| `agents/*.md` | Subagent 适配器 |
| `hooks/hooks.yaml` | Hook registry |
| `lsp.yaml` | 宿主应用 LSP manager |
| `monitors.yaml` | embedding application 调度器 |
| `themes/*.json` | embedding application GUI/TUI 主题目录 |
| `output-styles/*.md` | embedding application system context 投影 |

`scripts/` 和 `README.md` 是插件资源，不会自动执行；Skill 或 Hook 可以显式引用脚本。

## 框架与应用分层

通用框架负责清单解析、Skills、MCP、作用域与生命周期、Hooks、Subagent 定义和 LSP 适配输出。embedding application 只发现并转换产品专属的 `monitors.yaml`、`themes/` 和 `output-styles/`；应用 integration 不重复拥有依赖排序、组件所有权或重载语义。

## 发现与生命周期

默认扫描范围：

| 作用域 | 默认位置 |
|---|---|
| User | `~/.echo-agent/plugins/<name>/plugin.json` |
| Project | `<project>/.echo-agent/plugins/<name>/plugin.json` |
| Local | `<project>/.echo-agent/plugins.local/<name>/plugin.json` |

应用可以覆盖插件数据根目录，embedding application 会将其设为 `<application-data>`。

插件按依赖顺序加载。清单致命错误会跳过整个包；组件错误按最小可行边界隔离。运行时记录每个插件的组件所有权，因此 disable、uninstall 和 reload 能精确卸载对应组件。

`PluginIntegrator::prepare` 捕获唯一不可变的 `PreparedPluginSet`，包括单调 generation、确定性
内容 identity、结构化诊断、已解析的 Skills/Hooks/MCP，以及保留 owner 的 Subagent/LSP 文档。
`wire_prepared` 与 rollback 不读取组件文件；磁盘变化只在 registry mutation 或显式 invalidation
后可见。见 [ADR 0012](../adr/0012-immutable-plugin-preparation.md)。

## API

```rust,no_run
use echo_agent::plugin::{InstallSource, PluginRegistry, PluginScope};

let mut registry = PluginRegistry::new(Some(std::env::current_dir()?));
registry.scan_all()?;

let id = registry.install(
    &InstallSource::Local("./review-tools".into()),
    PluginScope::Project,
)?;
registry.disable(&id)?;
registry.enable(&id)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

需要安装前报告时，使用 `PluginRegistry::validate_plugin_dir`。
`PluginScope` 实现了标准 `FromStr`，并支持文档化的简写（`u`、`p`、`l`）；调用方可以
统一使用 `scope.parse()` 解析配置和命令输入。

## 设计依据

本设计复用 Agent Plugins 1.0 官方的 [manifest](https://agent-plugins.org/plugin-authors/manifest)、[Skills](https://agent-plugins.org/plugin-authors/skills)、[MCP](https://agent-plugins.org/plugin-authors/mcp-servers) 和 [loading](https://agent-plugins.org/client-implementers/loading-and-discovery) 约定。embedding application 作为本地个人助理，额外组件有意采用固定根位置，不引入客户端扩展 namespace。
