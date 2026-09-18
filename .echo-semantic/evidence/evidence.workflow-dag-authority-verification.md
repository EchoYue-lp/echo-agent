---
schema_version: 1
id: evidence.workflow-dag-authority-verification
kind: evidence
observed_at: d75bd27b1cee425fad4fdfa20c99f528d925ff8d
source_refs:
  - echo-agent-learning/tests/documentation_contract.rs
  - docs/adr/0059-task-workflow-dag-authority.md
  - docs/en/09-tasks.md
  - docs/zh/09-tasks.md
  - docs/en/17-graph-workflow.md
  - docs/zh/17-graph-workflow.md
supports: [behavior.task-subagent-execution]
limitations:
  - 文档合同只保证真实入口存在且双语文档引用ADR，不证明每个运行时场景的行为正确
  - 严格语义快照需在整合时刷新历史source digest，当前还不是闭合证据
  - 完整workspace门禁、独立复审与远端main交付尚未执行
---

# Task 与 Workflow 图边界定向验证证据

## 支持的结论

`cargo test -p echo-agent-learning --test documentation_contract
task_and_workflow_authorities_are_documented_against_public_entries --locked` 执行 1 项，
通过 1 项、失败 0 项。合同读取真实 `RevisionedTaskGraph`、`TaskRevisionService`、
`RuntimeTaskService`、`Graph::execute_loop`、`DagWorkflow`/`Workflow` 实现入口，检查
ADR 0059 的权威术语及中英文 Task/Workflow 两章的 ADR 导航，逐一验证本地链接。

`cargo fmt --all` 已写入，`cargo fmt --all -- --check` 与 `git diff --check` 在本 lane
提交前再次执行。既有 `echo_orchestration` 的 Task/Workflow runtime 测试未因纯文档
决策改变；这轮未运行其完整集合，也不把文档合同结果外推为行为等价。

## 来源与范围

在独立 worktree `doc/Echoyue/issue-111-task-workflow` 上基于远端 main
`0415ba15eb8d348f357fe55df4448897677e6960` 验证，Cargo 首次编译约 7 分 09 秒；
只覆盖 Issue #111 的 authority contract 与文档链接。当前 `semantic strict-snapshot`
因新增文档使 main 的历史 source digest 与未提交工作树不一致，且既有 #99 外部
control Evidence 的 `source:` 引用需整合时刷新可恢复快照；它不是通过项。

## 已知缺口

尚缺 independent rereview Audit、完整 merge gate、远端 PR/CI 和 main 合入。
