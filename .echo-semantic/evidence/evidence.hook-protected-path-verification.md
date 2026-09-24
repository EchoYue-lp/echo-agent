---
schema_version: 1
id: evidence.hook-protected-path-verification
kind: evidence
observed_at: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
source_refs:
  - src/agent/react/run/pipeline.rs
  - echo-orchestration/src/human_loop/service.rs
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - focused tests 由实现者运行，独立 reviewer 仅读取结果并复核源码
  - PR/CI 和远端 main 验证尚未计入本证据
---

# Hook protected-path 验证证据

## 支持的结论

原故障反例覆盖无 Hook、PreToolUse Allow 与 PermissionRequest Allow：三条路径
均拒绝 `.env` effect，并检查 protected-path 审计恰好一条。确定性的
PreToolUse programmatic Hook 将普通输入重写到 `.env` 且附带 Allow 时，
执行计数保持零，拒绝审计仍恰好一条。原外部 shell Hook 夹具在
all-feature workspace 负载下超时，可能让原始普通输入继续执行，因此已替换为
同一 pipeline 上不依赖外部进程调度的 Hook 反例。
PermissionService handler 将普通输入改写到 protected path 时，也对有效输入
拒绝并仅写一次审计。测试不以 Hook 直接 effect 的策略作为结论。

## 来源与范围

在 `cd37e5d3` 源码提交及 `origin/main@733d352f` 合并后的同一源码快照上，
实现者报告：

- `cargo test -p echo_agent --features human-loop hook_allow_ --locked`：exit 0，2/2。
- `cargo test -p echo_orchestration handler_rewrite_protected_path_audits_effective_input_once --locked`：exit 0，1/1。
- `cargo test -p echo_orchestration human_loop::service --locked`：exit 0，21/21。
- 测试夹具改为确定性 programmatic Hook 后，
  `hook_allow_rewrite_to_protected_path_is_denied_and_audited_once` 定向重跑：exit 0。

最终源码上的 `./scripts/verify.sh`：exit 0；依次覆盖 `cargo fmt --all -- --check`、
workspace all-target/all-feature Clippy、lib/bins panic/unwrap/expect/unreachable Clippy、
workspace all-target/all-feature tests 与 workspace lib no-default-features check。
17 个根 crate 独立 feature 编译检查均 exit 0，覆盖
`acp a2a mcp lsp sqlite telemetry topology subagent web media data statistics channels git database rag chart`。

## 已知缺口

独立 reviewer 未自行重跑测试；完整门禁与独立 feature 检查由主任务运行，
远端 PR/CI 和 main 验证仍待完成。
