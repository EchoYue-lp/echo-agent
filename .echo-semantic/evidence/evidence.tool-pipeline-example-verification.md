---
schema_version: 1
id: evidence.tool-pipeline-example-verification
kind: evidence
observed_at: source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74
source_refs:
  - echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs
  - docs/en/02-tools.md
  - docs/zh/02-tools.md
supports: [finding.tool-pipeline-example-drift]
limitations:
  - Remote CI and mainline delivery remain pending
  - Test observes a successful default pipeline call rather than every blocked or custom stage sequence
command_results:
  - { command: "cargo test -p echo-agent-learning --test example_contracts --features testing --locked contract_demo64_stage_inventory_matches_overview", exit_code: 101 }
  - { command: "cargo test -p echo-agent-learning --test example_contracts --features testing --locked contract_demo64_rejects_duplicate_production_stage", exit_code: 101 }
  - { command: "cargo test -p echo-agent-learning --test example_contracts --features testing --locked contract_demo64_rejects_invalid_pre_execution_order", exit_code: 101 }
  - { command: "cargo test -p echo-agent-learning --test example_contracts --features testing --locked contract_demo64", exit_code: 0 }
  - { command: "cargo clippy -p echo-agent-learning --test example_contracts --features testing --locked -- -D warnings", exit_code: 0 }
  - { command: "cargo test -p echo-agent-learning --test documentation_contract --locked", exit_code: 0 }
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
  - { command: "git diff --check", exit_code: 0 }
  - { command: "./scripts/verify.sh", exit_code: 0 }
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
在合入 `origin/main@93ee33d9` 的最终快照上，demo64 为 4 passed、0 failed，
文档合同为 15 passed、0 failed；完整门禁复验了两者。

## 来源与范围

上述结果来自专用分支 `fix/Echoyue/demo64-pipeline-contract`。最初的漂移红测
基于 `origin/main@69a3b864`；当前候选已合入 `origin/main@93ee33d9`。
红测日志位于 Git worktree 状态目录的
`supreme/logs/command-1790315416830.log`，新增重复阶段红测日志是
`command-1790316068568.log`，坏序红测日志是
`issue101-order-red-1790324325741.log`。整合后代码复测、Clippy 与文档合同日志分别是
`issue101-integrated-green-1790324364995.log`、
`issue101-integrated-clippy-1790324394633.log` 和 `issue101-integrated-docs-1790324258113.log`。
最终整合快照 `f55856125b08e3c9fc2e06545bb66d707b8c6eba` 的
`./scripts/verify.sh` 于 2026-09-25 09:03:28–09:20:00 UTC 退出码 0；
日志 `.git/worktrees/demo64-pipeline-contract/supreme/logs/issue101-full-gate-1790327008243.log`
共 379950 bytes、未截断。86 条 `test result` 汇总均为 0 failed，
包含既有 3 条 ignored，并非声称全部测试都实际执行。

## 已知缺口

独立复审和本地完整合并门禁已通过；远端 CI 与 main 交付仍需完成。
