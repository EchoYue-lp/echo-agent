---
schema_version: 1
id: evidence.tool-pipeline-example-verification
kind: evidence
observed_at: source:e2f3b5f8a9af67e4a9534a49815f6da72fb3c17c5834b86ce5221c121838a99f
source_refs:
  - echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs
  - docs/en/02-tools.md
  - docs/zh/02-tools.md
supports: [finding.tool-pipeline-example-drift]
limitations:
  - Full workspace merge gate, remote CI, and mainline delivery remain pending
  - Test observes a successful default pipeline call rather than every blocked or custom stage sequence
  - Documentation contract passed on the integrated base before the final code-only order-test increment; final gate must rerun it
command_results:
  - { command: "cargo test -p echo-agent-learning --test example_contracts --features testing --locked contract_demo64_stage_inventory_matches_overview", exit_code: 101 }
  - { command: "cargo test -p echo-agent-learning --test example_contracts --features testing --locked contract_demo64_rejects_duplicate_production_stage", exit_code: 101 }
  - { command: "cargo test -p echo-agent-learning --test example_contracts --features testing --locked contract_demo64_rejects_invalid_pre_execution_order", exit_code: 101 }
  - { command: "cargo test -p echo-agent-learning --test example_contracts --features testing --locked contract_demo64", exit_code: 0 }
  - { command: "cargo clippy -p echo-agent-learning --test example_contracts --features testing --locked -- -D warnings", exit_code: 0 }
  - { command: "cargo test -p echo-agent-learning --test documentation_contract --locked", exit_code: 0 }
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
  - { command: "git diff --check", exit_code: 0 }
---

# demo64 Tool pipeline executable contract verification

## 支持的结论

修复前，真实默认管线调用的结构化 tracing 读到 17 个阶段，原 demo64
声明 16 个；定向回归以 `left: 17, right: 16` 退出 101，证明测试确实
触达生产顺序，而非仅打印示例自己的数组。独立复审指出同名阶段重复可逃过
首次位置检查；插入第二个 `execute` 的回归先退出 101。集成复审又发现
若干执行前阶段互换仍可通过，基于真实阶段序列构造的坏序回归也先退出 101
（首个反例为 `intervention` 与 `tool_visibility` 互换）。修复后，同一 learning
测试目标的四个 demo64 测试均通过（4 passed、0 failed），包括真实阶段展示、
每项恰好一次，以及执行前、输入守卫、执行及终态观察的关键部分序。定向 Clippy 零警告，
文档合同在合入 `9abdf9de` 后、最终坏序测试增量之前为 14 passed、0 failed；
由于它会扫描 demo64 源码，最终快照仍需在完整门禁中复验。

## 来源与范围

上述结果来自专用分支 `fix/Echoyue/demo64-pipeline-contract`。最初的漂移红测
基于 `origin/main@69a3b864`；当前候选已合入 `origin/main@9abdf9de`。
红测日志位于 Git worktree 状态目录的
`supreme/logs/command-1790315416830.log`，新增重复阶段红测日志是
`command-1790316068568.log`，坏序红测日志是
`issue101-order-red-1790324325741.log`。整合后代码复测、Clippy 与文档合同日志分别是
`issue101-integrated-green-1790324364995.log`、
`issue101-integrated-clippy-1790324394633.log` 和 `issue101-integrated-docs-1790324258113.log`。

## 已知缺口

目前仅为候选源码的 focused 证据；独立复审已通过，合并前全量
门禁、远端 CI 和 main 交付仍需完成。
