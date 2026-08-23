# Hooks 系统

## 概述

Hooks 允许在 Agent 生命周期的关键节点注入自定义行为。框架提供三套独立的 Hook 系统：

1. **Skills Hooks** — 主 Hook 系统，支持 31 个事件和 7 种动作类型
2. **Task Hooks** — DAG 任务执行的生命周期回调
3. **Subagent Hooks** — 子代理调度的生命周期回调

---

## Skills Hooks

主 Hook 系统。通过 YAML 配置（`application configuration` 或 SKILL.md frontmatter），由 `HookRegistry` 统一分发执行。

### Hook 事件

事件按类别分组，matcher 语义因类别而异（见 `HookEventCategory`）。权威定义见 `echo-core/src/hooks/types.rs`。

#### 工具事件（matcher = 工具名）

| 事件 | 触发时机 | 可修改内容 |
|------|---------|-----------|
| `PreToolUse` | 工具执行前 | 输入、权限（允许/阻止） |
| `PostToolUse` | 工具成功后 | 输出、继续行为 |
| `PostToolUseFailure` | 工具失败后 | 错误反馈 |
| `PermissionRequest` | 权限对话框出现时 | 自动批准/拒绝 |
| `PermissionDenied` | 权限被拒绝时 | 重试信号 |

#### 会话生命周期事件（matcher = lifecycle hint）

| 事件 | 触发时机 | 可修改内容 |
|------|---------|-----------|
| `SessionStart` | 会话开始或恢复时 | 上下文注入 |
| `SessionEnd` | 会话终止时 | 清理 |
| `Stop` | Agent 完成响应时 | 继续原因 |
| `Notification` | Agent 需要用户注意时 | 权限快捷方式 |
| `UserPromptSubmit` | 用户提交 prompt 时 | 上下文注入、阻止 |
| `PreCompact` | 上下文压缩前 | 上下文注入 |
| `PostCompact` | 上下文压缩后 | 上下文注入 |
| `ConfigChange` | 配置文件变更时 | 阻止/重载 |
| `InstructionsLoaded` | 技能/指令加载后 | 加载后验证 |
| `PostToolBatch` | 并行工具调用批次完成后 | 聚合 |
| `PluginLoaded` | 插件加载并注册组件后 | — |
| `PluginDisabled` | 插件禁用/卸载后 | — |

#### Subagent 事件（matcher = subagent 名称/类型）

业界归一模型(Claude Code / Codex / OpenAI Agents SDK / AGTP):两个边界事件 + 终态 status 枚举,无独立 Cancelled 事件。

| 事件 | 触发时机 | Context |
|------|---------|---------|
| `SubagentStart` | 子代理调度前 | name/mode/task |
| `SubagentStop` | 子代理到达终态(始终只发一次) | name/mode/result + `subagent_stop_status` |

`SubagentStop` 的 `subagent_stop_status` 取值(穷尽终态):

| status | 含义 |
|--------|------|
| `completed` | 正常结束,有产出 |
| `failed` | 报错/异常退出 |
| `cancelled` | 被外部取消(用户 Esc / 父 run 取消) |
| `timed_out` | 触发 deadline/timeout |

> 注:旧的 `SubagentCancelled` 独立事件已删除 —— cancelled 现在是 `SubagentStop` 的一个 status 值。emission owner 是 `SubagentExecutor`(经 `unified_hook_executor`),每次实际 dispatch attempt 都有一对 Start/Stop；重试会开启新的 attempt。

#### Task 事件（matcher = task subject/name）

| 事件 | 触发时机 |
|------|---------|
| `TaskCreated` | 任务节点进入 plan revision 时 |
| `TaskStarted` | 任务 attempt 被 claim 时 |
| `TaskCompleted` | 任务终态；`task_terminal_status` 区分 completed/failed/skipped/cancelled/timed_out |

#### 错误事件（不支持 matcher）

| 事件 | 触发时机 |
|------|---------|
| `StopFailure` | Agent 遇到不可恢复错误时 |

#### Evolution 事件（matcher = memory source / layer）

| 事件 | 触发时机 |
|------|---------|
| `PostMemoryWrite` | 任意记忆写入 Store 后 |
| `MemoryLayerChange` | 记忆在层间升/降级后 |
| `SkillCandidateDetected` | 从记忆模式检测到技能候选后 |
| `SkillLifecycleTransition` | 技能在生命周期状态间转换后 |
| `SkillHealthCheck` | 技能健康检查完成后 |
| `SkillPatchApplied` | 技能补丁应用后 |
| `SkillMergeApplied` | 两个或更多技能合并后 |
| `RulePromoted` | 记忆提升为 AGENTS.md 规则后 |

上述 8 个 Evolution 事件均由对应的记忆写入/层级迁移、候选检测、生命周期转换、
健康检查、补丁、合并和规则提升主路径真实发射，不是只保留在枚举中的占位事件。
Task/Subagent 的取消与超时由对应终态事件的结构化 status 表达，不再使用独立事件。

