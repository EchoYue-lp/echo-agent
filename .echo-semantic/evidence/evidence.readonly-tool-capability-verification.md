---
schema_version: 1
id: evidence.readonly-tool-capability-verification
kind: evidence
observed_at: bd17c73075d6b3cf8e00877fa0fb10d36694ea54
source_refs:
  - src/agent/snapshot.rs
  - src/agent/react/builder.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/pipeline.rs
  - echo-orchestration/src/tasks/task_tools.rs
  - src/tools/builtin/cell_tools.rs
  - src/tools/builtin/subagent_message.rs
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - focused tests 由实现者运行，独立 reviewer 仅读取结果并复核源码
  - PR/CI 和远端 main 验证尚未计入本证据
---

# Read-only Tool capability 验证证据

## 支持的结论

自定义 read tool 留在构造与 LLM surface，自定义 write tool 不进入构造结果；
后续批量/替换/trait 注册不能引入 mutating Tool。直接 ToolManager 注入的
write tool 在 LLM view 不可见，手动请求执行被 PlanModeStage 阻断。
`task_list`、`list_cells`、`subagent_list` 仍可见可执行；task mutation、
cell stop、subagent message、会写 recall telemetry 的工具被排除或阻断。

## 来源与范围

在 `cd37e5d3` 源码提交及 `origin/main@733d352f` 合并后的同一源码快照上，
实现者报告：

- `cargo test -p echo_agent --lib readonly_ --locked`：exit 0，3/3。
- `cargo test -p echo_agent --features 'human-loop subagent' readonly_agent_keeps_subagent_list_and_blocks_message --locked`：exit 0，1/1。

最终源码上的 `./scripts/verify.sh`：exit 0；依次覆盖 `cargo fmt --all -- --check`、
workspace all-target/all-feature Clippy、lib/bins panic/unwrap/expect/unreachable Clippy、
workspace all-target/all-feature tests 与 workspace lib no-default-features check。
17 个根 crate 独立 feature 编译检查均 exit 0，覆盖
`acp a2a mcp lsp sqlite telemetry topology subagent web media data statistics channels git database rag chart`。

## 已知缺口

独立 reviewer 未自行重跑测试；完整门禁与独立 feature 检查由主任务运行，
远端 PR/CI 和 main 验证仍待完成。外部自定义工具错误声明 capability 的风险仍由 embedding
application 与 Tool 作者负责。
