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

---

## Skill vs Tool

| 维度 | Tool | Skill |
|------|------|-------|
| 粒度 | 单一操作 | 领域能力包 |
| 注册方式 | `agent.add_tool(box)` | `agent.add_skill(box)` |
| 系统提示词 | 无 | 可携带提示词注入片段 |
| 工具数量 | 1 个 | 多个 |
| 语义 | "做一件事" | "我掌握某个领域" |

---

## 内置 Skill

| Skill | 包含工具 | 描述 |
|-------|---------|------|
| `CalculatorSkill` | add/subtract/multiply/divide | 数学计算 |
| `FileSystemSkill` | read_file/write_file/list_dir | 文件系统操作 |
| `ShellSkill` | shell | Shell 命令执行 |
| `WeatherSkill` | get_weather | 天气查询 |

```rust
use echo_agent::prelude::*;

let mut agent = ReactAgent::new(
    AgentConfig::new("qwen3-max", "assistant", "你是一个有帮助的助手")
        .enable_tool(true)
);

agent.add_skill(Box::new(CalculatorSkill));
agent.add_skill(Box::new(FileSystemSkill));

let answer = agent.execute("计算 42 * 8，并将结果写入 result.txt").await?;
```

---

## 自定义 Code-based Skill

实现 `Skill` trait：

```rust
use echo_agent::skills::Skill;
use echo_agent::tools::Tool;

struct ResearchSkill;

impl Skill for ResearchSkill {
    fn name(&self) -> &str { "research" }
    fn description(&self) -> &str { "网络研究：搜索 + 摘要" }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![Box::new(SearchTool), Box::new(SummarizeTool)]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        Some("当需要获取网络信息时，先用 web_search 搜索，再用 summarize 整理。".to_string())
    }
}

agent.add_skill(Box::new(ResearchSkill));
```

---

## 外部 Skill（渐进式披露）

