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

### MCP 所有权与投影

`name` 仍是可移植配置中的本地 server 名称。运行时框架会注入所有者：普通配置使用
`McpServerOwner::Direct`，插件 wiring 使用稳定的 `PreparedPlugin.id`。因此两个插件都声明
`filesystem` 时仍会拥有不同连接、工具和清理债务。需要区分所有者时使用类型化的
`McpServerId`/`server_ids()`；旧字符串 API 继续指向 Direct server。

Direct 工具保持 `mcp__<server>__<tool>`。插件工具使用
`mcp__plugin_<plugin>_<server>__<tool>`；规范 Unicode 或标点被有损归一化时追加稳定摘要。
资源 selector 使用不透明的 `plugin:<base64url-plugin>:<base64url-server>`。资源目录保存
类型化 identity，不从工具名或 URI 反推 owner。
Direct 名称通常保持旧 selector；以保留的 `plugin:`/`direct:` 开头时，会可逆转义为
`direct:<base64url-name>`。插件 Hook 的 `mcp_tool.server` 在包内仍写本地名称，注册前由
Integrator 注入 prepared plugin owner。

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

## 冻结的构建能力诊断

当前 build 未启用 `lsp` 时，成功冻结的 `lsp.yaml` 会产生通用 preparation Warning，携带 `component() == "lsp"`、插件身份和来源路径。这只说明构建能力边界，不是应用退役政策，也不证明文本是有效 LSP YAML；变量替换、冻结文本和原有 hash 仍由 prepare 拥有。

Invalid UTF-8 与读失败保留 Error。错误文件种类、metadata 失败、失效/循环链接也产生带 LSP 组件与路径的结构化诊断；真实缺失仍是合法 optional 状态。非普通文件只分类、不读取，避免将 FIFO 作为文本打开。组件 Error 不丢弃健康 Skill/Hook/MCP，也不使整代准备结果失效。

应用须读取完整 `PreparedPluginSet::diagnostics()`，不能只消费 apply receipt 的 Warning 投影。诊断随同一 identity/generation 冻结；cache hit 返回同一 Arc，不重新探测或追加重复项，文件变化只在 invalidate 后重捕获。wire/rollback 仍不读取包文件。

启用 `lsp` 时保持原行为。此中间桥接不新增 public API、不物理删除框架 LSP，也不改 SDK accepted/pin。应用可将通用组件诊断映射为自己的产品说明，无需调用 LSP 专属文档 accessor 或重新读取文件。见 [ADR 0090](../adr/0090-a2a-lsp-retirement-contract.md)。

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

插件按依赖顺序加载。清单致命错误会跳过整个包；framework解析的Skill、Hook、MCP错误按组件边界隔离并记录error diagnostic，无法读取的冻结Subagent/LSP文档也按相同方式排除。二者的产品级语法由embedding application的第二阶段prepare校验。同一插件中健康的兄弟组件仍保留在prepared plugin中。运行时记录每个插件的组件所有权，因此disable、uninstall和reload能精确卸载对应组件。

`PluginIntegrator::prepare` 捕获唯一不可变的 `PreparedPluginSet`，包括单调 generation、确定性
内容 identity、结构化诊断、已解析的 Skills/Hooks/MCP，以及保留 owner 的 Subagent/LSP 文档。
`wire_prepared` 与 rollback 不读取组件文件；磁盘变化只在 registry mutation 或显式 invalidation
后可见。见 [ADR 0012](../adr/0012-immutable-plugin-preparation.md)。
generation 序号在同一进程的所有 Integrator 间统一分配，因此两个独立 Integrator 为同一 Agent
准备的新旧快照仍有可比较的顺序。

### 选择性 prepared 内容身份

`PreparedPlugin::content_identity() -> &str` 返回该插件同次 prepare 输入的 SHA-256，覆盖
manifest（真实可选 version 与依赖）、有效配置/变量、已发现 Skill 身份文档、Hook/MCP
声明及冻结的 Subagent 文档。对象键和 manifest 依赖顺序规范化，相对来源路径与内容共同
参与摘要。同输入重准备或进程内 generation 变化不改变内容身份；未声明 version 继续保持
None，摘要不冒充版本号或 latest。

`PreparedPluginSet::dependency_closure(id) -> Result<Vec<&PreparedPlugin>, String>` 从本次
不可变且适用的 Set 选择已启用的根及传递依赖，按稳定的依赖优先顺序返回。缺失/禁用依赖、
不满足或非法版本条件、环仍由原 registry 在 prepare 时检查；Set 不适用或根未准备时明确
返回错误。无关插件改变不改变所选成员的内容身份，必要依赖改变会改变对应成员身份。
读取旧 Set 的闭包不授予发布旧代或继续持有外部资源的资格。

完整 `PreparedPluginSet::identity()` 继续为整批 publication 提供聚合身份，不能代替某任务
选择的闭包。本次编码改为聚合规范化的单插件身份，不承诺历史摘要字符串跨版本兼容；
generation、publication target、rollback receipt、取消和清理 owner 的职责均不改变。

摘要只覆盖框架实际冻结的输入，不是完整包归档：已发现 SKILL.md 身份文档不等于整个
scripts/references 目录；Hook 命令、MCP 连接声明不固定外部实现或服务。产品声明与显式输入
仍由 embedding application 承接。见 [ADR 0096](../adr/0096-selective-prepared-plugin-identity.md)。
既有 `demo56_plugin_system` 已在实际 prepared 产物上选择闭包，再以同一 integrator 进入
coordinator publication，继续纳入 workspace examples 的编译/测试链，不新增演示目标或依赖。

