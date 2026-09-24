---
schema_version: 1
id: evidence.scheduler-occurrence-authority-verification
kind: evidence
observed_at: source:d0d70f0819595200d148564ce11cb55de583ba943d7a5554b4c06a0f6dcb8af9
source_refs:
  - echo-orchestration/src/scheduler/mod.rs
  - echo-orchestration/src/scheduler/cron_task.rs
  - echo-orchestration/src/scheduler/runner.rs
  - echo-state/src/delivery.rs
  - echo-state/src/journal/file.rs
  - docs/adr/0042-scheduler-occurrence-authority.md
supports: [behavior.task-subagent-execution]
limitations:
  - 历史命令记录durable occurrence candidate；当前任务分支另附lane-local focused结果
  - 未执行操作系统kill或断电注入；crash通过保留EffectStarted并重开file authority模拟
  - 不证明跨进程CronTaskStore writer线性一致
  - 历史durable occurrence integration/main曾完成workspace、17-feature矩阵、远端CI与shared semantic snapshot；不适用于当前任务分支
  - 当前任务分支完整本地门禁已通过；PR/CI及远端main合入尚未执行
  - 首次crate全量测试出现一次非scheduler workflow lease测试失败；未修改该owner代码
---

# Scheduler occurrence authority验证证据

## 支持的结论

本证据确认当前集成快照中的 durable occurrence、owner-loss replay、control revision、定义代际和取消前置检查均沿同一 framework authority 运行；以下命令结果只针对该集成快照。

## 来源与范围

2026-09-17在`fix/Echoyue/semantic-84-120-55`、基线`b71f03ba`上集成候选
`573ee8b2`。scheduler源码、manifest、示例、ADR与专属evidence从候选复用前均确认
自候选base `c5f76882`至main没有变化。Cargo.lock仅补充echo_orchestration的echo_state
依赖；architecture只修改scheduler依赖行，保留main的SDK提取说明。
未恢复SDK文件，未修改共享semantic maps/assets/behaviors、baseline或generated inventories。

保留候选的durable occurrence、owner-loss replay、control revision、definition incarnation、
cache刷新与Store path隔离测试。另补`cancelled_replay_does_not_construct_callback`：
预先取消runner不得创建callback，已持久occurrence保留Persisted状态。修复在claim前与
control lock获得后检查cancel，不将未开始的effect伪装成OutcomeUnknown。

## 实际命令与结果

以下cargo构建/测试均使用
`CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/target`，
未设置其它profile或jobs override。命令从上述worktree根目录执行。

- 修复前`cargo test -p echo_orchestration scheduler:: --locked`：exit 101，29 passed，
  新增cancelled replay反例失败，callback构造计数实际1、预期0。
- 修复后`cargo test -p echo_orchestration --all-features --locked`首次：exit 101，
  387 passed，scheduler全部30项通过；唯一失败为
  `workflow::checkpoint_store::tests::renewed_file_claim_is_not_recovered_after_original_lease_age`
  (`checkpoint_store.rs:1390`)。该文件未修改。
- 原命令完整重跑：exit 0，388 unit tests passed；12 doctests passed、5既有ignored。
  重跑通过不等于已修复上述workflow时序不稳定性，交集成owner排查。
- `cargo test -p echo_orchestration --locked scheduler::runner::tests::cancelled_replay_does_not_construct_callback -- --exact`：exit 0，1 passed。
- `cargo clippy -p echo_orchestration --all-targets --all-features --locked -- -D warnings`：exit 0。
- `cargo clippy -p echo_orchestration --lib --bins --all-features --locked -- -D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable`：exit 0。
- `cargo check -p echo_orchestration --all-features --locked`：exit 0。
- `cargo check -p echo-agent-learning --example demo70_scheduler --locked`：exit 0。
- `cargo fmt -p echo_orchestration`及`cargo fmt -p echo_orchestration -- --check`：exit 0。
- `git diff --check`：exit 0。

## 分层、重复性搜索与研究

搜索 framework 中的 CronTaskStore、SchedulerRunner、DeliveryLedger 与 OccurrenceFireFn：
framework DeliveryLedger 已有通用 claim/attempt/settlement 权威。
复用ADR 0042的Kubernetes CronJob近似调度与Temporal Activity幂等研究，不重设计ledger。
框架负责 durable occurrence 与恢复；embedding application 负责 data-root、Agent 调用及业务
effect 幂等策略。

## 已知缺口

- 历史 `main@e15cc17f` 上 Public `CronTaskStore` clone 可在 runner 构造后直接
  disable/remove/update，runner cache 不会自动刷新；当前任务分支已用 Store mutation
  owner 拒绝 live runner 外的公开写入。
- Consumer stable data-root、callback idempotency、SDK mapping 与 website 同步由各自 owner 追踪，
  不阻塞 #84。
- Cron offline misfire 与跨进程 Store writer 不属于当前合同。

## 当前任务分支 focused 验证

基于 `origin/main@53628a3e` 的 `fix/Echoyue/issue-84-scheduler-occurrence` 工作树，
调度源码与新测试位于 `echo-orchestration/src/scheduler/{cron_task,runner}.rs`；
同轮还更新 ADR 0042 与双语长期任务文档。
独立 code reviewer 核对 ADR 0042、Store/runner 调用路径、两组文档和测试事实，
返回 0 action items；reviewer 没有自行运行测试。

- 修复前，retained/reanchored Store-backed handles 两个反例失败，live legacy source
  migration 反例失败；修复后均通过。
- 最终 `cargo test -p echo_orchestration scheduler:: --locked`：exit 0，
  58 passed、0 failed，包含 mutation owner、same-path/backend handle、migration、
  caller cancellation 后提交与 runner reopen 回归。
- `cargo fmt --all -- --check`：exit 0；`git diff --check`：exit 0。

## 当前任务分支完整门禁

在最后一次源码修改后，同一工作树执行 `./scripts/verify.sh`，退出码 0。该脚本依次通过
`cargo fmt --all -- --check`、workspace all-targets/all-features Clippy（`-D warnings`）、
lib/bins panic/unwrap/expect/unreachable Clippy、workspace all-targets/all-features 测试，
以及 workspace lib no-default-features 编译。测试输出无失败；示例目标包含在 all-targets
编译范围内。运行时使用 `CARGO_TARGET_DIR` 指向本仓库共享 target，
`CARGO_BUILD_JOBS=2`、`CARGO_INCREMENTAL=0`，并将 dev/test 调试符号设为 0 以控制磁盘占用；
检查项和 feature 组合未改变。

PR CI 与远端 main 合入后的验证尚未执行。旧 integration/main 验证只证明之前的 durable
occurrence 范围。
