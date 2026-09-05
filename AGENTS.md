# echo-agent AGENTS.md

本文件是 AI agent 在本仓库工作时的**最高优先级约束**(优先级高于 agent 默认行为和任何技能),请严格遵守。本仓库可独立检出使用,本文件即完整约束,不依赖外部 superproject。

## 仓库定位

通用 Rust Agent 框架 Cargo workspace:根 crate `echo_agent` + 子 crate `echo_core` / `echo_macros` / `echo_execution` / `echo_integration` / `echo_tools` / `echo_state` / `echo_orchestration` / `echo-agent-learning`。crate 名用下划线(`echo_agent`,`cargo check -p echo_agent`)。

**框架服务所有复用方,不是某个应用的私有库。** 任何"能放框架、也能放应用"的功能,动手前必须先回答分层问题(见下)。

## 统一术语:只有 Subagent,没有 Worker(强制)

产品模型、领域模型、运行时模型和代码术语中都只有 `Subagent`,没有 `Worker` 概念。标准关系是 `TaskRun → PlanTask → SubagentRun`。类型、trait、字段、函数、事件、注释、文档和 UI 文案均使用 subagent 命名;后续迭代中触及遗留 `worker` 命名必须随手迁移。仅第三方协议固定 wire name 可在最小适配边界保留,内部立即转换。

## Rust 编码硬性约束(最高优先级)

### 1. 字符串处理:UTF-8 安全,禁止字节级截断

`str::len()` 是字节数;字节索引切片(`&s[..n]`)在中文/emoji 上会切到字符中间 → **panic**。处理任意文本必须用字符迭代器:

```rust
// 正确(本项目既有 pattern)
let preview: String = s.chars().take(N).collect::<String>();
if s.chars().count() > N { ... }
// 禁止:&s[..100] / &s[100..] / s.len() > 100
```

### 2. 禁止任何会导致系统 panic 的 API

| 禁止 | 安全替代 |
|---|---|
| `.unwrap()` | `.ok_or(...)?` / `unwrap_or(default)` / `unwrap_or_else(...) \| ... \|` |
| `.expect("msg")` | 同上,带明确错误处理 |
| `arr[i]`(可能越界) | `arr.get(i)` + Option 处理 |
| `&s[..n]` 字节切片 | `.chars().take(n).collect()` |
| `"123".parse()` 不处理错误 | `.parse().map_err(...)?` |
| 整数运算可能溢出 | `checked_add` / `saturating_*` / `wrapping_*` |
| `panic!` / `unreachable!` / `todo!` | 返回 `Result` 或处理该分支 |

CI 对 lib/bins 强制 `-D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable`。每一处 unwrap/expect/直接索引都要质疑;只有逻辑上 100% 不可能失败的内部场景才可保留且须注释说明。

## 关键决策:这个功能放框架还是应用?(强制)

- **放框架**(全部满足):任何用 echo-agent 的 agent 都可能需要的通用能力、与产品形态无关、可独立编译测试文档化。不需要已有第二个消费者;"多项目可复用"是语义判断。
- **放应用**(满足任一):只服务于特定产品形态、依赖产品决策(本地优先、交互审批、特定 UI 字段)、换个产品不成立。
- **拿不准默认放应用层**:应用下沉容易,框架污染后清理难(YAGNI)。
- 把应用概念(产品专属并发模型、UI 投影表、approval gate)塞进框架的代价:框架被产品状态机绑架,复用方要先剥掉无关代码。

### 删除框架代码的判定(强制)

删任何 pub API 前,先 grep **整个仓库**确认无构造点/调用点,再判断它是不是 trait 的多实现之一 / 合理对外选项——**是的话,即使当前无调用也保留**(它是框架的能力菜单,不是死代码)。"echo-agent-cli 没用"不构成删除理由。拿不准时保留。被新实现明确取代且覆盖全部能力的旧机制才可删。

## 关键决策:动手前先查"是不是已经有了"(强制)

本仓库代码量大,框架和应用各有任务/存储/调度/工具实现,且历史迭代留过死代码。新增任何字段/函数/类型/模块/工具/表之前:

- 先按名称、概念、相邻命名风格 grep 整个仓库,分清哪个实现是活的、哪个是死的(`#[allow(dead_code)]`、只在 `#[cfg(test)]`、被永远 false 的 guard 挡住 = 死信号)。
- 能复用就不新建,能扩展就不另起。发现重复就归一,发现过时就删。
- 严禁平行实现同一语义:同一种动态 PlanTask 调度、DAG 校验、状态迁移只能有一个权威实现。迁移期可有薄 adapter(只做类型转换/注入,不得重新拥有 ready frontier、DAG 主循环、重试取消),每阶段必须切换至少一条真实主路径并删除被替代逻辑。
- 任务关系只有一个权威 API:框架提供 `task_create/task_update/task_list`;`TaskPlan` 只能是版本化 artifact,`TodoItem` 只能是 UI 投影,不得各自拥有 store、状态机或执行器。

## 关键决策:先调研业界成熟实现(强制)

遇到关键决策(架构选型、状态机设计、API 形状、编排模式、数据流),动手前必须先搜索调研业界实现:必查 Claude Code(agent 编排/plan mode/subagent/工具权限模型)、Codex(事件流/任务生命周期/sandbox)、Cursor/Devin(plan-then-execute、approval gate),按相关度补充。提炼跨系统共性模式再结合本框架定位取舍。方案里要写明"参考了哪些实现、它们怎么做、为何这样取舍"。修 bug、改文案、加测试不需要。

