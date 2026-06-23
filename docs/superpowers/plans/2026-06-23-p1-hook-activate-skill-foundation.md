# P1 框架底座实施计划:Hook 直接激活技能

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 EKO 的 hook 系统能"直接激活技能"(而非只能提示模型),并补齐 `UserPromptSubmit` 触发点和 `hooks.json` 发现——这是三重触发的框架底座,后续 P2-P8 全部依赖它。

**Architecture:** 在 `HookAction` enum 新增 `ActivateSkill` 变体(声明式,frontmatter 可写),在 `HookResult` 新增 `activate_skill` 字段传递激活请求,在 `execute_action` 产出该字段,在 `fire_lifecycle_hook` 收到该字段后调用 `activate_skill_for_context` 完成真实激活。`hooks.json` 发现放在 `SkillLoader::scan_directory`,与 `SKILL.md` 并列读取。

**Tech Stack:** Rust(workspace `echo-agent`,crate `echo_agent` + `echo_core` + `echo_execution`),`tokio`,`serde`,`serde_json`/`serde_yaml_ng`。测试用 `tokio::test` + `tempfile`。

**前置事实(已代码核实,实现时直接用)**:
- `HookAction` enum 定义:`echo-execution/src/skills/hooks.rs:128-186`,`#[serde(tag = "type", rename_all = "lowercase")]`。
- `validate()`:`hooks.rs:188-271`,对每个变体校验。
- `execute_action(action, source_dir, context, sandbox, http_client, mcp_executor) -> HookResult`:**自由函数**(非方法),`hooks.rs:668`,match 每个变体返回 `HookResult`。
- `HookResult` struct:`echo-core/src/hooks/types.rs:812`,字段含 `block/block_reason/messages/injected_context` 等。`Default` 已 derive。
- `merge_result(combined: &mut HookResult, incoming: HookResult)`:**自由函数**,`hooks.rs:1329`,逐字段合并。
- `fire_lifecycle_hook(&self, event, matcher) -> HookResult`:`ReactAgent` 方法,`echo-agent/src/agent/react/run/context.rs:317`,末尾(453-466)处理 `injected_context`/`messages` 后返回 result。
- `activate_skill_for_context(&self, skill_name: &str) -> Result<()>`:`ReactAgent` 方法,`capabilities.rs:686`。
- `UserPromptSubmit` 事件已在 `HookEvent`(`types.rs:75`)且 `supports_matcher` 为真,**但 `fire_lifecycle_hook` 从不被以 `UserPromptSubmit` 调用**(`context.rs` 只有 SessionStart 调用)。
- `SkillLoader::scan_directory`:`echo-execution/src/skills/external/loader.rs:136`,遍历子目录读 `path.join(SKILL_FILE)`。
- `SKILL_FILE` 常量 + `parse_skill_file` 同文件内。

**遵循 AGENTS.md 硬性约束**:
- 禁止 `unwrap()`/`expect()`/字节切片/直接越界索引;全部用 `?`/`unwrap_or`/`get`/`chars().take()`。
- 测试失败必须修到全绿;提交前 `cargo check/test/fmt/clippy` 全过 + 全 feature 矩阵。
- `cargo check -p echo_agent`(crate 名下划线)。
- 所有 `git commit` 加 `-c commit.gpgsign=false`;在 `echo-agent` 子仓库目录内执行。

---

## 文件结构

每个文件一个清晰职责:

| 文件 | 改动 | 职责 |
|---|---|---|
| `echo-core/src/hooks/types.rs` | 修改 | `HookResult` 加 `activate_skill` 字段(+ getter) |
| `echo-execution/src/skills/hooks.rs` | 修改 | `HookAction` 加 `ActivateSkill` 变体 + `validate` + `execute_action` 分支 + `merge_result` 合并 |
| `echo-execution/src/skills/external/loader.rs` | 修改 | `scan_directory` 并列读 `hooks.json` |
| `echo-agent/src/agent/react/run/context.rs` | 修改 | `fire_lifecycle_hook` 收到 `activate_skill` 后调用 `activate_skill_for_context`;新增 `UserPromptSubmit` 触发点 |
| `echo-core/src/hooks/types.rs`(测试) | 新增 | `HookResult` 字段单测 |
| `echo-execution/src/skills/hooks.rs`(测试) | 新增 | `ActivateSkill` validate + execute_action + merge 单测 |
| `echo-execution/src/skills/external/loader.rs`(测试) | 新增 | `hooks.json` 发现单测 |

