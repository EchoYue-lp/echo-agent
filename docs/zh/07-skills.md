# Skill 系统

## 是什么

Skill（技能）是比 Tool 更高层次的能力单元。Echo Agent 提供两种 Skill 类型：

| 类型 | 注册方式 | 加载策略 |
|------|---------|---------|
| **Code-based** | `agent.add_skill(Box::new(MySkill))` | 立即加载（工具 + 提示词一次性注入） |
| **File-based** | `agent.discover_skills(scopes)` | 渐进式披露（目录 → 激活 → 资源） |

```
Tool:  单一原子操作（"读取文件"）
Skill: 领域能力包（"文件系统操作" = read_file + write_file + list_dir + 使用说明提示词）
```

框架契约（`Skill`、`SkillDocument` 与 `SkillRegistry`）位于 `echo-core` 和
`echo-execution`。`SkillDocument::parse` 与 `parse_at` 是运行时发现、产品目录和安装校验
共用的唯一解析/验证 API。产品目录只投影 typed descriptor，不自行解析 frontmatter。

---

## Skill vs Tool

| 维度 | Tool | Skill |
|------|------|-------|
| 粒度 | 单一操作 | 领域能力包 |
| 注册方式 | `agent.add_tool(box)` | `agent.add_skill(box)`（code-based）或 `discover_skills`（file-based） |
| 系统提示词 | 无 | 可携带提示词注入片段 / SKILL.md 正文 |
| 工具数量 | 1 个 | 多个 |
| 语义 | "做一件事" | "我掌握某个领域" |

---

## 内置 Code-based Skill

框架内仅有两个 `Skill` trait 实现，皆受 feature 门控：

| Skill | feature | 包含工具 | 描述 |
|-------|---------|---------|------|
| `FileSystemSkill` | `files` | `read_file`、`write_file`、`list_dir` | 文件系统操作 |
| `ShellSkill` | `shell` | `shell` | Shell 命令执行 |

```rust,no_run
use echo_agent::prelude::*;

# fn demo() -> echo_agent::error::Result<()> {
let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("你是一个有帮助的助手")
    .build()?;

#[cfg(feature = "files")]
agent.add_skill(Box::new(FileSystemSkill));
#[cfg(feature = "shell")]
agent.add_skill(Box::new(ShellSkill));
# Ok(())
# }
```

> **提醒**：此前文档曾列出 `CalculatorSkill` 与 `WeatherSkill`，它们已经移除；`FileSystemSkill` 和 `ShellSkill` 是仓库内的 code-based 示例。应用可以通过发现机制加入自己的 file-based skill。

---

## 自定义 Code-based Skill

实现 `Skill` trait：

```rust,no_run
use echo_agent::skills::Skill;
use echo_agent::tools::Tool;

struct ResearchSkill;

impl Skill for ResearchSkill {
    fn name(&self) -> &str { "research" }
    fn description(&self) -> &str { "网页调研：搜索 + 摘要" }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![Box::new(SearchTool), Box::new(SummarizeTool)]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        Some("当需要网络信息时，先用 web_search，再用 summarize 整理结果。".to_string())
    }
}

# fn demo(agent: &mut echo_agent::ReactAgent) {
agent.add_skill(Box::new(ResearchSkill));
# }
```

---

## File-based Skill（渐进式披露）

