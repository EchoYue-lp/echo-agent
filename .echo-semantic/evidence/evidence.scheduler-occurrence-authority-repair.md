---
schema_version: 1
id: evidence.scheduler-occurrence-authority-repair
kind: evidence
observed_at: source:b28c584ece690cbed560ee12938634abe9ccfe9d73658c45a1c4ea5579020735
source_refs:
  - echo-orchestration/src/scheduler/mod.rs
  - echo-orchestration/src/scheduler/cron_task.rs
  - echo-orchestration/src/scheduler/runner.rs
  - echo-state/src/delivery.rs
  - echo-state/src/journal/file.rs
  - docs/adr/0042-scheduler-occurrence-authority.md
supports:
  - behavior.task-subagent-execution
  - rule.task-subagent-authority
limitations:
  - Callback delivery 是 at-least-once，不提供 external effect exactly-once
  - Cron polling 仍为 approximate，不补跑 runner 离线期间的任意 schedule
  - 不提供跨进程 writer 的 compare-and-swap；CronTaskStore 仍是单进程共享锁模型
---

# Scheduler occurrence authority 修复证据

## 支持的结论

`CronTaskStore` 在 load、add、save 和 legacy migration 入口校验非空且唯一的
CronTask ID；重复定义 fail closed，Store backend 已存在的目标值不会被 legacy
文件覆盖。SchedulerRunner 在每次成功 store mutation 后重新加载完整快照，使
`tasks` 只作为 derived cache，不再保留旧的 `last_run` projection。

Tick 保存 `task.id + created_at + scheduled_at` occurrence identity，并将持久
`control_revision` 写入 payload。每次 status control 都在 CronTaskStore 中递增 revision；
callback 创建前在 runner control lock 下重新确认同一任务定义仍存在、为 Enabled 且 revision
未变化，因此enqueue后的disable→enable不能通过ABA。该admission点之后，disable/remove
不撤回已接纳 invocation。callback settlement
用 captured definition identity 更新 last-run，旧定义不能写入被删除后重建的同 ID
任务。`run_once` 仍是显式手动触发路径，但不会绕过 Disabled 状态，并在持久化
成功后刷新 cache；控制/admission gate 只约束 tick 捕获的 scheduled occurrence。

Definition identity 不再由caller可复用的`id + created_at`单独承担。CronTaskStore每次add都
覆盖caller输入并分配fresh `definition_id`；legacy record缺少该字段时仅在未re-add期间回退
`created_at`。因此remove后add exact clone也会得到新incarnation，旧queued occurrence被drop，
旧callback settlement不能更新replacement。

#84 阶段没有新增 scheduler 私有 ledger。`SchedulerRunner` 组合
`echo_state::delivery::DeliveryLedger`、`FileEventJournal` 和 `FileCheckpointStore`；route
为 task ID，payload 保留 definition snapshot、trigger 与 scheduled/requested time。
Scheduled occurrence ID 由版本、task ID、store-owned `definition_id`、legacy `created_at` 和
scheduled time稳定散列，manual occurrence使用UUID。DeliveryLedger继续唯一拥有attempt与
attempt ID。

Occurrence persist、claim 与 `EffectStarted` 均在 callback future 创建前完成 `SyncData`
确认。Callback 已知成功/失败写 terminal settlement；shutdown 或 crash 恢复遇到
`EffectStarted` 时先写 `OutcomeUnknown` retry settlement，再以同一 occurrence ID 和新
attempt ID 重投。`OccurrenceFireFn` 获得 durable context；旧 `FireFn(CronTask)` 只是丢弃
context 的薄 adapter，仍走同一 claim/settlement 路径。Last-run 写入失败不会把已知 callback
结果伪装成 unknown；错误作为 terminal settlement reason 保留。Process-local active-attempt
guard 只检测同进程 future drop/abort，真正恢复事实仍由 `EffectStarted -> OutcomeUnknown`
journal transition 提供，不能被解释成 exactly-once fence。同一路径第二个 live runner
fail closed，避免两个独立 projection 竞争一个 journal。

File-backed store 对完整 definition filename 追加 occurrence suffix，不再用
`set_extension` 产生别名。Store trait 没有可恢复 backend identity，因此 Store-backed
CronTaskStore 必须用既有 `with_path()` 显式提供 stable backend anchor；缺失时 runner
fail closed，不再共享 default-home journal。

## 验证

- 修复前 `admitted_occurrence_is_replayed_immediately_after_restart`：failed，错误为
  `admitted occurrence was not replayed after restart`。
- 首轮修复后 `cargo test -p echo_orchestration scheduler::runner::tests --locked`：
  11 passed；最终 focused 矩阵由 verification Evidence 记录。

## 来源与范围

本 Evidence 覆盖 SchedulerRunner/CronTaskStore 的 definition authority、cache freshness、
control/admission 顺序以及 DeliveryLedger 组合。它不把 callback effect 本身伪装成可回滚
事务；stable occurrence ID 只给 callback 提供幂等依据。

## 已知缺口

Cron polling 本身不补跑进程离线期间的任意 schedule；这需要独立 misfire/deadline policy。
Delivery journal 为 append-only，物理 compaction 是后续运维能力。默认 ledger retention 保持
256 个 terminal record / 256 KiB envelope bytes（先触及的上限生效）；dedup 仅覆盖仍在
projection 中保留的 occurrence，不是永久去重或固定时间窗口。terminal 被淘汰后，即使
append-only journal 仍有历史事件，同一 scheduled identity 再次提交也可能执行；外部 effect
的更长去重期限须由 callback 自行维护。非 terminal occurrence 不受 terminal retention 淘汰。

本轮复审修复：恢复 claim 与已接纳的 EffectStarted 使用同一 journal batch 原子提交，恢复
在 callback 构造前再次中断仍可 reopen 并沿原 occurrence ID 重投。remove/remove_exact/status
在 store mutation 前撤销对应 cache entry；reload 失败或 management future drop 不再留下
可执行的旧 Enabled revision，只有成功权威 reload 才恢复 cache。新增两项 regression 覆盖
disabled/removed recovery 以及三种管理操作的 post-commit reload failure/drop、queued/new work。

迁移保护只覆盖同一 Store
backend 的目标 key 已存在场景；跨进程 CronTaskStore writer 不在本次 closure 内。
新增 public framework identity/API 将触发 SDK inventory drift；按并行 extraction 所有权，本
分支不修改 SDK contracts，Finding 保持open并等待集成分支生成与审核。
