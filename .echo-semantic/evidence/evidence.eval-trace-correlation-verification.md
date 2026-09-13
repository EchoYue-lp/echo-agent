---
schema_version: 1
id: evidence.eval-trace-correlation-verification
kind: evidence
observed_at: 81e2756cee9127fa23a9bb1023bd56aa8f954964
source_refs:
  - src/eval/runner.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/stream_channel.rs
  - src/trace/mod.rs
  - docs/en/24-eval-system.md
  - docs/zh/24-eval-system.md
  - docs/adr/0038-eval-trace-correlation-identity.md
  - echo-agent-learning/tests/documentation_contract.rs
supports: [behavior.eval-evolution, behavior.observation-persistence, behavior.agent-turn-lifecycle, rule.quality-observation-boundary, rule.fact-projection-separation]
limitations:
  - 完整workspace合并门禁与远端CI尚未执行
  - 未执行持久JsonlRunStore并发stress或外部trace exporter验收
---

# Eval trace correlation 验证证据

## 支持的结论

真实ReactAgent red在旧实现上命中1项并exit 101；green返回可load的真实`run_<uuid>`，其parent/turn/execution共享唯一`eval-` correlation，37/5 token metrics来自同一Run，legacy `product-run`保持不变。

两个lookup tests覆盖唯一精确匹配、其它child忽略、零候选、两个精确候选拒绝、list failure、load failure、dangling summary和loaded Run identity mismatch。端到端projection test证明零trace Agent仍可通过output criteria，而RunStore list failure使Eval失败并产生明确violation；真实ReactAgent provider failure仍返回status Failed的诊断trace。Unsettled timeout对list/load计数都为0。

## 来源与范围

工程日志位于`.supreme/logs/plan16-*`。最终focused阶段中EvalRunner 16 tests、Eval 28 tests、Improve 17 tests和documentation contract 5 tests通过；`eval,improve` all-target Clippy及eval no-default check通过。contracts/sdk与sdks/shared应在最终门禁保持相对`8332345a`零diff。

## 已知缺口

当前证据不覆盖第三方RunStore破坏trait外的持久一致性、跨进程export完成或海量trace group性能。独立review最终0 findings；最终semantic change-evidence、SDK零diff和Issue reconciliation仍需与提交前门禁共同使用。