对齐 [agentskills.io](https://agentskills.io/specification) 开放规范，从文件系统加载 Skill，**无需修改代码**即可扩展 Agent 能力。

### 三层渐进式披露模型

这是核心设计思想：不一次性加载所有内容，而是按需逐层展开，保持上下文窗口精简。

| 层级 | 内容 | 触发方式 | Token 开销 |
|------|------|---------|-----------|
| **Tier 1: 目录** | 名称 + 描述（frontmatter） | 启动时自动扫描 | ~50-100 / skill |
| **Tier 2: 激活** | 完整指引 + 资源列表 | LLM 调用 `activate_skill` | <5000 / skill |
| **Tier 3a: 资源** | 参考文件内容 | LLM 调用 `read_skill_resource` | 按需 |
| **Tier 3b: 脚本** | Python/Bash/TS/PowerShell 脚本执行 | LLM 调用 `run_skill_script` | 按需 |

### SKILL.md 格式（agentskills.io 标准）

```markdown
---
name: code-review
description: >-
  代码审查技能：识别缺陷、安全风险和最佳实践违规。
  当被要求审查或改善代码质量时使用。
license: Apache-2.0
shell: bash
paths:
  - "*.rs"
  - "*.py"
allowed-tools:
  - read_skill_resource
  - run_skill_script
  - Bash
metadata:
  author: my-team
  version: "1.0.0"
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: prompt
          prompt: "执行命令前验证安全性"
  PostToolUse:
    - matcher: "*"
      hooks:
        - type: command
          command: "${SKILL_DIR}/scripts/log_usage.sh"
          timeout: 5
---

## 代码审查

当用户要求审查代码时：

1. 加载检查清单：`read_skill_resource("code-review", "references/checklist.md")`
2. 逐项检查代码
3. 输出结构化审查意见

当前环境：!`uname -s`
技能目录：${SKILL_DIR}
```

### Frontmatter 字段说明

| 字段 | 必须 | 说明 |
|------|------|------|
| `name` | ✓ | 唯一名称，kebab-case，1-64 字符 |
| `description` | ✓ | 描述，最长 1024 字符，说明何时使用 |
| `license` | | SPDX 许可标识 |
| `shell` | | 内联命令使用的 Shell：`bash`（默认）或 `powershell` |
| `paths` | | 条件激活的文件 glob 模式（如 `["*.py"]`） |
| `allowed-tools` | | 声明此 Skill 偏好/允许使用的工具列表 |
| `hooks` | | PreToolUse / PostToolUse 钩子定义 |
| `metadata` | | 任意键值对（author, version, tags 等） |

`hooks` 内可用的 action 类型：
- `command`：执行命令，stdin 会收到 JSON 格式的 hook 上下文
- `prompt`：向 Agent 上下文注入额外提示
- `permission`：直接返回 `allow` / `deny` / `ask`，覆盖权限决策流程

### 内联命令执行

Skill 激活时，Markdown 正文中的命令会被自动执行并替换为输出：

```markdown
当前主机：!`uname -s`
```
→ 激活后变为：`当前主机：Darwin`

块命令：

````markdown
```!
rustc --version
```
````
→ 激活后变为：`rustc 1.93.0 (254b59607 2026-01-19)`

**安全限制**：MCP 来源的 Skill **永远不执行**内联命令（远程不可信内容）。

当内联命令或 hook 命令回退到直接进程执行（未配置 `SandboxManager`）时，运行时现在会：
- 先清空继承环境变量，再只注入最小白名单（`PATH`、`SKILL_DIR`、`SESSION_ID`）
- 通过 `kill_on_drop(true)` 做超时后的 best-effort 终止

这条 fallback 更适合 demo 和本地开发；生产环境仍建议优先配置 `SandboxManager`。

### 变量替换

| 变量 | 值 |
|------|-----|
| `${SKILL_DIR}` / `${CLAUDE_SKILL_DIR}` | Skill 目录的绝对路径 |
| `${SESSION_ID}` / `${CLAUDE_SESSION_ID}` | 当前会话标识 |
| `${ARGUMENTS}` | 所有参数（空格连接） |
| `${1}`, `${2}`, ... | 位置参数 |

### 目录结构

```
skills/
├── code-review/
│   ├── SKILL.md              ← 技能定义
│   ├── scripts/
│   │   └── lint.sh           ← 可执行脚本
│   └── references/
│       ├── checklist.md      ← 参考文档
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

```rust
use echo_agent::prelude::*;

let mut agent = ReactAgent::new(config);

// 方式 1：自动发现（项目级 + 用户级）
let skills = agent.discover_skills(&[
    DiscoveryScope::Project(".".into()),  // ./skills/ + ./.agents/skills/
    DiscoveryScope::User,                 // ~/.agents/skills/
]).await?;

// 方式 2：指定目录（向后兼容）
let skills = agent.load_skills_from_dir("./skills").await?;
```

发现后自动注册三个渐进式披露工具：

| 工具 | 说明 |
|------|------|
| `activate_skill` | 加载完整指引 + 资源列表（支持 `arguments` 参数） |
| `read_skill_resource` | 读取参考文件 |
| `run_skill_script` | 执行 Python/Bash/TS/PowerShell 脚本 |

如果同一个 Agent 后续再次调用 `discover_skills()` 发现了新的 file-based skill，
这三个工具会被刷新，以便它们内部共享的 registry 和可选技能列表始终与最新发现结果保持一致。

---

## Hooks 系统

Skill 可以通过 Hooks 拦截工具调用，实现安全审查、日志记录、输入/输出修改等功能。

### Hook 事件

| 事件 | 时机 | 能力 |
|------|------|------|
| `PreToolUse` | 工具执行前 | 阻止执行、修改输入、注入提示 |
| `PostToolUse` | 工具执行后 | 检查输出、触发后续操作 |

### Hook 类型

| 类型 | 行为 |
|------|------|
| `command` | 执行 Shell 命令，stdin 接收 JSON 上下文，stdout 返回 JSON 控制指令 |
| `prompt` | 注入提示消息给 LLM |

### Command Hook 输入（stdin JSON）

```json
{
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": {"command": "git status"},
  "tool_output": null
}
```

### Command Hook 输出（stdout JSON）

```json
{
  "decision": "block",
  "reason": "检测到不安全命令",
  "updatedInput": {"command": "git status --short"},
  "continue": false
}
```

| 字段 | 说明 |
|------|------|
| `decision` | `"allow"` 放行 / `"block"` 阻止 |
| `reason` | 阻止原因 |
| `updatedInput` | 修改后的工具输入（仅 PreToolUse） |
| `continue` | `false` 停止后续 Hook 执行 |

如果多个匹配的 hook 同时返回 `permission_mode_override`，运行时会保留最后一个非空覆盖值。
而权限决策本身仍然遵循更严格的优先级（`deny > ask > allow`）。

### 示例：YAML 定义

```yaml
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: command
          command: "${SKILL_DIR}/scripts/validate.sh"
          timeout: 5
        - type: prompt
          prompt: "执行前请验证命令安全性"
  PostToolUse:
    - matcher: "*"
      hooks:
        - type: prompt
          prompt: "检查输出中是否包含敏感信息"
```

### Matcher 匹配规则

- `"*"` — 匹配所有工具
- `"Bash"` — 精确匹配
- `"Bash"` 也匹配 `"Bash(git:*)"` 等带括号的变体

---

## 条件激活

带 `paths` 字段的 Skill 仍会出现在 catalog 中，但运行时激活会要求提供匹配的
`context_path`：

```yaml
paths:
  - "*.py"
  - "tests/**"
```

目录中会标注：`- python-linter: ... [activates for: *.py, tests/**]`

激活时需要类似这样调用：

```json
{
  "name": "python-linter",
  "context_path": "tests/test_api.py"
}
```

如果缺少 `context_path`，或它不匹配声明的 glob，`activate_skill` 会直接报错，
而不是继续加载该 skill。

---

## 工具权限限制

`allowed-tools` 用来声明 Skill 偏好/允许使用的工具。该约束会在激活时注入提示，
同时内建的渐进式披露工具也会在运行时执行校验：

```yaml
allowed-tools:
  - read_skill_resource
  - run_skill_script
  - Bash
```

其中 `read_skill_resource` 和 `run_skill_script` 会在运行时检查白名单；如果当前
skill 没有允许对应工具，调用会被直接拒绝。

---

## 跨平台脚本执行

`run_skill_script` 工具支持自动检测解释器：

| 扩展名 | Unix | Windows |
|--------|------|---------|
| `.py` | `python3` | `python` / `py -3` |
| `.js` | `node` | `node` |
| `.ts` | `bun` → `deno` → `npx tsx` | 同左 |
| `.sh` | `bash` | Git Bash → PowerShell |
| `.ps1` | `pwsh` | `powershell` |
| `.rb` | `ruby` | `ruby` |

直接调用解释器（不通过 `sh -c` / `cmd /C`），避免 Shell 注入风险。

另外还有两条运行时保证：
- `script` 路径必须是相对路径，并且规范化后仍位于已激活 skill 的目录内
- 畸形 `args`（如未闭合引号）会直接报错，而不是静默当作一个整体参数继续执行

---

## 上下文保护

已激活 Skill 的指引内容受到**压缩保护**——即使上下文超限触发压缩，Skill 指引也不会被裁剪。

```rust
// 内部机制：包含 <skill_content 标记的消息被排除在压缩范围外
ctx.add_protected_marker("<skill_content".to_string());
```

---

## 查询已安装 Skill

```rust
// 列出所有已安装 Skill
for info in agent.list_skills() {
    println!("- {} ({} 个工具)", info.name, info.tool_names.len());
}

// 检查 Skill 是否已安装
if agent.has_skill("calculator") {
    println!("计算器技能已安装");
}

// 总数
println!("已安装 {} 个 Skill", agent.skill_count());
```

---

## 完整示例

对应示例文件：
- `examples/demo07_skills.rs` — Code-based Skill 演示
- `examples/demo08_external_skills.rs` — File-based Skill 全功能演示（渐进式披露 + 脚本执行 + 内联命令 + Hooks）