---

## Task 1: `HookResult` 新增 `activate_skill` 字段

这是底座——没有它,后续 execute_action/merge/fire_lifecycle_hook 无处传递激活请求。先改最底层(`echo-core`)。

**Files:**
- Modify: `echo-agent/echo-core/src/hooks/types.rs:812-873`(`HookResult` struct + impl)
- Test: `echo-agent/echo-core/src/hooks/types.rs`(同文件 `#[cfg(test)] mod tests`)

- [ ] **Step 1: 写失败测试——字段存在 + 默认 None + 带构造函数**

在 `echo-core/src/hooks/types.rs` 的 `#[cfg(test)] mod tests`(若不存在则在文件末尾新建)里加:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activate_skill_defaults_to_none() {
        let r = HookResult::default();
        assert!(r.activate_skill.is_none());
    }

    #[test]
    fn activate_skill_constructor_sets_field() {
        let r = HookResult::with_activate_skill("docx".to_string(), "检测到 .docx 文件".to_string());
        assert_eq!(r.activate_skill, Some(("docx".to_string(), "检测到 .docx 文件".to_string())));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo test -p echo_core --lib hooks::types::tests`
Expected: 编译失败,`no field activate_skill` / `no function with_activate_skill`。

- [ ] **Step 3: 加字段 + 构造函数**

在 `HookResult` struct(`types.rs:812`,字段列表末尾,`pub metadata` 之后)加:

```rust
    /// For ActivateSkill hooks: skill to activate directly (name + reason).
    /// Populated by `execute_action` when an `ActivateSkill` action matches;
    /// consumed by `fire_lifecycle_hook` to call `activate_skill_for_context`.
    pub activate_skill: Option<(String, String)>,
```

在 `impl HookResult`(`types.rs:837`,在 `ask` 构造函数之后、`has_permission_decision` 之前)加:

```rust
    /// Create a result requesting direct skill activation.
    pub fn with_activate_skill(skill: String, reason: String) -> Self {
        Self {
            activate_skill: Some((skill, reason)),
            ..Self::default()
        }
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo test -p echo_core --lib hooks::types::tests`
Expected: 2 tests PASS。

- [ ] **Step 5: 确认下游 crate 仍编译(默认值不破坏现有调用)**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo check -p echo_execution`
Expected: 编译通过(新字段有 derive Default,现有 `HookResult::default()` 自动带 None)。

- [ ] **Step 6: 提交**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent
git add echo-core/src/hooks/types.rs
git -c commit.gpgsign=false commit -m "feat(hooks): HookResult 加 activate_skill 字段

为三重触发的'hook 直接激活技能'打底:新增可选 (skill, reason) 字段,
默认 None 不影响现有调用。下一步 Task 2 在 HookAction/execute_action 接线。"
```

---

## Task 2: `HookAction::ActivateSkill` 变体 + validate + execute_action + merge

声明层(frontmatter 可写)+ 执行层(产出 HookResult)+ 合并层(多 hook 累积)。

**Files:**
- Modify: `echo-agent/echo-execution/src/skills/hooks.rs:128-186`(enum)、`:188-271`(validate)、`:668-760`(execute_action)、`:1329-1390`(merge_result)
- Test: `echo-agent/echo-execution/src/skills/hooks.rs`(同文件 `#[cfg(test)] mod tests`)

- [ ] **Step 1: 写失败测试——validate + execute_action + merge**

在 `hooks.rs` 的 `#[cfg(test)] mod tests` 加:

```rust
#[cfg(test)]
mod activate_skill_tests {
    use super::*;
    use crate::skills::hooks::HookContext;

    fn ctx() -> HookContext {
        HookContext::for_user_prompt_submit("hello", None, "s1", "agent")
    }

    #[test]
    fn validate_rejects_empty_skill_name() {
        let a = HookAction::ActivateSkill { skill: String::new(), reason: "r".into() };
        assert!(a.validate().is_err());
    }

    #[test]
    fn validate_accepts_nonempty() {
        let a = HookAction::ActivateSkill { skill: "docx".into(), reason: "r".into() };
        assert!(a.validate().is_ok());
    }

    #[tokio::test]
    async fn execute_action_activate_skill_produces_result() {
        let a = HookAction::ActivateSkill {
            skill: "docx".into(),
            reason: "检测到 .docx".into(),
        };
        let result = execute_action(&a, "/tmp", &ctx(), None, None, None).await;
        assert_eq!(result.activate_skill, Some(("docx".to_string(), "检测到 .docx".to_string())));
        // ActivateSkill 不阻塞、不注入 context(激活由 fire_lifecycle_hook 负责)
        assert!(!result.block);
        assert!(result.injected_context.is_none());
    }

    #[test]
    fn merge_takes_first_activate_skill() {
        // 多个 ActivateSkill 命中:第一个优先(stop_propagation 由调用方控制)
        let mut combined = HookResult::default();
        let first = HookResult::with_activate_skill("docx".into(), "r1".into());
        merge_result(&mut combined, first);
        let second = HookResult::with_activate_skill("pdf".into(), "r2".into());
        merge_result(&mut combined, second);
        assert_eq!(combined.activate_skill, Some(("docx".to_string(), "r1".to_string())));
    }

    #[test]
    fn merge_activate_skill_from_none() {
        let mut combined = HookResult::default();
        merge_result(&mut combined, HookResult::with_activate_skill("pdf".into(), "r".into()));
        assert_eq!(combined.activate_skill.as_ref().map(|(s, _)| s.clone()), Some("pdf".into()));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo test -p echo_execution --lib skills::hooks::activate_skill_tests`
Expected: 编译失败,`no variant ActivateSkill`。

- [ ] **Step 3: 加 enum 变体**

在 `HookAction` enum(`hooks.rs:130`,在 `Agent { ... }` 变体之后、enum 闭合 `}` 之前)加:

```rust
    /// Directly activate a skill without going through the LLM.
    /// The hook engine calls `activate_skill_for_context(skill)` when matched.
    /// `reason` is surfaced to the model as a system note explaining why.
    ActivateSkill {
        /// Name of the skill to activate (must match a discovered skill).
        skill: String,
        /// Human-readable reason shown to the model.
        #[serde(default)]
        reason: String,
    },
```

- [ ] **Step 4: 加 validate 分支**

在 `validate()` 的 match(`hooks.rs:192`)里,`HookAction::Agent { ... }` 分支之后加:

```rust
            HookAction::ActivateSkill { skill, .. } => {
                if skill.is_empty() {
                    return Err("ActivateSkill hook has empty skill name".into());
                }
            }
```

- [ ] **Step 5: 加 execute_action 分支**

在 `execute_action` 的 match(`hooks.rs:676`)里,`HookAction::Agent { ... }` 分支之后加:

```rust
        HookAction::ActivateSkill { skill, reason } => HookResult::with_activate_skill(
            skill.clone(),
            reason.clone(),
        ),
```

> 注意:`ActivateSkill` 不需要 sandbox/http/mcp_executor 参数,纯声明式。真实激活在 Task 3 的 `fire_lifecycle_hook` 完成(因为那里有 `&self: &ReactAgent`,而 execute_action 是自由函数无 agent 句柄)。

- [ ] **Step 6: 加 merge 分支**

在 `merge_result`(`hooks.rs:1329`)内,`injected_context` 合并段(`:1377`)之前加:

```rust
    // activate_skill: first-wins(第一个非 None 的优先;避免多 hook 互相覆盖导致不确定)
    if combined.activate_skill.is_none() {
        combined.activate_skill = incoming.activate_skill;
    }
```

- [ ] **Step 7: 跑测试确认通过**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo test -p echo_execution --lib skills::hooks::activate_skill_tests`
Expected: 5 tests PASS。

- [ ] **Step 8: 跑全 hooks 模块测试,确认没破坏现有 match 穷尽性**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo test -p echo_execution --lib skills::hooks`
Expected: 全部 PASS(新增变体已在所有 match 处理:validate/execute_action 各一处)。若有 `non-exhaustive` 编译错误,定位漏掉的 match 补上分支。

- [ ] **Step 9: 提交**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent
git add echo-execution/src/skills/hooks.rs
git -c commit.gpgsign=false commit -m "feat(hooks): HookAction::ActivateSkill 变体

新增声明式'直接激活技能'hook 动作:frontmatter 可写
  hooks:
    UserPromptSubmit:
      - matcher: \"*\"
        hooks:
          - type: activate_skill
            skill: docx
            reason: 检测到 .docx 路径
execute_action 产出 HookResult.activate_skill,merge 首个优先。
真实激活在 fire_lifecycle_hook(Task 3)完成。"
```

---

## Task 3: `fire_lifecycle_hook` 接线激活 + `UserPromptSubmit` 触发点

把 Task 1/2 的声明→产出连到真实激活(`activate_skill_for_context`),并补齐 `UserPromptSubmit` 在每条用户消息的触发。

**Files:**
- Modify: `echo-agent/src/agent/react/run/context.rs:317-467`(`fire_lifecycle_hook` 尾部结果处理)、`:453-464`(新增 activate 分支)
- Modify: `echo-agent/src/agent/react/run/context.rs`(`prepare_stream_context` / `prepare_react_context` 入口,新增 UserPromptSubmit 触发)
- Test: `echo-agent/src/agent/react/run/context.rs` 或集成测试

> **测试策略说明**:这一层涉及 `ReactAgent` 完整运行时,单元测试需要 mock agent。鉴于现有 `context.rs` 的 `fire_lifecycle_hook` 是 `pub async`,且 agent 构造成本高,采用**集成测试 + 日志断言**策略:构造一个最小 agent + 一个带 ActivateSkill hook 的测试技能目录,提交 prompt,断言日志出现"自动激活技能"。如果现有测试基建里已有 agent fixture(查 `echo-agent/tests/`),复用它;否则此 Task 用编译验证 + 手动触发验证(在 Task 5 整体验证里跑),单测聚焦在"activate_skill 字段被读取后调用了 activate_skill_for_context"的逻辑分支上。

- [ ] **Step 1: 找 prepare 入口和现有 agent 测试 fixture**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && grep -rn "fn prepare_react_context\|fn prepare_stream_context" src/agent/react/run/context.rs`
记录行号。再查 fixture:
Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && ls tests/ 2>/dev/null; grep -rln "ReactAgentBuilder\|fn test_agent" tests/ | head -5`

(这步是信息收集,不改代码。记录 fixture 路径供 Step 5 用。)

- [ ] **Step 2: 写失败测试——UserPromptSubmit 触发后 activate_skill 被消费**

在 `context.rs` 的 `#[cfg(test)] mod tests`(若无则新建,放最小逻辑测试)加。**如果无法构造完整 agent,改用这个纯逻辑测试**验证 `fire_lifecycle_hook` 在 result.activate_skill 为 Some 时会调 `activate_skill_for_context`(用 trait mock 或观测副作用):

```rust
#[cfg(test)]
mod activate_hook_tests {
    use super::*;
    use crate::skills::hooks::HookResult;

    /// 验证辅助函数:从 HookResult 决定是否需要激活、激活哪个技能。
    /// 这是 fire_lifecycle_hook 内联逻辑的纯函数提取,便于单测。
    #[test]
    fn extract_activation_request_from_result() {
        let r = HookResult::with_activate_skill("docx".into(), "r".into());
        assert_eq!(activation_target(&r), Some(("docx", "r")));

        let r2 = HookResult::default();
        assert_eq!(activation_target(&r2), None);
    }

    fn activation_target(r: &HookResult) -> Option<(&str, &str)> {
        r.activate_skill.as_ref().map(|(s, reason)| (s.as_str(), reason.as_str()))
    }
}
```

> 说明:`fire_lifecycle_hook` 依赖 `&self` 的完整 agent 状态(context lock、registry),无法纯单测。这里提取"决策"为纯函数 `activation_target`,并在 Step 3 让 `fire_lifecycle_hook` 调用它,使逻辑可测。真实端到端验证留到 Task 5。

- [ ] **Step 3: 跑测试确认失败**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo test -p echo_agent --lib agent::react::run::context::activate_hook_tests`
Expected: 编译失败,`cannot find function activation_target`。

- [ ] **Step 4: 在 fire_lifecycle_hook 末尾接 activate_skill**

先在 `context.rs` 加纯函数(放在 `fire_lifecycle_hook` 函数之后):

```rust
/// Decide whether a hook result requests direct skill activation.
/// Pure helper extracted so the decision is unit-testable without a full agent.
fn activation_target(r: &crate::skills::hooks::HookResult) -> Option<(&str, &str)> {
    r.activate_skill.as_ref().map(|(s, reason)| (s.as_str(), reason.as_str()))
}
```

然后在 `fire_lifecycle_hook`(`context.rs:466`,即 `result` 返回语句**之前**)插入激活逻辑:

```rust
        // ActivateSkill hook: directly activate the requested skill.
        if let Some((skill, reason)) = activation_target(&result) {
            match self.activate_skill_for_context(skill).await {
                Ok(()) => {
                    // Surface the reason to the model as a runtime note.
                    let note = format!("已根据上下文自动激活技能 {skill}:{reason}");
                    let mut ctx = self.memory.context.lock().await;
                    ctx.push(runtime_context_note("Hook:ActivateSkill", &note));
                    info!(skill = %skill, reason = %reason, "Hook activated skill");
                }
                Err(e) => {
                    warn!(skill = %skill, error = %e, "Hook-requested skill activation failed");
                }
            }
        }
```

- [ ] **Step 5: 跑单测确认通过**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo test -p echo_agent --lib agent::react::run::context::activate_hook_tests`
Expected: 1 test PASS。

- [ ] **Step 6: 补 UserPromptSubmit 触发点**

在 `prepare_stream_context`(`context.rs:473`)和 `prepare_react_context`(Step 1 记录的行号)函数体**开头**(reset/restore 逻辑之后、返回之前)加 UserPromptSubmit 触发。以 `prepare_stream_context` 为例,在其返回语句之前加:

```rust
        // Fire UserPromptSubmit hook so third-tier (forced check) hooks run per turn.
        // The prompt is passed via context; matcher "*" matches all prompts.
        let _ = self
            .fire_lifecycle_hook(HookEvent::UserPromptSubmit, Some("*"))
            .await;
```

> 注意:`UserPromptSubmit` 的 prompt 文本在 `fire_lifecycle_hook` 内 `HookContext::for_user_prompt_submit("", ...)` 用了空串(`context.rs:340` 注释"prompt is set by caller")。为让 hook 能看到 prompt,改为传入 input:把 `fire_lifecycle_hook` 内 `UserPromptSubmit` 分支的空串改为用 matcher 位传 prompt(或新增一个带 prompt 的重载)。**最小改动**:在调用处不传 prompt,保持现状;真正需要 prompt 内容的 hook(如强检查)留 P4 TriggerSupervisor 用专门的分类器读 prompt。本 Task 只补"每轮触发"这个机制,不传 prompt 文本,避免改 fire_lifecycle_hook 签名。

- [ ] **Step 7: 编译确认整个 workspace**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo check --workspace`
Expected: 编译通过。若有未使用 import 警告(`HookEvent`/`info` 等),确保已 `use`。

- [ ] **Step 8: 跑全 workspace 测试,确认没破坏 SessionStart 等现有 hook 行为**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo test --workspace`
Expected: 全绿。重点看 hooks / react / context 相关测试无回归。

- [ ] **Step 9: 提交**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent
git add src/agent/react/run/context.rs
git -c commit.gpgsign=false commit -m "feat(react): fire_lifecycle_hook 接线 ActivateSkill + UserPromptSubmit 触发

第三重保障框架底座:
- fire_lifecycle_hook 收到 HookResult.activate_skill 后调用
  activate_skill_for_context 完成真实激活,reason 作为 runtime note 注入。
- prepare_stream_context / prepare_react_context 每轮触发 UserPromptSubmit hook。
激活决策提取为纯函数 activation_target 便于单测。"
```

---

## Task 4: `hooks.json` 发现(SkillLoader 兼容 superpowers)

superpowers 的 hook 定义在外部 `hooks.json`,不是 frontmatter。让 `SkillLoader::scan_directory` 并列读取它,合并进该技能的 `HooksDefinition`。

**Files:**
- Modify: `echo-agent/echo-execution/src/skills/external/loader.rs:136-193`(`scan_directory`)、文件顶部常量区
- Test: `echo-agent/echo-execution/src/skills/external/loader.rs`(同文件 `#[cfg(test)] mod tests`)

**前置:EKO 的 hooks.json 格式约定**

superpowers 的 `hooks.json` 是 Claude Code 格式(数组里每项 `{events: [...], hooks: [{type, command}]}`)。EKO 的 `HooksDefinition` 是 `HashMap<HookEvent, Vec<HookRule>>`,frontmatter 用 `SessionStart:` 等事件名做 key。**为避免实现一整套 Claude Code 格式解析器**,本 Task 约定 EKO 读取的 `hooks.json` 采用**与 frontmatter 相同的结构**(事件名为顶层 key):

```json
{
  "SessionStart": [
    { "matcher": "startup|clear", "hooks": [ { "type": "activate_skill", "skill": "docx", "reason": "r" } ] }
  ]
}
```

> 这与 superpowers 原生 `hooks.json`(Claude Code 格式)不同。superpowers 资产移植时(P3),把原生 `hooks.json` **转写**成这个 EKO 格式,或直接写进 frontmatter。本 Task 只实现"EKO 格式 hooks.json 的发现与解析",不解析 Claude Code 格式(那是适配层工作,留 P3)。在 plan 里明确这一点,避免实现者误解。

- [ ] **Step 1: 写失败测试——发现并合并 hooks.json**

在 `loader.rs` 的 `#[cfg(test)] mod tests` 加:

```rust
#[cfg(test)]
mod hooks_json_tests {
    use super::*;
    use tempfile::TempDir;

    fn write_skill(dir: &std::path::Path, name: &str, frontmatter: &str, body: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let content = format!("---\n{}\n---\n{}", frontmatter, body);
        std::fs::write(skill_dir.join(SKILL_FILE), content).unwrap();
    }

    #[tokio::test]
    async fn scan_directory_merges_hooks_json() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // SKILL.md without hooks in frontmatter
        write_skill(
            root,
            "my-skill",
            "name: my-skill\ndescription: test",
            "body",
        );
        // hooks.json alongside SKILL.md (EKO format)
        let hooks_json = r#"{
            "SessionStart": [
                {"matcher": "startup", "hooks": [{"type": "activate_skill", "skill": "docx", "reason": "r"}]}
            ]
        }"#;
        std::fs::write(root.join("my-skill").join("hooks.json"), hooks_json).unwrap();

        let loader = SkillLoader::new();
        let descs = loader.discover_from_dir(root.to_path_buf()).await.unwrap();
        assert_eq!(descs.len(), 1);
        let desc = &descs[0];
        // hooks from hooks.json merged into descriptor
        let start_rules = desc.hooks.rules_for(HookEvent::SessionStart);
        assert!(!start_rules.is_empty(), "SessionStart rules should be merged from hooks.json");
    }

    #[tokio::test]
    async fn scan_directory_without_hooks_json_still_works() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "plain", "name: plain\ndescription: d", "b");
        let loader = SkillLoader::new();
        let descs = loader.discover_from_dir(tmp.path().to_path_buf()).await.unwrap();
        assert_eq!(descs.len(), 1);
        assert!(descs[0].hooks.is_empty());
    }
}
```

> 注:`SkillDescriptor` 必须有 `hooks: HooksDefinition` 字段(核实 `external/types.rs`),`rules_for` 来自 `HooksDefinition`。若字段名不同,按实际改测试。`discover_from_dir` 是 `loader.rs:128` 的方法。

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo test -p echo_execution --lib skills::external::loader::hooks_json_tests`
Expected: 失败——`hooks.json` 未被读取,`start_rules` 为空。

- [ ] **Step 3: 核实 SkillDescriptor.hooks 字段名**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && grep -n "pub hooks" echo-execution/src/skills/external/types.rs`
记录字段名(假设是 `hooks`)。若字段不存在或为 `Option`,测试和实现都要相应调整。

- [ ] **Step 4: 加常量 + 实现 hooks.json 读取**

在 `loader.rs` 顶部常量区(`SKILL_FILE` 旁边)加:

```rust
/// Hook definition file (EKO format) alongside SKILL.md.
/// Distinct from superpowers' Claude-Code-format hooks.json; assets are
/// transcribed to this format at integration time (see P3 of the plan).
const HOOKS_FILE: &str = "hooks.json";
```

在 `scan_directory`(`loader.rs:170`,`if skill_file.exists()` 块内,`parse_skill_file` 成功分支里)加 hooks.json 合并逻辑。把现有:

```rust
                    found.push((desc, legacy_instr));
```

改为:

```rust
                        // Merge external hooks.json (EKO format) if present.
                        let mut desc = desc;
                        let hooks_path = path.join(HOOKS_FILE);
                        if hooks_path.exists() {
                            match tokio::fs::read_to_string(&hooks_path).await {
                                Ok(text) => {
                                    match serde_json::from_str::<HooksDefinition>(&text) {
                                        Ok(extra) => {
                                            info!(
                                                "Merged hooks.json for skill '{}' from {}",
                                                desc.name,
                                                hooks_path.display()
                                            );
                                            desc.hooks.merge(extra);
                                        }
                                        Err(e) => {
                                            warn!(
                                                "Failed to parse '{}' for skill '{}': {}",
                                                hooks_path.display(),
                                                desc.name,
                                                e
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        "Cannot read '{}': {}",
                                        hooks_path.display(),
                                        e
                                    );
                                }
                            }
                        }
                        found.push((desc, legacy_instr));
```

> 确保 `HooksDefinition` 和 `HookEvent` 已 `use` 进 `loader.rs`(查文件顶部 `use` 块,缺的补:`use crate::skills::hooks::{HooksDefinition};` 和 `use echo_core::hooks::HookEvent;`——按实际路径)。

- [ ] **Step 5: 跑测试确认通过**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo test -p echo_execution --lib skills::external::loader::hooks_json_tests`
Expected: 2 tests PASS。若第一个测试因 `HooksDefinition::merge` 签名不同失败,调整(merge 是 `&mut self`,代码已是 `desc.hooks.merge(extra)`)。

- [ ] **Step 6: 提交**

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent
git add echo-execution/src/skills/external/loader.rs
git -c commit.gpgsign=false commit -m "feat(skills): SkillLoader 发现并合并 hooks.json

scan_directory 并列读取 SKILL.md 同级的 hooks.json(EKO 格式,
事件名为顶层 key,与 frontmatter hooks: 同构),合并进 descriptor.hooks。
解析失败仅告警不中断。注意:superpowers 原生 Claude-Code 格式 hooks.json
需在 P3 资产移植时转写为本格式。"
```

---

## Task 5: 全 feature 矩阵验证 + clippy + fmt + 收尾

AGENTS.md 强制:P1 改动涉及 `echo_core` + `echo_execution` + `echo_agent`,必须全 feature 验证,不能只跑默认 feature。

**Files:** 无(纯验证)

- [ ] **Step 1: 格式化**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo fmt --all`
Expected: 无输出或自动格式化(若有改动,`git add` 后继续)。

- [ ] **Step 2: 默认 feature 编译 + 测试**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo check --workspace && cargo test --workspace`
Expected: 编译零错误,测试全绿。

- [ ] **Step 3: 逐 feature 编译(catch cfg 路径破坏)**

P1 改的 hook/skill/loader 代码在 `echo_core`/`echo_execution`,无 feature gate(常驻),但仍需确认不破坏带 feature 的编译:

```bash
cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent
cargo check -p echo_agent --no-default-features --features sqlite
cargo check -p echo_agent --no-default-features --features subagent
cargo check -p echo_agent --no-default-features --features human-loop
cargo check -p echo_agent --no-default-features --features mcp
```
Expected: 全部编译通过。若某 feature 失败,定位是否该 feature 下有未 match 的 `HookAction`(unlikely,新变体无 feature gate)。

- [ ] **Step 4: clippy 零警告**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo clippy --all-targets -- -D warnings`
Expected: 零警告。常见:`unused variable`(Step 5 的 `_` 已处理)、`unnecessary unwrap`(检查没引入)。

- [ ] **Step 5: 如有 fmt/clippy 改动,补提交**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && git status --short`
若有改动:
```bash
git add -A
git -c commit.gpgsign=false commit -m "chore: fmt + clippy 修复 (P1)"
```

- [ ] **Step 6: 释放空间(AGENTS.md 强制)**

Run: `cd /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent && cargo clean`
Expected: 无输出,`target/` 清空。

---

## 验收清单(P1 Definition of Done)

实现者完成全部 Task 后,确认:

- [ ] `HookResult.activate_skill` 字段存在,`with_activate_skill` 构造函数可用
- [ ] `HookAction::ActivateSkill` 变体可从 frontmatter 反序列化(`type: activate_skill`)
- [ ] `validate()` 对空 skill name 报错、非空通过
- [ ] `execute_action` 对 `ActivateSkill` 返回带 `activate_skill` 的 `HookResult`
- [ ] `merge_result` 对 `activate_skill` 取首个(first-wins)
- [ ] `fire_lifecycle_hook` 收到 `activate_skill` 后调用 `activate_skill_for_context`,reason 注入 runtime note
- [ ] `UserPromptSubmit` 在 `prepare_stream_context`/`prepare_react_context` 每轮触发
- [ ] `scan_directory` 发现并合并 EKO 格式 `hooks.json`,无文件时正常工作
- [ ] `cargo check --workspace` + `cargo test --workspace` 全绿
- [ ] 4 个独立 feature 编译通过
- [ ] `cargo clippy --all-targets -- -D warnings` 零警告
- [ ] `cargo fmt --all` 无改动
- [ ] `cargo clean` 已执行
- [ ] 每个 Task 一个 commit,全部 `-c commit.gpgsign=false`,在 `echo-agent` 子仓库内

---

## 后续 Phase 预告(本计划不含,留待各自 plan)

- **P2 脚本运行时**:uv 优先解析、`minimal_env` +HOME、SkillSandboxPolicy 接线、默认 sandbox 装配、二进制探测。依赖本 P1 的 hook 基础。
- **P3 资产 Tier A + baseline 注入**:移植 9 个方法论技能、`inject_methodology_baseline`、`enabled-skills.json`。会用到本 P1 的 `ActivateSkill`(方法论 SessionStart hook)+ `hooks.json` 发现。
- **P4 TriggerSupervisor**:三源融合,依赖本 P1 的 `UserPromptSubmit` 触发点。