### Hook 动作类型

| 类型 | 行为 |
|------|------|
| `command` | 执行 shell 命令；stdin 接收 JSON 上下文 |
| `prompt` | 为 LLM 注入提示消息 |
| `permission` | 直接返回权限决策（allow/deny/ask） |
| `http` | 向 URL 发送事件数据并解析响应 |
| `mcp_tool` | 调用用户配置的 MCP 服务器工具 |
| `subagent` | 通过 Agent 已注册的 Subagent 运行时派发具名 Subagent |
| `activate_skill` | 直接激活一个技能（不经 LLM），reason 作为系统说明呈现给模型 |

### Hook 来源（source identity）

每个 hook 注册时携带来源标识，用于 `list_sources`、热更新按来源替换、reload 重建：

| `HookSource` | 含义 | 注册入口 |
|---|---|---|
| `Skill(name)` | 来自文件型 skill 的 hooks | `HookRegistry::register` |
| `UserConfig` | 来自用户配置（application configuration 内嵌 + hooks.yaml 文件） | `HookRegistry::register_user_hooks` |
| `Plugin(name)` | 来自已安装插件的 hooks | `HookRegistry::register_plugin_hooks` |

执行优先级：`UserConfig` < `Plugin` < `Skill`（见 `HookRegistry::run_hooks` 的 source 排序）。

### YAML 配置

```yaml
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: command
          command: "${SKILL_DIR}/validate.sh"
          timeout: 5
    - matcher: "Write"
      hooks:
        - type: prompt
          prompt: "写入前检查文件权限"
  PostToolUse:
    - matcher: "Edit|Write"
      hooks:
        - type: command
          command: "jq -r '.tool_input.file_path' | xargs prettier --write"
  Stop:
    - hooks:
        - type: command
          command: "osascript -e 'display notification \"完成\"'"
  SessionStart:
    - matcher: "startup"
      hooks:
        - type: prompt
          prompt: "记住使用 bun，不要用 npm。"
  PermissionRequest:
    - matcher: "shell"
      hooks:
        - type: permission
          decision: "allow"
  StopFailure:
    - hooks:
        - type: subagent
          name: incident-reviewer
          task: "总结失败原因并提出恢复步骤"
          timeout: 900
```

### 匹配模式

`matcher` 字段过滤哪些工具/事件触发 Hook：

- `"Bash"` — 精确匹配工具名
- `"Edit|Write"` — 管道符分隔的候选项（匹配 Edit 或 Write）
- `"*"` 或省略 matcher — 匹配所有事件
- `"startup"` — 匹配 SessionStart 中的上下文关键词

### Command Hook 上下文

Command Hook 通过 stdin 接收完整的 `HookContext` JSON（含 `hook_event_name` 兼容字段）：

```json
{
  "event": "PreToolUse",
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": { "command": "ls -la" },
  "session_id": "abc123",
  "agent_name": "eko",
  "cwd": "/home/user/project"
}
```

命令的 stdout 被解析为 `HookResult`（见 `parse_hook_output`）：

```json
{
  "decision": "allow",
  "updatedInput": { "command": "ls -la --color=never" },
  "injected_context": "已修改命令以禁用颜色",
  "permission_mode_override": "auto"
}
```

退出码语义（对齐 Claude Code 约定）：`0`/`1` 不阻塞，`2` 显式阻塞，其它非零仅告警不阻塞。

插件 Command Hook 还会收到 `PLUGIN_ROOT` 与 `PLUGIN_DATA`。为兼容已有插件包，
相同值也通过 `CLAUDE_PLUGIN_ROOT` / `CLAUDE_PLUGIN_DATA` 和
`ECHO_PLUGIN_ROOT` / `ECHO_PLUGIN_DATA` 提供。路径通过进程环境传递，不再拼接进
shell 源码，因此安装路径包含空格或 shell 特殊字符时仍可正常运行。

退出码 `2` 会阻止操作，并把 stderr 作为面向用户的原因；其它非零退出仍不阻止，
但会进入 HookResult 消息，不再只留在日志里。

为复用可移植插件，embedding application 也接受 Codex 风格的 `systemMessage` 与
`hookSpecificOutput` 字段：`additionalContext`、`permissionDecision`、
`permissionDecisionReason`、`updatedInput`，以及 PermissionRequest 的
`decision.behavior` 对象。所有进入模型上下文的文本都会先进行 UTF-8 安全的长度限制。

以上是规范 wire 字段名，`modified_input`、`message` 和 `permission_mode` 不是别名。
`PreToolUse` 或 `PermissionRequest` 可以返回 `permission_mode_override`。该值只作用于
当前工具调用，由主执行 Pipeline 传入权限服务，不会修改会话级权限模式，也不会污染
并发调用。规范值为 `default`、`plan`、`auto`、`acceptEdits`、
`bypassPermissions`、`bubble`、`dontAsk` 和 `strict`。

### 来源、热更新与 Dry Run

