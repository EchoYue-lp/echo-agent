---
schema_version: 1
id: evidence.diagnostic-persistence-failure-visibility-repair
kind: evidence
observed_at: ab3ed7d23f0a3fbe2bb859a7537df2546531239e
source_refs:
  - src/trace/mod.rs
  - src/audit.rs
  - src/agent/react/builder.rs
  - src/agent/react/mod.rs
  - src/agent/snapshot.rs
  - src/agent/react/run/phases/prepare.rs
  - src/agent/react/run/phases/finalize.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/react/run/react_loop.rs
  - src/agent/react/run/stream_channel.rs
  - echo-state/src/audit/mod.rs
  - echo-state/src/audit/file.rs
  - docs/adr/0053-trace-audit-persistence-visibility.md
  - docs/en/27-tracing.md
  - docs/zh/27-tracing.md
supports: [behavior.observation-persistence, rule.fact-projection-separation]
limitations:
  - 修正后的focused tests、main集成、17项feature矩阵、完整门禁与revision-bound rereview已完成，结果见verification evidence
  - Finding因外部SDK inventory未刷新而保持open；框架修复不代表跨仓库全部交付
  - InMemoryAuditLogger poisoned lock成功丢写继续由finding.in-memory-audit-successful-drop追踪
  - Trace/Audit backend error文本脱敏继续由finding.trace-audit-secret-boundary追踪
---

# Trace/Audit persistence failure visibility 修复证据

## 支持的结论

`RunStore::append_event`默认实现不再把缺失run当成功；初始trace save失败不发布新ID并清除
legacy stale ID。append、load、finalize与Audit callback/backend失败形成带occurred_at、record
family、operation、identity与error的`DiagnosticDeliveryFailure`。

Agent producer只向有界dispatcher执行非阻塞`try_send`。默认tracing与用户observer在独立
dispatcher上运行；重入、queue saturation/disconnect、初始化失败与unwind进入saturating
drop counter。该路径不新增Agent terminal，required persistence仍由RunStore/AuditLogger
直接`Result`表达，skill telemetry保持独立best-effort语义。

`FileAuditLogger`用exclusive lease、stable file guard、expected length与`SyncData`实现单一
live authority。缺失文件通过`create_new`创建，现有或并发创建文件不被覆盖；恢复只修复
无换行final tail，完整坏记录失败关闭，path replacement无法让旧扫描结果截断新文件。

## 来源与范围

修复复用既有RunStore、AuditLogger、React trace/audit producer与`echo_core::utils::fs`
identity/durable append原语。ADR 0053记录OpenTelemetry与tracing-appender一手依据、分层、
失败策略和残余边界；中英文tracing文档同步公共observer与drop counter合同。

## 已知缺口

真实断电/`sync_data`故障、`panic=abort`、进程退出、永久阻塞subscriber、同inode等长外部
改写与非配合进程不属于本轮可确定运行证据。外部SDK inventory刷新和分类完成前Finding不得关闭。
