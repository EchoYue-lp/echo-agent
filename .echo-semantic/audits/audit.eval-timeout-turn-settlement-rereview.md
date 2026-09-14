---
schema_version: 1
id: audit.eval-timeout-turn-settlement-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: time_lifecycle
freshness: examined
revision: 8332345abfefb1aa23b31f32697e97f0cc7a43d3
finding_refs: [finding.eval-timeout-settlement]
challenges:
  single-drive-cancel-settlement:
    revision: 8332345abfefb1aa23b31f32697e97f0cc7a43d3
    source_refs: [src/eval/runner.rs, echo-orchestration/src/runtime/turn_driver.rs]
    evidence_refs: [evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification]
  react-producer-terminal-release:
    revision: 8332345abfefb1aa23b31f32697e97f0cc7a43d3
    source_refs: [src/agent/react/run/stream_channel.rs, src/agent/mod.rs]
    evidence_refs: [evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification]
  timeout-trace-criteria-workspace-order:
    revision: 8332345abfefb1aa23b31f32697e97f0cc7a43d3
    source_refs: [src/eval/runner.rs, docs/adr/0036-eval-workspace-generation-lifecycle.md, docs/adr/0037-eval-timeout-turn-settlement.md]
    evidence_refs: [evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification]
---

# Eval timeout Turn settlement 独立复审

## 审查范围

复审Eval主deadline、cancel与共享grace是否只poll一个drive future，React managed stream的terminal/producer结算，late Completed与unsettled timeout的trace、criteria和workspace顺序，以及双语文档、ADR、语义证据和Issue状态。

## 已检查故障假设

验证deadline后是否重启Agent，cancel request是否被误作terminal，terminal/Err是否领先React producer或重复释放，JoinHandle poll是否漏waker，producer失败是否泄漏已缓存success，grace失败是否读取RunStore、执行criteria或close generation，以及late Completed是否越过deadline转为成功。

## 实际实现路径与证据

Eval只创建一次pinned AgentTurnDriver future，主deadline和6秒grace均借用同一future。收到receipt后仍根据deadline事实保留Timeout；未收到receipt时不load trace、不运行criteria并retain workspace。React managed stream缓存terminal/Err，producer JoinHandle正常结算后才释放；producer failure覆盖已缓存terminal且只返回一次Err，自然settled路径不再触发Drop cancel，提前drop仍由共享grace reaper处理。

Deterministic tests证明responsive Cancelled与late Completed都只构造一次stream；late Completed保持score 0和空criteria metrics；带run ID的unresponsive路径RunStore load为0；producer在terminal缓冲后受控阻塞时consumer保持Pending；producer abort覆盖buffered success并随后EOF。完整stream_channel 46、EvalRunner 11、Eval 23、Improve 17、TurnDriver 22和documentation contract 5 tests通过，fmt、两档Clippy、eval no-default check、SDK零diff与semantic gates通过。

## 问题记录

三轮独立复审依次发现验证矩阵缺口、确认生产状态机并提出producer-failure测试，全部修正后最终Critical、Important、Minor均为0。`finding.eval-timeout-settlement`具备repair、verification与rereview证据，可标记resolved。Issue #48保持open，等待本地提交进入远端main后关闭。

## 残余风险

第三方Agent必须遵守terminal-is-last合同；框架不能强杀忽略CancellationToken或自行创建后台任务的外部实现。React terminal现在等待SessionEnd hook，慢hook会增加终态可见延迟。`current_run_id()`并发trace identity由独立Finding继续跟踪。

## 未检查项

未执行真实provider/Tool取消、跨进程或操作系统级强杀、完整workspace合并门禁、远端CI和其它76个open Finding。