对齐 [agentskills.io 规范](https://agentskills.io/specification)。Skill 从文件系统加载 —— 扩展 Agent 能力 **无需改代码**。

### 三级渐进披露模型

核心设计原则：不一次性加载所有内容，而是按需逐层显示，保持上下文窗口紧凑。

| 层级 | 内容 | 触发条件 | Token 成本 |
|------|------|---------|----------|
| **Tier 1: 目录** | name + description（frontmatter） | 启动时自动扫描 | ~50-100 / skill |
| **Tier 2: 激活** | 完整指令 + 资源清单 | LLM 调用 `activate_skill`（或 IntentRouter 分类） | <5000 / skill |
| **Tier 3a: 资源** | 引用文件内容 | LLM 调用 `read_skill_resource` | 按需 |
| **Tier 3b: 脚本** | Python/Bash/TS/PowerShell 脚本执行 | LLM 调用 `run_skill_script` | 按需 |

### SKILL.md 文件格式（agentskills.io 标准）

```markdown
---
name: code-review
description: >-
  专业代码审查技能：识别代码缺陷、安全风险和最佳实践违规。
  当用户要求审查代码质量、review 代码或查找 bug 时使用。
license: Apache-2.0
allowed-tools: read_skill_resource run_skill_script Bash
metadata:
  team: backend
---

## 代码审查

当被要求审查代码时：

1. 加载检查清单：`read_skill_resource("code-review", "references/checklist.md")`
2. 逐项分析代码
3. 输出结构化的审查结果

当前环境: !`uname -s`
Skill 目录: ${SKILL_DIR}
```

agentskills.io 官方 Skill 文件格式不包含 per-skill Hook 字段或 sidecar。
请在宿主应用的 Hook 配置或 Plugin Hook component 中配置 Hooks，见
[Hooks 系统](./23-hooks.md)。

### Frontmatter 字段（当前）

定义在 `echo-agent/echo-execution/src/skills/external/types.rs`。

| 字段 | 必需 | 说明 |
|------|------|------|
| `name` | 是 | 唯一名称，kebab-case，1-64 字符，必须等于 Skill 目录名 |
| `description` | 是 | 描述，最多 1024 字符，写清"做什么"与"何时使用"——路由由这段文本驱动 |
| `license` | | SPDX 许可证标识或随附 license 文件引用 |
| `compatibility` | | 自由格式环境要求（OS、二进制、网络等），最多 500 字符 |
| `metadata` | | 任意 字符串 → 字符串 键值对 |
| `allowed-tools` | | 空格分隔的已注册工具白名单 —— **不**是要注册的工具列表 |

### 单一格式权威

`SKILL.md` 只有一种内容模型：frontmatter 描述目录条目，Markdown 正文是指令唯一来源；
支持文件与它放在同一 Skill 目录中，通常位于 `references/`、`scripts/` 或 `assets/`。

只接受上表的官方 agentskills.io 字段。未知顶层字段一律拒绝，避免拼写错误静默改变
运行时行为；echo-agent 旧字段（`version`、`author`、`tags`、`instructions`、
`resources`，以及旧顶层扩展 `triggers`、`hooks`、`shell`、`paths`、`sandbox`、
`depends_on`）会直接解析失败：字符串扩展元数据放进 `metadata`，指令写在结束 `---`
之后，支持文件放进 Skill 目录。Hooks 属于宿主配置，不是 Skill 文件内容。路由是 description-driven——
把"何时使用"的场景与关键词写进 `description`。参见
[ADR 0023](../adr/0023-current-skill-frontmatter.md) 与
[ADR 0026](../adr/0026-official-skill-frontmatter-only.md)。

用进程内的 `skills-ref validate` 等价物校验 Skill：

```rust,no_run
use echo_agent::skills::external::validate_skill_dir;

# fn inspect(dir: &std::path::Path) {
let report = validate_skill_dir(dir);
assert!(report.is_valid(), "{:?}", report.violations);
# }
```

### 内联命令执行

激活时，Markdown 正文中的命令会被执行并替换为输出：

```markdown
当前主机: !`uname -s`
```
→ 激活后：`当前主机: Darwin`

代码块命令：

````markdown
```!
rustc --version
```
````
→ 激活后：`rustc 1.93.0 (254b59607 2026-01-19)`

**安全策略**：MCP 来源的 skill **绝不执行**内联命令（不可信远程内容）。

当内联命令或 hook 命令回退到直接进程派生（未配置 `SandboxManager`）时，运行时会：
- 清空继承环境变量后再应用最小白名单（`PATH`、`SKILL_DIR`、`SESSION_ID`）
- 使用 `kill_on_drop(true)` 实现尽力超时杀进程

此回退适合本地开发与 demo，生产环境仍建议配置 `SandboxManager`。

### 变量替换

| 变量 | 值 |
|------|---|
| `${SKILL_DIR}` / `${CLAUDE_SKILL_DIR}` | Skill 所在目录绝对路径 |
| `${SESSION_ID}` / `${CLAUDE_SESSION_ID}` | 当前 Session ID |
| `${ARGUMENTS}` | 全部参数（空格连接） |
| `${1}`、`${2}`、... | 位置参数 |

### 目录结构

```
skills/
├── code-review/
│   ├── SKILL.md              ← skill 定义
│   ├── scripts/
│   │   └── lint.sh           ← 可执行脚本
│   └── references/
│       ├── checklist.md      ← 引用文档
│       └── style_guide.md
└── project-stats/
    ├── SKILL.md
    ├── scripts/
    │   ├── count_lines.py    ← Python 脚本
    │   ├── find_todos.sh     ← Bash 脚本
    │   └── dep_summary.ts    ← TypeScript 脚本
    └── references/
        └── metrics_guide.md
```

### 发现与加载

```rust,no_run
use echo_agent::prelude::*;

# async fn demo(agent: &mut ReactAgent) -> echo_agent::error::Result<()> {
// 方式一：自动发现（项目级 + 用户级）
let skills = agent.discover_skills(&[
    DiscoveryScope::Project(".".into()),  // ./skills/ + ./.agents/skills/
    DiscoveryScope::User,                 // ~/.agents/skills/
]).await?;

// 方式二：指定目录（向后兼容）
let skills = agent.load_skills_from_dir("./skills").await?;
# Ok(())
# }
```

发现完成后，`ReactAgent::discover_skills` 会自动注册三个渐进披露工具：

| 工具 | 用途 |
|------|------|
| `activate_skill` | 加载完整指令 + 资源列表（支持 `arguments` 参数） |
| `read_skill_resource` | 读取引用文件 |
| `run_skill_script` | 执行 Python/Bash/TS/PowerShell 脚本 |

后续若再次调用 `discover_skills()` 加入新 skill，这三个工具会通过 `replace_tool` 刷新内部共享注册表，使可用 skill 视图始终与最新发现结果一致。

### 依赖与循环检测

descriptor 声明 `depends_on`（程序化字段——标准 frontmatter 没有来源）时，
`SkillRegistry` 会在被请求 skill 之前递归激活每个依赖。`SkillLoader` 在加载阶段
通过 DFS 检测依赖循环并产生警告；重复项被去重，最后选取一条无环的激活顺序。

---

## 单一激活投影

两种激活入口使用同一份 wrapped skill 内容，也使用同一个受保护上下文投影权威。

LLM 可以调用 `activate_skill` 工具；应用代码和 `IntentRouter` 可以调用 `ReactAgent::activate_skill`。`ActivateContent::to_prompt_block` 会把激活内容包裹为以下 XML 信封：

```
<skill_content name="paper-search">
{instructions}

Skill directory: ...
<allowed_tools> ... </allowed_tools>
<skill_resources>
  <file kind="reference">references/...</file>
</skill_resources>
</skill_content>
```

`ReactAgent::activate_skill` 用 `ContextManager::replace_projection` 把该块写到精确 marker `echo-agent:skill:<name>`。`activate_skill` 工具返回 typed activation fact，ReAct 工具阶段再把其内容投影到同一 marker。重复激活会替换旧投影，不会累积第二份权威。上下文压缩会跳过 projection，并在压缩后重新插回。

### triggers 来自哪里

标准 frontmatter 没有 trigger 字段，因此文件型 skill 的
`SkillDescriptor.triggers` 为空。消费方从 `description` 文本派生关键词路由
（description-driven，规范自身的建议），或在程序化注册的 descriptor 上填充
`triggers`。如果某 skill 没有 trigger，关键词路由无法选择它；显式 API 激活与
LLM 工具路径仍然可用。

---

## Hooks 系统

Skill 与用户和插件 Hook 文件使用同一套 31 事件系统，覆盖工具、会话、Subagent、Task、
插件和自演化生命周期，并不局限于成功的工具调用。

### Hook 事件

| 类别 | 事件 |
|------|------|
| 工具（5） | `PreToolUse`、`PostToolUse`、`PostToolUseFailure`、`PermissionRequest`、`PermissionDenied` |
| 会话/运行（11） | `SessionStart`、`SessionEnd`、`Stop`、`StopFailure`、`Notification`、`UserPromptSubmit`、`PreCompact`、`PostCompact`、`ConfigChange`、`InstructionsLoaded`、`PostToolBatch` |
| Subagent（2） | `SubagentStart`、`SubagentStop` |
| Task（3） | `TaskCreated`、`TaskStarted`、`TaskCompleted` |
| Plugin（2） | `PluginLoaded`、`PluginDisabled` |
| Evolution（8） | `PostMemoryWrite`、`MemoryLayerChange`、`SkillCandidateDetected`、`SkillLifecycleTransition`、`SkillHealthCheck`、`SkillPatchApplied`、`SkillMergeApplied`、`RulePromoted` |

各事件的触发点和 matcher 语义以 [Hooks 系统](./23-hooks.md) 为准。

### Hook 类型

| 类型 | 行为 |
|------|------|
| `command` | 执行 shell 命令；stdin 接收 JSON 上下文，stdout 返回 JSON 控制指令 |
| `prompt` | 注入提示消息给 LLM |
| `permission` | 直接返回 `allow`、`deny` 或 `ask` |
| `http` | POST 事件上下文并解析响应 |
| `mcp_tool` | 调用用户所配置 MCP 服务器暴露的工具 |
| `agent` | 派发指定 Subagent |
| `activate_skill` | 不经额外 LLM 往返直接激活已发现 Skill |

### 命令 Hook 输入（stdin JSON）

```json
{
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": {"command": "git status"},
  "tool_output": null
}
```

### 命令 Hook 输出（stdout JSON）

```json
{
  "decision": "block",
  "reason": "检测到不安全命令",
  "updatedInput": {"command": "git status --short"},
  "injected_context": "使用规范化后的命令",
  "permission_mode_override": "auto",
  "continue": false
}
```

| 字段 | 说明 |
|------|------|
| `decision` | `"allow"` 继续 / `"block"` 阻止 |
| `reason` | 阻止原因 |
| `updatedInput` | 修改后的工具输入（仅 PreToolUse） |
| `injected_context` | 注入当前运行的上下文 |
| `permission_mode_override` | 仅作用于当前工具调用的权限模式覆盖 |
| `continue` | `false` 停止执行后续 hooks |

这些规范 wire 字段名区分大小写；`modified_input`、`message` 和 `permission_mode` 不是别名。

若多个匹配 hook 都返回 `permission_mode_override`，运行时仅保留最后一个非空覆盖值。权限决策本身仍按更严格的优先级处理：`deny > ask > allow`。

插件拥有的 Skill 会在解析 frontmatter 前，对完整 `SKILL.md` 应用 `PluginVariables`
替换。因此 `${ECHO_PLUGIN_ROOT}`、`${ECHO_PLUGIN_DATA}`、`${ECHO_PROJECT_DIR}`、
`${user_config.KEY}` 及支持的环境变量占位符，在 Skill metadata 与正文中均生效；Plugin
Hook Action 属于 Plugin Hook component，不是 Skill 文件内容。

### Matcher 规则

- `"*"` —— 匹配所有工具
- `"Bash"` —— 精确匹配
- `"Bash"` 也匹配 `"Bash(git:*)"` 等带括号变体

---

## 路径条件激活

带 `paths`（程序化字段——标准 frontmatter 没有来源）的 descriptor 始终出现在目录中，
但运行时激活由匹配的 `context_path` 把守：

```rust
# // 在程序化注册的 descriptor 上填充
# descriptor.paths = vec!["*.py".to_string(), "tests/**".to_string()];
```

目录显示为：`- python-linter: ... [activates for: *.py, tests/**]`

激活时调用：

```json
{
  "name": "python-linter",
  "context_path": "tests/test_api.py"
}
```

如果未提供 `context_path` 或与声明的 glob 不匹配，`activate_skill` 直接报错而非加载。

---

## allowed-tools 白名单

`allowed-tools` **不**注册工具 —— 它是用来过滤工具调用的白名单，对所有已激活 skill 的白名单取并集（`registry.rs:178-199`）。官方 wire 格式是一个空格分隔的 plain 字符串：

```yaml
allowed-tools: read_skill_resource run_skill_script Bash(git:*)
```

如果值包含 YAML alias 指示符（例如 `*`），请使用引号（如 `allowed-tools: "*"`）。
程序化 descriptor 仍可设置任意 matcher 列表，但标准格式文件与 validator 要求空格分隔字符串。

匹配语义（`types.rs:277-307`）：
- 精确名（`"read_skill_resource"`）
- 通配符 `"*"`（允许所有）
- 前缀-括号（`"Bash"` 匹配 `"Bash(git:status)"`）
- glob via `glob::Pattern`（`"Bash(git:*)"`）

内置 `read_skill_resource` 与 `run_skill_script` 工具在调用时同样校验该白名单，不在已激活 skill 白名单内的调用会被拒绝。

---

## 跨平台脚本执行

`run_skill_script` 自动选择正确的解释器：

| 扩展名 | Unix | Windows |
|--------|------|---------|
| `.py` | `python3` | `python` / `py -3` |
| `.js` | `node` | `node` |
| `.ts` | `bun` → `deno` → `npx tsx` | 同左 |
| `.sh` | `bash` | Git Bash → PowerShell 兜底 |
| `.ps1` | `pwsh` | `powershell` |
| `.rb` | `ruby` | `ruby` |

解释器直接调用（不通过 `sh -c` / `cmd /C`），避免 shell 注入。

运行时附加保证：
- `script` 路径必须为相对路径，且 canonicalize 后落在已激活 skill 目录内
- 畸形 `args` 字符串会被拒绝，而不是当作单个不透明参数

---

## 上下文保护

已激活 skill 指令存放在命名上下文投影中。`activate_skill` 工具与直接 `ReactAgent::activate_skill` 路径都使用 `echo-agent:skill:<name>`，因此 wrapped block 能在压缩后保留，重复激活则替换旧投影。

```rust,ignore
ctx.replace_projection(
    "echo-agent:skill:code-review",
    Some(Message::system(block)),
);
```

---

## 框架 Registry 与产品 Catalogue

`SkillRegistry` 负责可复用运行时生命周期：发现、激活、依赖排序、资源访问和沙箱策略；它不负责安装或卸载市场包。

应用可以另加产品目录。embedding application 当前的 `SkillsHub` 扫描 `<application-data>/skills/`，服务其 UI 与安装流程，但不会取代框架 registry。产品细节可能独立于 echo-agent 演化，应以 [embedding application SkillsHub 源码](https://github.com/EchoYue-lp/echo-agent-cli/tree/main/echo-agent-app-core/src/skills_hub) 为准。

---

## Skill 遥测（Telemetry）

公开的 `echo_agent::skill_telemetry` 模块定义 `SkillExecutionRecord`、`SkillTelemetry` 和
`SkillTelemetryStore`，由 `Store` trait 在命名空间 `["agent", "skill_telemetry"]` 下背书。
调用方应使用这个 facade 路径，不要依赖拆分后的 `echo-state` crate。

统一的 tool execution 主路径会在每次 tool 结果产生后，为当前已激活的每个 skill 记录一条
observation。framework 使用进程级共享锁串行化 read-modify-write；telemetry 缺少 store 或写入失败时只记录
warning，不改变 tool 的成功、失败或 retry 语义。只有 host 显式注入 `Curator` 时才会刷新其
活跃时间。

---

## 查询已安装的 Skill

```rust,no_run
use echo_agent::prelude::*;

# fn demo(agent: &echo_agent::ReactAgent) {
// 列出所有已安装 Skill
for info in agent.list_skills() {
    println!("- {} ({} 个工具)", info.name, info.tool_names.len());
}

// 检查某 Skill 是否已安装
if agent.has_skill("paper-search") {
    println!("paper-search skill 已安装");
}

// 总数
println!("已安装 {} 个 skill", agent.skill_count());
# }
```

---

## 已删除组件：SkillGateway

`SkillGateway`（早期产品层的 skill 路由器）已被移除，职责拆给：

- `IntentRouter` + `KeywordClassifier` —— 关键词 / 语义路由（框架）
- `SkillRegistry` —— 激活与生命周期（框架）
- `SkillsHub` —— 用户可见的目录 UI（产品）

如果下游的 eval harness 或文档仍引用 `SkillGateway`，按历史遗留处理。

---

## 示例

参见示例文件：
- `echo-agent-learning/examples/demo07_skills.rs` —— Code-based skill 演示
- `echo-agent-learning/examples/demo08_external_skills.rs` —— File-based skill 完整功能演示（渐进披露 + 脚本执行 + 内联命令 + hooks）
