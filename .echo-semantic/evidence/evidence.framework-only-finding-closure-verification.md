---
schema_version: 1
id: evidence.framework-only-finding-closure-verification
kind: evidence
observed_at: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
source_refs:
  - src/agent/snapshot.rs
  - src/agent/react/run/phases/finalize.rs
  - src/trace/mod.rs
  - echo-state/src/audit/mod.rs
  - echo-state/src/memory/file_conversation.rs
  - echo-state/src/memory/sqlite_conversation.rs
  - echo-orchestration/src/tasks/runtime_executor.rs
  - src/agent/subagent/control.rs
  - src/agent/subagent/executor.rs
  - docs/adr/0053-trace-audit-persistence-visibility.md
  - docs/adr/0056-durable-transcript-projection-settlement.md
  - docs/adr/0058-task-claim-subagent-attempt-control.md
  - docs/adr/0051-extract-sdk-repository.md
supports:
  - finding.transcript-projection-settlement
  - finding.diagnostic-persistence-failure-visibility
  - finding.task-subagent-attempt-link
  - behavior.context-memory-lifecycle
  - behavior.observation-persistence
  - behavior.task-subagent-execution
  - rule.context-persistence-separation
  - rule.fact-projection-separation
  - rule.task-subagent-authority
limitations:
  - Issues 55 and 84 remain open for source-backed framework defects and are not supported as resolved by this Evidence
  - SDK Host, EKO, language SDK, website and A2A are outside this framework-only verification scope
---

# Framework-only Finding closure 验证证据

## 支持的结论

当前 framework main 的 transcript projection、Trace/Audit diagnostic delivery 与
TaskClaim-derived Subagent attempt 三条生产路径已分别进入 main，并保持唯一 durable authority、
typed terminal、stale fence 与 consumer projection 分离。本 Evidence 只重审 framework 完成边界，
不把 consumer adoption 当作 repair 或 verification。

## 来源与范围

本 Evidence 复用已经进入 framework main 的三组 repair，并在当前治理分支上重新验证直接生产
路径、public facade、正式文档与 executable examples。SDK Host、EKO、语言 SDK、website 与 A2A
没有参与命令、状态判断或关闭条件。

## Focused 验证

以下命令在同一 source snapshot 上执行并退出 0：

- transcript Agent 路径：26 passed；
- File/SQLite RuntimeState 路径：62 passed；
- File/SQLite ConversationStore 路径：55 passed；
- Trace 路径：62 passed；
- Audit 路径：23 passed；
- diagnostic delivery 路径：4 passed；
- Task runtime exact attempt 路径：34 passed；
- Subagent lib：885 passed；public facade：10 passed；
- MCP focused：99 passed；Scheduler focused：52 passed，用于确认 #55/#84 的已完成子范围；
- root doctests：50 passed、22 opt-in ignored；documentation contract：12 passed；
- `demo06_mcp`（启用 `mcp` feature）与 `demo70_scheduler` 编译通过。

第一次直接检查 `demo06_mcp` 未启用其 manifest 声明的 `mcp` feature，Cargo 在编译前以
exit 101 拒绝；使用示例要求的 feature 重跑后退出 0。该结果是命令调用修正，不是产品失败。

## 远端 main 依据

当前基线 `main@e15cc17f98940f4423231a31d513baa9a6109b51` 的 Rust CI run
`35464088695` 为 success，七个 job 全部通过：Linux quality、framework/foundations/tools/learning
分组测试、Windows compile/atomic replacement 与 dependency policy。

## 完整门禁

最终治理候选执行 `./scripts/verify.sh`，exit 0，无截断、无 warning/error。该命令完成 fmt、
两项 workspace Clippy、all-target/all-feature tests 与 no-default lib check。主要测试汇总包括：
root lib 1055、orchestration 399、state 325、integration 258、execution 427、tools 192，全部
0 failed；仅保留仓库既有 opt-in ignored tests。

随后逐项执行 `cargo check -p echo_agent --no-default-features --features <feature> --locked`，
`acp`、`a2a`、`mcp`、`lsp`、`sqlite`、`telemetry`、`topology`、`subagent`、`web`、`media`、
`data`、`statistics`、`channels`、`git`、`database`、`rag`、`chart` 共 17 项全部 exit 0。

最终 strict semantic snapshot 与 base change-evidence 在相同 source digest 上独立执行；结果由
semantic verification 与 current-diff rereview 共同收口。

## 已知缺口

`#55` 保留 preparation/construction Drop cleanup owner 缺口；`#84` 保留 public CronTaskStore
mutation 绕过 runner cache 同步缺口。两者不再包含外部仓库 blocker。`#106/#46/#99` 的
framework repair、focused scenarios、完整门禁、feature matrix、文档和语义证据已闭合。
