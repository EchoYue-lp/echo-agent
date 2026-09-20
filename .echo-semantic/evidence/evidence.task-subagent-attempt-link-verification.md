---
schema_version: 1
id: evidence.task-subagent-attempt-link-verification
kind: evidence
observed_at: f310825c418932cde02f661f0686f83f216771d6
source_refs:
  - echo-orchestration/src/tasks/runtime_executor.rs
  - echo-orchestration/src/tasks/runtime_service.rs
  - src/agent/subagent/control.rs
  - src/agent/subagent/executor.rs
  - src/agent/subagent/team/mod.rs
  - tests/facade_smoke.rs
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - PR #133 remote CI passed; current closure evidence records the final framework snapshot
  - Consumer command replay and cross-repository E2E are outside the framework Finding
---

# TaskClaim 与 SubagentAttempt framework 验证证据

## 支持的结论

定向验证证明 TaskClaim-derived identity、精确 control、targeted cancellation、join-to-CAS
observation、恢复顺序和 Team runtime handle 在 framework 内形成一条闭合路径。

## 已验证场景

- claim-derived event/control lineage、physical reclaim identity 与 control-scope isolation；
- pre-reservation、reserved、active、duplicate、stale 与 settled typed interrupt；
- sibling-safe child cancellation、pre-admission cancellation、non-cooperative targeted abort，
  以及 root/exact 并发下的 JoinSet ID 归因；
- join-to-CAS 正常终态、settlement 与 claim lookup 同时失败时的 typed authority error、
  durable recovery 在 controller hook 阻塞时先释放 waiter，以及重复 recovery 无本地状态增长；
- lost settlement response、unknown abandonment、post-CAS cleanup observer；
- 同一 business run 的多个 Team runtime handle 不互相覆盖，active handle 不被历史上限淘汰；
- public facade 对 interrupt receipt/error、cleanup observer 和 Team controller/handle 的可达性。

## 来源与范围

在实现 checkpoint 前后的最终增量上执行并通过：

- `cargo fmt --all -- --check` 与 `git diff --check`；
- `cargo clippy -p echo_orchestration --lib --locked -- -D warnings`；
- `cargo clippy -p echo_agent --lib --features subagent --locked -- -D warnings`；
- `cargo test -p echo_orchestration --locked tasks::runtime_executor::tests::`，34/34；
- `cargo test -p echo_agent --lib --features subagent --locked`，778/778；
- `cargo test -p echo_agent --test facade_smoke --features subagent --locked`，10/10。

合并前完整门禁随后在包含最新 main 的任务分支上全部通过：workspace all-target/all-feature
Clippy、panic-policy Clippy、workspace all-target/all-feature tests、workspace lib no-default check
均为 exit 0；`acp`、`a2a`、`mcp`、`lsp`、`sqlite`、`telemetry`、`topology`、
`subagent`、`web`、`media`、`data`、`statistics`、`channels`、`git`、`database`、
`rag`、`chart` 共 17 个独立 feature check 全部 exit 0。完整测试包含 examples、benches、
learning contracts 与 workspace tests；存在既有 opt-in ignored tests，未出现 failed。

## 已知缺口

PR #133 远端 CI 已通过。Consumer command replay 与跨仓库 E2E 是独立 outcome，不阻塞 #99。
