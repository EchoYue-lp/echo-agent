---
schema_version: 1
id: evidence.eval-trace-correlation-repair
kind: evidence
observed_at: 81e2756cee9127fa23a9bb1023bd56aa8f954964
source_refs:
  - src/eval/runner.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/stream_channel.rs
  - src/trace/mod.rs
  - echo-core/src/agent/event_envelope.rs
  - echo-core/src/tools/mod.rs
  - docs/en/24-eval-system.md
  - docs/zh/24-eval-system.md
  - docs/adr/0038-eval-trace-correlation-identity.md
supports: [behavior.eval-evolution, behavior.observation-persistence, behavior.agent-turn-lifecycle, rule.quality-observation-boundary, rule.fact-projection-separation]
limitations:
  - Agent仍可选择不在配置的RunStore生产trace，零候选保持合法
  - 本修复不改变RunStore retention、export delivery或其它调用方的trace identity
---

# Eval trace correlation 修复证据

## 支持的结论

基准`8332345abfefb1aa23b31f32697e97f0cc7a43d3`在Turn settled后直接把`Agent::current_run_id()`写入EvalResult并作为RunStore key。真实ReactAgent red预置`product-run`，同时在RunStore生成独立`run_<uuid>` trace；旧Eval明确返回product ID并以`EvalResult exposed the product run as a trace run`退出101。

当前Eval将既有每次run UUID同时装配为EventIdentity与ExternalRunContext的formal run、turn和execution correlation。ReactAgent继续分配真实trace Run ID并把correlation写入parent/turn/execution。Settled后Eval通过`list_by_parent_run`和精确tuple筛选唯一summary，再load并二次校验真实Run；只有该Run可设置EvalResult.run_id并进入criteria、constraints和metrics。agent-wide product getter已退出Eval trace路径。

## 来源与范围

`EvalRunner`拥有调用方correlation与结果投影；ReactAgent/RunStore拥有真实trace identity；AgentTurnDriver只提供settlement。ADR 0038记录OpenTelemetry、OpenAI Agents SDK与Codex依据、三类identity owner、失败语义、兼容和回滚。

## 已知缺口

本修复不增加TurnReceipt、AgentEvent、Agent trait、RunStore trait或SDK字段，也不保证第三方Agent生产trace。Trace export与retention失败、terminal commit/projection顺序和其它adapter identity由独立Finding跟踪。