每个 `ReactAgent` 仍拥有唯一 publication target，但 Host 通过 `PluginCoordinator` 驱动完整
生命周期。用 durable registry 与 integrator 构造 coordinator 后，调用
`coordinator.reconcile(&mut agent)` 收敛当前意图，或使用 `enable`、`reload`、`disable`、
`uninstall`、`shutdown` typed operation。失败返回 `ActualPending`；必须先调用
`coordinator.retry(&mut agent)` 从原 operation receipt 与 phase 继续，再开始后续 operation。
Coordinator 只串行化 transition；target 仍是 generation、publication receipt 与 cleanup debt
的唯一权威。

Registry dependency graph 决定 transition 顺序：dependency 先 init/activate 并发出
`PluginLoaded`，dependent 先 deactivate 并发出 `PluginDisabled`。Converged receipt 只对同一
Agent publication target 有效；收敛后新增 lifecycle callback 会使 no-op 快径失效，下一次
reconcile 必须初始化并激活它。错误 Agent 会在 registry intent 或 callback 改变前被拒绝。

Registry 意图先于 runtime 收敛提交。实际顺序固定为 callback cleanup、精确 receipt withdrawal、
immutable generation publication、callback activation，最后才尝试 lifecycle Hook 通知。
dependency resolution 与 generation-wide applicability validation 会先于 callback cleanup；
无效输入保留旧 actual generation，retry 在同一 operation receipt 下重新读取修复后的插件文件。
Shutdown 只撤销进程内 effect，不改变 durable enabled intent。`PluginLoaded` 与
`PluginDisabled` 在单个 operation 内有序且去重，但不是跨进程 durable event log；取消或崩溃
仍可能丢失通知，该缺口继续属于更广泛的 Hook producer contract。
Registry refresh 只在完整 scan 成功后提交，并保留上一次成功的 scope 集合；受限 Host view
不会因 coordinator retry 被扩大。
未完成 receipt 始终投影为下一 retry phase 的 `ActualPending`，包括 transition future 被取消后。
每个成功的 MCP 连接在开始下一个 server 前立即进入 pending receipt。原本不存在的名字在
连接 await 前预留清理范围，覆盖 manager 已发布而 Agent 尚未返回时的取消；新连接失败须先
结算该名字才能发布 generation。这不代替 #75 独立处理的 MCP owner-qualified identity。

存在组件诊断时，整个 set 仍然可应用；只有依赖排序或 generation 分配等代次级不变量无法
构造完整不可变快照时，set 才会被拒绝。

`PluginLifecycleManager` 单独拥有 callback 清理状态。`deactivate`、`init` 或 `activate`
失败会留下 cleanup debt，并阻断 `reconcile`、`activate_enabled` 和直接 `activate` 的后续
callback 激活。失败注册项保留供重试；已激活 callback 成功撤销后只结清撤销债务，初始化或
激活失败则须成功执行 `unregister` 清理；初始化失败只需 shutdown，不调用尚未进入的
deactivate 阶段。期望 enabled 集合变化本身不表示旧资源已撤销。
`shutdown` 失败的债务不会被后续成功的 `deactivate` 清除，仍需通过 `unregister` 重试。见
[ADR 0060](../adr/0060-plugin-lifecycle-reconcile-settlement.md)。`PluginCoordinator` 将该
callback authority 与 durable registry intent、Agent-bound publication receipt 串联起来，
不复制 callback 或 generation 状态。见
[ADR 0069](../adr/0069-plugin-host-lifecycle-coordinator.md)。
init 或 activate 失败后，retry 先调用 `PluginLifecycleManager::reset_for_retry`，由原 authority
结算 callback 自身的 deactivate/shutdown debt，并保留同一 registration。

## API

```rust,no_run
use echo_agent::agent::ReactAgentBuilder;
use echo_agent::plugin::{
    InstallSource, PluginCoordinator, PluginIntegrator, PluginRegistry, PluginScope,
};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let root = std::env::current_dir()?;
let mut registry = PluginRegistry::new(root.join(".echo-agent"), Some(root));
registry.scan_all()?;
let id = registry.install(
    &InstallSource::Local("./review-tools".into()),
    PluginScope::Project,
)?;
let mut agent = ReactAgentBuilder::new().model("local-model").build()?;
let mut coordinator = PluginCoordinator::new(registry, PluginIntegrator::new());
coordinator.reconcile(&mut agent).await?;
coordinator.disable(&mut agent, &id).await?;
coordinator.enable(&mut agent, &id).await?;
# Ok(())
# }
```

需要安装前报告时，使用 `PluginRegistry::validate_plugin_dir`。
`PluginScope` 实现了标准 `FromStr`，并支持文档化的简写（`u`、`p`、`l`）；调用方可以
统一使用 `scope.parse()` 解析配置和命令输入。

## 设计依据

本设计复用 Agent Plugins 1.0 官方的 [manifest](https://agent-plugins.org/plugin-authors/manifest)、[Skills](https://agent-plugins.org/plugin-authors/skills)、[MCP](https://agent-plugins.org/plugin-authors/mcp-servers) 和 [loading](https://agent-plugins.org/client-implementers/loading-and-discovery) 约定。embedding application 作为本地个人助理，额外组件有意采用固定根位置，不引入客户端扩展 namespace。
