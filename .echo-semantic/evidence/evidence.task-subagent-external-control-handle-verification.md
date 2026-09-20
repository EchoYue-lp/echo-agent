---
schema_version: 1
id: evidence.task-subagent-external-control-handle-verification
kind: evidence
observed_at: 0415ba15eb8d348f357fe55df4448897677e6960
source_refs:
  - src/agent/subagent/executor.rs
  - src/agent/subagent/team/mod.rs
  - tests/facade_smoke.rs
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - PR #134 and main-push remote CI passed; current closure evidence records the final framework snapshot
  - No consumer Host or cross-process command replay behavior is claimed by these framework-focused checks
---

# External Task adapter attempt-control handle 验证证据

## 支持的结论

保留的red test在实现前因public type与constructor缺失而编译失败；实现后同一facade test通过，并验证
固定scope、空scope拒绝、claim-derived queued interrupt disposition与scope-local reconcile。Team完整定向
测试证明default/programmatic manager与pipeline仍使用canonical task runtime。

## 来源与范围

当前快照已执行并通过：

- public facade red/green test：green 1/1；
- Team定向测试：25/25；
- control identity/scope/run isolation：15/15；
- typed projection mapping与exact context字段保留/冲突：3/3；
- React Team non-cooperative targeted abort：1/1；
- same-run multiple Team handles retention：1/1；
- subagent完整lib测试：781/781；
- `cargo clippy -p echo_agent --lib --features subagent --locked -- -D warnings`：exit 0。

包含最新main的任务分支随后通过完整本地门禁：fmt、workspace all-target/all-feature Clippy、
lib/bins panic-policy Clippy、workspace all-target/all-feature tests、workspace lib no-default check均为
exit 0；`acp`、`a2a`、`mcp`、`lsp`、`sqlite`、`telemetry`、`topology`、`subagent`、
`web`、`media`、`data`、`statistics`、`channels`、`git`、`database`、`rag`、`chart`
共17个独立feature check全部exit 0。完整测试包含examples、benches、learning contracts和workspace tests，
仅有既有opt-in ignored tests，无failed。

命令由Supreme run-command保存到该worktree Git状态目录的`supreme/logs/`。测试与实现均使用
同一非语义source snapshot。

## 已知缺口

semantic strict/change evidence、独立 implementation rereview、PR #134 与 main-push remote CI 已通过。
