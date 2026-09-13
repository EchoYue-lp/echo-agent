---
schema_version: 1
id: evidence.eval-timeout-turn-settlement-repair
kind: evidence
observed_at: 8332345abfefb1aa23b31f32697e97f0cc7a43d3
source_refs:
  - src/eval/runner.rs
  - src/agent/mod.rs
  - src/agent/react/run/mod.rs
  - src/agent/react/run/stream_channel.rs
  - echo-orchestration/src/runtime/turn_driver.rs
  - docs/en/24-eval-system.md
  - docs/zh/24-eval-system.md
  - docs/adr/0036-eval-workspace-generation-lifecycle.md
  - docs/adr/0037-eval-timeout-turn-settlement.md
supports: [behavior.eval-evolution, behavior.agent-turn-lifecycle, rule.quality-observation-boundary, rule.turn-terminal-authority]
limitations:
  - 非协作Agent在共享grace后仍可能继续运行，框架只能隔离并保留其workspace
  - 本切片不改变raw Agent、Channel、A2A或产品adapter的driver覆盖范围
---

# Eval timeout Turn settlement 修复证据

## 支持的结论

基准`29a00f66843263f27503f83247ee8a770b89e913`由Eval私有helper直接消费raw Agent stream；deadline丢弃该future后只调用`cancel()`，随即读取RunStore、评分并返回。ReactAgent的stream producer reaper是detached task，Eval既拿不到JoinHandle，也没有receipt证明其已终止。

当前Eval只创建一次`AgentTurnDriver::drive` future并pin住：主deadline和取消后的bounded grace都借用同一future，不会重启Agent。deadline到达后结果恒为Timeout；收到TurnReceipt才允许读取terminal trace并显式close generation，grace再次超时则记录未settled、跳过trace criteria并retain generation。ReactAgent managed stream会缓存terminal并等待自有producer JoinHandle settled后才释放；terminal前的consumer drop仍进入bounded reaper。Stream reaper与Eval共享一个crate-private 6秒settlement period，避免两套时限语义漂移。

## 来源与范围

`AgentTurnDriver`/`TurnReceipt`保持唯一通用Turn终态权威；`EvalRunner`只拥有deadline、质量结果与workspace disposition。ADR 0037记录Tokio、CancellationToken、OpenAI Codex和Inspect AI依据，以及framework/application分层、兼容和回滚边界。

## 已知缺口

本修复不声称强制终止忽略CancellationToken的第三方Agent，也不为违反terminal-is-last合同后自建的后台任务提供隐式join；不新增后台reaper、公共状态、SDK identity或EKO产品策略。完整workspace合并门禁、远端CI和跨平台真实Tool cancellation仍待任务级交付。