> 反面教训:曾凭直觉把"plan 是否被批准"塞进 run 状态机做出 13 状态,与业界共识相悖,引入系统性 bug。先花 10 分钟查就能避开。

## 分支规范:任务在非 main 分支开发,按任务合并(强制)

- **新任务必须在非 main 分支开发**,分支命名 `<type>/<user>/<任务名>`。`<type>` 是任务事件类型,与 conventional commits 前缀对齐:`feature`(新功能)、`fix`(bug 修复)、`doc`(文档)、`debug`(排查/诊断类调查)、`refactor`(重构)、`test`(补测试)、`chore`(构建/工具/依赖)。如 `fix/Echoyue/journal-race`。worktree 场景见后文。main 保持随时可发布。
- 开发阶段可以细粒度拆分提交,每个 commit 只需通过 focused 快检(见"验证节奏"),**不要求每个 commit 跑全量门禁**。
- **合并粒度 = 任务**:任务完成后,先 `git merge main` 进任务分支,在任务分支上跑下节全量门禁,开 PR 等远端 CI 全绿(或确认本地门禁已全绿),然后 **squash merge 成一个 commit 进 main**,commit message 用与分支一致的类型前缀(如 `fix: ...` / `feat: ...` / `docs: ...`)。门禁或 CI 不绿不得合并;合并后 `git branch -D` 删除任务分支。

## 全量提交门禁(合并到 main 前强制)

在任务分支(已 merge 最新 main)根目录依次执行,**全部通过(零失败、零警告、零 fmt diff)才能合并**;任何失败都必须修复,不允许跳过或绕过:

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo clippy --workspace --lib --bins --all-features --locked -- \
  -D clippy::unwrap_used \
  -D clippy::expect_used \
  -D clippy::panic \
  -D clippy::unreachable
cargo test --workspace --all-targets --all-features --locked
cargo check --workspace --lib --no-default-features --locked
```

等价快捷方式:`./scripts/verify.sh`(同一组命令,本地门禁入口)。

### 条件矩阵(仅触及对应风险面时)

修改 `Cargo.toml`、feature 定义、`#[cfg(...)]` 分支、可选依赖或跨 crate 公共 API 时,额外逐个编译根 crate 的独立 feature:

```bash
for feature in acp a2a mcp lsp sqlite telemetry topology subagent web media \
  data statistics channels git database rag chart; do
  cargo check -p echo_agent --no-default-features --features "$feature" --locked || exit 1
done
```

### 验证节奏

开发阶段(非 main 分支)每个 commit 用 `cargo test -p <crate> <test_name>` / `cargo check -p <crate>` 做 focused 快检即可;完整 workspace 全量门禁只在任务合并到 main 前跑一次。延后全量 ≠ 延后修复:已知失败、编译错误、警告、格式错误必须在当前工作单元修掉。工具 schema budget(16_000 bytes / 4_000 tokens 首轮工具定义)是回归合同,大重构期间不为它提前设计 deferred load;超限记录为 residual,不抬阈值。

## 文档与示例同步(强制)

- 任何代码修改都同步检查 `docs/`、`examples/` 是否需要更新;不适用时在提交说明写明原因,不得静默跳过。
- 架构变更必须记录 ADR(背景、候选方案、决策、取舍、影响),放 `docs/adr/`。
- `examples` 必须纳入可执行的编译或测试链路,防止示例与 API 漂移。
- 注释写设计意图、关键约束和非显而易见逻辑;不写复述代码表面的无效注释。

## Worktree 并行开发

- worktree 放 `.worktrees/`(`.gitignore` 必须含 `.worktrees/`,合并前检查未被删除)。
- 合并前必须先 `git merge main`(merge commit 用 `--no-gpg-sign --no-edit`),验证 main 新改动未丢失,再 squash merge;之后 `git branch -D` 删分支、`git worktree remove --force` + `prune` 清理。
- Cargo.toml 里的绝对路径(worktree 临时的)合并前必须改回相对路径;`grep -rn "worktrees\|/Users/" */Cargo.toml` 必须零命中。

## 磁盘与清理

默认保留 `target/` 增量缓存。仅在:可用磁盘低于约 50 GiB、两个 workspace 的 target 合计超约 60 GiB、缓存损坏或用户要求时 `cargo clean`。

## 提交方式

- 本机 `commit.gpgsign=true` 会失败,提交必须显式关闭签名:`git -c commit.gpgsign=false commit -m "..."`,推送正常 `git push`。
- 注释和 commit message 可用中文;代码风格与周围一致。

## CI 职责边界(不要恢复完整门禁镜像或单一测试 job)

完整 all-feature 门禁由任务分支在合并前执行一次。CI 只补本地 macOS 无法等价覆盖的
信号:Linux all-target/all-feature lint、分组的默认 feature 测试、Windows 编译与原子
文件替换测试、依赖审计。同一 ref 的旧 run 由 concurrency gate 自动取消。

Linux 测试分组是资源下限:单一 workspace test job 在标准 runner 上会 OOM;逐包运行
all-feature 测试也曾令 `echo_agent`/`echo_tools` runner 失联。all-feature 测试属于本地
合并前门禁,CI 的默认 feature 测试只负责 Linux 运行时信号。不得为了远端资源限制放宽
产品测试契约或加入常驻诊断依赖。
