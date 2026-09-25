---
schema_version: 1
id: evidence.tool-pipeline-example-verification
kind: evidence
observed_at: source:c47c1a2f477aa8d78cd95f13b29119df92f2daf44d22457dcabaece4796c2474
source_refs:
  - echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs
  - docs/en/02-tools.md
  - docs/zh/02-tools.md
supports: [finding.tool-pipeline-example-drift]
limitations:
  - Full workspace merge gate, independent rereview, remote CI, and mainline delivery remain pending
  - Test observes a successful default pipeline call rather than every blocked or custom stage sequence
command_results:
  - { command: "cargo test -p echo-agent-learning --test example_contracts --features testing --locked contract_demo64_stage_inventory_matches_overview", exit_code: 101 }
  - { command: "cargo test -p echo-agent-learning --test example_contracts --features testing --locked contract_demo64_rejects_duplicate_production_stage", exit_code: 101 }
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
首次位置检查；插入第二个 `execute` 的回归先退出 101。修复后，同一 learning 测试目标
的三个 demo64 测试均通过（3 passed、0 failed），包括真实阶段展示与
权限、输入守卫、执行及终态观察的关键顺序检查。定向 Clippy 零警告，
文档合同 14 passed、0 failed。

## 来源与范围

上述结果来自专用分支 `fix/Echoyue/demo64-pipeline-contract`，base 为
`origin/main@69a3b864`。红测日志位于 Git worktree 状态目录的
`supreme/logs/command-1790315416830.log`，新增重复阶段红测日志是
`command-1790316068568.log`。最终代码复测、
Clippy 与文档合同日志分别是 `command-1790316121414.log`、
`issue101-final-clippy-1790316186596.log` 和 `command-1790315660724.log`。

## 已知缺口

目前仅为候选源码的 focused 证据；独立复审、合并前全量
门禁、远端 CI 和 main 交付仍需完成。
