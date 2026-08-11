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

框架那一半（`Skill` trait + `SkillRegistry`）位于 `echo-agent/echo-core/` 与 `echo-agent/echo-execution/`。产品那一半（用户可见的技能**目录 / 市场**，位于 `~/.echo-agent/skills/`）实现在 `echo-agent-cli/echo-agent-app-core/src/skills_hub/`，与运行时注册表正交 —— 详见下文 [SkillsHub vs SkillRegistry](#skillshub-vs-skillregistry)。

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

> **提醒**：此前文档曾列出 `CalculatorSkill` 与 `WeatherSkill`，它们已经移除。现在仅 `FileSystemSkill` 和 `ShellSkill` 作为框架内置的 code-based skill 存在。领域型能力（网页搜索、论文检索、编码等）改为 **file-based skill** 形式发行在 `echo-agent-cli/skills/` —— 见后文 [内置 File-based Skill](#内置-file-based-skill)。

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
  当用户要求审查代码质量时使用。
license: Apache-2.0
shell: bash
paths:
  - "*.rs"
  - "*.py"
triggers:
  - 帮我 review 代码
  - 找一下 bug
allowed-tools:
  - read_skill_resource
  - run_skill_script
  - Bash
depends_on:
  - coding
metadata:
  team: backend
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: prompt
          prompt: "执行前请先验证命令安全性"
  PostToolUse:
    - matcher: "*"
      hooks:
        - type: command
          command: "${SKILL_DIR}/scripts/log_usage.sh"
          timeout: 5
sandbox:
  isolation: process
  network: deny
  allowed_paths:
    - "${SKILL_DIR}"
---

## 代码审查

当被要求审查代码时：

1. 加载检查清单：`read_skill_resource("code-review", "references/checklist.md")`
2. 逐项分析代码
3. 输出结构化的审查结果

当前环境: !`uname -s`
Skill 目录: ${SKILL_DIR}
```

### Frontmatter 字段（当前）

定义在 `echo-agent/echo-execution/src/skills/external/types.rs`（`SkillDescriptor` 第 75 行 + `RawFrontmatter` 第 415 行）。

| 字段 | 必需 | 说明 |
|------|------|------|
| `name` | 是 | 唯一名称，kebab-case，1-64 字符 |
| `description` | 是 | 描述，最多 1024 字符，说明何时使用 |
| `license` | | SPDX 许可证标识 |
| `compatibility` | | 自由格式兼容性说明（echo-agent 版本、OS 等） |
| `shell` | | 内联命令的 shell：`bash`（默认）或 `powershell` |
| `paths` | | 条件激活的 glob 模式（如 `["*.py"]`），同时进入目录 |
| `triggers` | | 用户语句触发词，由 `KeywordClassifier` 消费（参见 [两条激活路径](#两条-skill-激活路径)） |
| `allowed-tools`（别名 `allowed_tools`） | | 已注册工具的白名单 —— **不**是要注册的工具列表 |
| `depends_on` | | 自动先行激活的其他 skill；DFS 检测循环并 warn（`loader.rs:387-446`） |
| `hooks` | | 31 个主 Hook 事件中的任意事件规则；见 [Hooks 系统](./23-hooks.md) |
| `sandbox` | | 单 skill 沙箱策略：`isolation`、`network`、`allowed_paths`、`denied_paths`、`timeout` |
| `metadata` | | 任意键值对 |

### Frontmatter 字段（Legacy）

下列字段仍能被解析，但已废弃；激活使用它们的 skill 时会在加载阶段输出弃用警告（`loader.rs:279-285`），未来版本会移除。

| Legacy 字段 | 替代方案 |
|------------|---------|
| `version` / `author` / `tags` | 移入 `metadata:` |
| `instructions` | 直接放在 `---` 之后的 Markdown 正文 |
| `resources` | 自动枚举 `references/`、`scripts/`、`assets/`，无需声明 |

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

发现完成后，框架会自动注册三个渐进披露工具（`capabilities.rs:577-587`）：

| 工具 | 用途 |
|------|------|
| `activate_skill` | 加载完整指令 + 资源列表（支持 `arguments` 参数） |
| `read_skill_resource` | 读取引用文件 |
| `run_skill_script` | 执行 Python/Bash/TS/PowerShell 脚本 |

后续若再次调用 `discover_skills()` 加入新 skill，这三个工具会通过 `replace_tool` 刷新内部共享注册表，使可用 skill 视图始终与最新发现结果一致。

### 依赖与循环检测

skill 声明 `depends_on` 时，每个依赖会在被请求 skill 之前递归激活（`registry.rs:236-325`）。加载阶段通过 DFS 检测依赖循环并产生警告（`loader.rs:387-446`）；loader 不会阻塞 —— 重复项被去重，最后选取一条无环的激活顺序。

---

## 两条 Skill 激活路径

> ⚠️ **重要：两条路径产出消息类型略有不同，且其中一条路径 *不受* 压缩保护。**

### 路径 1：LLM 通过 `activate_skill` 工具激活

LLM 调用 `activate_skill` 工具。工具成功结果由 `ActivateContent::to_prompt_block`（`echo-execution/src/skills/external/types.rs:334-372`）包裹为 XML 信封：

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

该块作为 `Role::Tool` 消息进入消息流，包含子串 `"<skill_content"`，已在 `ContextManager`（`agent/react/capabilities.rs:589-597`）注册为**保护 marker**。压缩流程通过 `is_protected`（`compression/mod.rs:456-465`）将其跳过，并在压缩后由 `merge_protected` 重新插回原位。

### 路径 2：IntentRouter 预分类

`KeywordClassifier` / `LlmIntentClassifier` 返回 `Intent::SkillRequired` 时，`react_loop.rs:738-764` 激活 skill 并把 **`content.instructions` 原文作为 `Message::system` 推入**：

```rust
self.memory.context.lock().await
    .push(Message::system(content.instructions));
```

它**不**调用 `to_prompt_block()`，所以这条消息没有 `<skill_content` 子串。⚠️ **由 IntentRouter 注入的 skill 内容因而*不受*压缩保护** —— 滑动窗口或摘要压缩可能把它丢掉。该分歧记录在 [07-cross-cutting.md §3](../../../echo-agent-cli/docs/system-deep-dive/07-cross-cutting.md)。

### triggers 来自哪里

`KeywordClassifier` 在运行时从每个 `SkillDescriptor.triggers` 字段填充（`echo-agent-cli/echo-agent-app-core/src/runtime.rs:196-255`）。如果某 skill 的 `triggers` 为空，IntentRouter 永远不会为它触发 —— 此时只能走 LLM 工具路径。

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
`${user_config.KEY}` 及支持的环境变量占位符，在 Skill metadata、正文和 frontmatter
Hook Action 中均生效。

### Matcher 规则

- `"*"` —— 匹配所有工具
- `"Bash"` —— 精确匹配
- `"Bash"` 也匹配 `"Bash(git:*)"` 等带括号变体

---

## 路径条件激活

带 `paths` 的 skill 始终出现在目录中，但运行时激活由匹配的 `context_path` 把守：

```yaml
paths:
  - "*.py"
  - "tests/**"
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

`allowed-tools` **不**注册工具 —— 它是用来过滤工具调用的白名单，对所有已激活 skill 的白名单取并集（`registry.rs:178-199`）：

```yaml
allowed-tools:
  - read_skill_resource
  - run_skill_script
  - Bash
  - "Bash(git:*)"
  - "*"           # 通配
```

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

通过 **`activate_skill` 工具路径**送入的已激活 skill 指令受压缩保护 —— 上下文超过 token 上限时，该 wrapped 块在压缩中存活。

```rust,ignore
// 内部机制：含 "<skill_content" 子串的消息被排除在压缩外
ctx.add_protected_marker("<skill_content".to_string());
```

⚠️ 该保护通过 `try_lock` 注册（`agent/react/capabilities.rs:589-597`）；安装时若锁被争用，marker 注册会静默跳过，后续 skill 激活就可能被压缩掉。**通过 IntentRouter 路径**送入的 skill 内容则完全无保护 —— 见 [两条激活路径](#两条-skill-激活路径)。

---

## SkillsHub vs SkillRegistry

这是**两个完全不同的概念**，并存于代码库中。请勿混淆。

| 关注点 | `SkillRegistry`（框架） | `SkillsHub`（产品） |
|--------|------------------------|--------------------|
| crate | `echo-agent/echo-execution/src/skills/registry.rs` | `echo-agent-cli/echo-agent-app-core/src/skills_hub/` |
| 用途 | Agent 实例的运行时生命周期：发现 → 目录 → 激活 → 资源 | 本地技能**目录 / 市场** UI：浏览、安装、卸载到磁盘 |
| 默认扫描路径 | `<project>/skills/`、`<project>/.agents/skills/`、`~/.agents/skills/` | 仅 `~/.echo-agent/skills/` |
| Frontmatter 解析 | 完整 `serde_yaml_ng`（`loader.rs`） | 手写 mini key-value 解析器（无完整 YAML） |
| 状态 | 已激活集合、沙箱策略、code-based skill、session_id | 仅本地已安装 skill 元数据列表 |
| 谁在用 | 每一轮 `ReactAgent` 都会调用 | CLI `/skills` 命令（list / search / install / uninstall） |
| 安装/卸载 | 不负责安装；只负责运行时发现 | `git clone https-only` + 本地拷贝 + uninstall |

`SkillsHub` 不会把 skill 装载进 agent。`echo-agent-cli/skills/` 下的内置 skill 由框架的 `discover_skills` 路径在启动时载入（`echo-agent-cli/echo-agent-app-core/src/runtime.rs:133-153`），与 hub 无关。

---

## 内置 File-based Skill

11 个 file-based skill 发布在 `echo-agent-cli/skills/`，由上文的发现路径在启动时载入：

| Skill | 一句话角色 |
|-------|----------|
| `coding` | 编程与软件工程：编写/调试/重构/审查代码 |
| `data-visualization` | 图表与可视化（柱状图/折线图/仪表盘） |
| `data-wrangling` | CSV/Excel/JSON 加载、清洗、EDA |
| `doc-writing` | 报告、技术文档、文章、邮件、提案 |
| `evidence-medicine` | 医学文献检索 + 循证分析（PubMed、ClinicalTrials、PICO、GRADE） |
| `git-workflow` | Git 操作（分支、提交、PR/MR、冲突） |
| `paper-reader` | 单篇论文的深度阅读与批判性评估 |
| `paper-search` | 跨 ArXiv + Semantic Scholar 的学术论文搜索（非医学） |
| `statistical-analysis` | 统计检验（t/χ²/ANOVA）、回归、建模 |
| `translation` | 翻译、校对、本地化 |
| `web-search` | 网络信息检索 + 多源交叉验证 |

每个均带 `SKILL.md` + 可选 `references/` 子目录。它们与 `SkillsHub` 独立管理。

---

## Skill 遥测（Telemetry）

独立模块 `echo-state/src/skill_telemetry.rs` 定义了 `SkillExecutionRecord` 和 `SkillTelemetry` 类型，由 `Store` trait 在命名空间 `["agent", "skill_telemetry"]` 下背书。CLI `evolution` 命令读取这些记录（`echo-agent-cli/src/cli/cmd_impls/evolution.rs:7,344`）。

⚠️ **运行时目前并未向该 store 写入。** 至今没有任何 `record_execution` 调用点存在于 agent runtime；schema 已就位，但生产者侧尚未接入激活路径。

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
- `examples/demo07_skills.rs` —— Code-based skill 演示
- `examples/demo08_external_skills.rs` —— File-based skill 完整功能演示（渐进披露 + 脚本执行 + 内联命令 + hooks）
