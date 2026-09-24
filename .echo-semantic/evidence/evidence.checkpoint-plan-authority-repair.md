---
schema_version: 1
id: evidence.checkpoint-plan-authority-repair
kind: evidence
observed_at: source:87b717676a7b51e213630677989947777b4bed441acd8c7d655fb6c96dca77ad
source_refs:
  - src/agent/react/mod.rs
  - src/agent/react/run/context.rs
  - src/agent/snapshot.rs
  - src/state/mod.rs
  - src/state/file.rs
  - src/state/sqlite.rs
  - README.md
  - README.zh.md
  - CHANGELOG.md
  - src/memory.rs
  - docs/adr/0008-canonical-runtime-task-authority.md
supports: [behavior.context-memory-lifecycle, behavior.task-subagent-execution, rule.context-persistence-separation, rule.task-subagent-authority]
limitations:
  - Remote CI and main delivery remain pending
---

# Checkpoint plan authority repair

## 支持的结论

基线为 framework `be8cbbc6e95c8082814e3cb5fad9823a7cbf42a9`。
旧 ReAct `plan_state` 只有 reset、legacy checkpoint restore 和新 checkpoint capture，
没有 canonical Task writer。ADR 0008 确认 `TaskRevisionService` 独占 revisioned Task graph
及其版本化 Plan artifact。删除私有状态和 ToolRuntime 快照路径，新 ReAct checkpoint
固定写 `current_plan: None`；读取旧 checkpoint 仍返回其原始公共字段。

File/SQLite 的 serde、SQL column、CAS 比较和 pending transcript proof 仍使用公开
`AgentCheckpoint` 记录；本修复不改变存储格式，也不抹除尚未重写的旧记录。Legacy
value 只有原始读写语义，不再能覆盖 graph revision 或由 ReAct 在下一 safe point 传播。
独立复审发现根 README、memory/snapshot rustdoc 与 Changelog 仍宣称 plan recovery；
这些公共说明已改为 legacy round-trip 合同，并由 documentation contract 防回归。

## 来源与范围

来源为 ADR 0008、ReAct hydration/reset/checkpoint 生产路径及 File/SQLite
RuntimeStateStore 的公共读写合同。变更未删除文件、字段或 Store 选项。

## 已知缺口

若兼容回归失败，可在任务分支撤销本修复提交并回到基线；不得仅恢复 ReAct 的私有
plan writer，否则会重引入第二权威。删除的是字段使用路径而非公开文件或协议字段，
无需 data migration。重启、取消、stale identity、受管 transcript 和 Store proof
测试负责验证此退役切片。完整 framework 门禁与逐 feature matrix 已通过；
独立复审已通过；远端 CI 与 main 交付仍待完成。