User、Skill、Plugin 三种来源统一使用同一套注册期 Action 校验：无效 Action
会被记录并过滤，同一规则中的有效 Action 仍正常注册。

embedding application 会合并 `application configuration` 内嵌 Hooks、全局 `<application-data>/hooks.yaml` 和项目
`<application-data>/hooks.yaml`。监听器同时监控这三个目标；创建、修改、原子替换和删除都会触发重载，
因此删除 `hooks.yaml` 会立即移除其 Hook，无需重启。解析失败时保留 last-known-good
注册表。CLI、TUI、GUI 的 Hook 测试均调用
`HookRegistry::dry_run`，真实计算事件、matcher、来源和 Action，但不执行任何副作用。

### 运行限制与本地扩展模型

| 限制 | 值 | 目的 |
|------|-----|------|
| 默认超时 | 600 秒 | 支持真实 Command、MCP、HTTP 与 Subagent 工作 |
| 最大超时 | 3600 秒 | 限制意外不结束的 Hook |
| 最大命令长度 | 32K 字符 | 拒绝明显畸形的 YAML |
| 沙箱执行 | 可选 | Hook 可在沙箱内运行 |

embedding application 是用户本机上的可信个人助理。HTTP Hook 允许 loopback、私网和 link-local IP
字面量，以及 `localhost`、`nas` 这类单标签主机和以 `.local` / `.lan` 结尾的域名使用
明文 HTTP；远程地址仍要求 HTTPS。用户配置的 headers 与 payload 会原样发送，命令
诊断会对已替换的敏感值脱敏。MCP Hook 可调用用户所配置服务器暴露的任意工具，框架
不再维护针对本地可信扩展的工具 deny-list。

---

## Task Hooks

DAG 任务执行的生命周期回调。实现 `TaskHooks` trait。

### Trait 定义

```rust
use async_trait::async_trait;
use echo_agent::tasks::{RetryDecision, TaskHookContext, TaskHooks};

struct LoggingHooks;

#[async_trait]
impl TaskHooks for LoggingHooks {
    async fn before_execute(&self, ctx: &TaskHookContext) {
        println!("开始任务: {}", ctx.task.subject);
    }

    async fn after_execute(&self, ctx: &TaskHookContext, result: &str) {
        println!("完成: {} -> {}", ctx.task.subject, result);
    }

    async fn on_failure(&self, ctx: &TaskHookContext, error: &str) -> RetryDecision {
        if ctx.task.retry_count < ctx.task.max_retries {
            RetryDecision::Retry { delay_secs: 1 }
        } else {
            RetryDecision::Fail
        }
    }
}
```

### Hook 上下文

```rust
pub struct TaskHookContext {
    pub task: Task,              // 正在执行的任务
    pub attempt: u32,            // 当前尝试次数（从 1 开始）
    pub executor: Option<String>, // 执行任务的 Agent
}
```

### 重试决策

| 决策 | 行为 |
|------|------|
| `Retry { delay_secs }` | 延迟后重新执行 |
| `Skip` | 跳过任务，继续 DAG |
| `Fail` | 标记任务为失败 |

---

## Subagent Hooks

子代理调度的生命周期回调。实现 `SubagentHooks` trait。

### Trait 定义

```rust
use async_trait::async_trait;
use echo_agent::subagent::{SubagentHooks, SubagentHookContext, SubagentRetryDecision, SubagentResult};

struct MySubagentHooks;

#[async_trait]
impl SubagentHooks for MySubagentHooks {
    async fn before_dispatch(&self, ctx: &SubagentHookContext) {
        println!("调度到: {}", ctx.subagent_name);
    }

    async fn after_dispatch(&self, ctx: &SubagentHookContext, result: &SubagentResult) {
        println!("完成: {}", ctx.subagent_name);
    }

    async fn on_failure(&self, ctx: &SubagentHookContext, error: &str) -> SubagentRetryDecision {
        SubagentRetryDecision::Retry { delay_secs: 2 }
    }
}
```

### Hook 上下文

```rust
pub struct SubagentHookContext {
    pub parent_agent: String,         // 父 Agent 名称
    pub subagent_name: String,        // 被调度的子代理
    pub execution_mode: ExecutionMode, // Sync/Fork/Teammate
    pub task: String,                 // 被调度的任务
    pub attempt: u32,                 // 当前尝试次数（从 1 开始）
}
```

### 重试决策

| 决策 | 行为 |
|------|------|
| `Retry { delay_secs }` | 延迟后重新调度 |
| `Fail` | 向父 Agent 传播错误 |
| `Delegate { alternative_agent }` | 调度到不同的子代理 |

---

## 组合使用

三套 Hook 系统可以同时使用：

```rust
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .with_task_hooks(Arc::new(LoggingHooks))
    .with_subagent_hooks(Arc::new(MySubagentHooks))
    .build()?;
```

Skills Hooks 通过 YAML 配置，从 `application configuration` 或 SKILL.md 文件自动加载。
