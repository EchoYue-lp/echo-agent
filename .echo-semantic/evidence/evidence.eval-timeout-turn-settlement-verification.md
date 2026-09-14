---
schema_version: 1
id: evidence.eval-timeout-turn-settlement-verification
kind: evidence
observed_at: 8332345abfefb1aa23b31f32697e97f0cc7a43d3
source_refs:
  - src/eval/runner.rs
  - src/agent/react/run/stream_channel.rs
  - echo-orchestration/src/runtime/turn_driver.rs
  - docs/en/24-eval-system.md
  - docs/zh/24-eval-system.md
  - docs/adr/0037-eval-timeout-turn-settlement.md
  - echo-agent-learning/tests/documentation_contract.rs
supports: [behavior.eval-evolution, behavior.agent-turn-lifecycle, rule.quality-observation-boundary, rule.turn-terminal-authority]
limitations:
  - 完整workspace合并门禁与远端CI尚未执行
  - 可响应测试使用确定性Agent，不替代真实provider、Tool或跨进程取消验收
---

# Eval timeout Turn settlement 验证证据

## 支持的结论

有效red在旧实现上以`Eval returned before cancellation settlement`退出101：可响应Agent收到cancel后写marker、发送oneshot并发出Cancelled，而旧Eval在1.21秒先返回。修复后同一测试命中1项并通过，且要求`Turn settled after timeout: cancelled`和已删除workspace。

不响应取消测试命中1项并在约7秒后通过，要求`Turn did not settle within`、duration至少覆盖共享grace、result保持Timeout、reported generation仍存在，并以带run ID的CountingRunStore证明load次数为0。Late Completed测试证明deadline后final answer仍保持Timeout、criteria metrics为空、score为0、stream只创建一次且settled generation已删除。Managed React stream测试在terminal已入channel而producer仍受控阻塞时保持Pending，producer settled后再精确释放一个terminal；producer在已缓存success terminal后被abort时只返回一次stream error并随后EOF。完整stream_channel 46 tests、EvalRunner 11 tests、Eval 23 tests、Improve 17 tests、TurnDriver 22 tests和documentation contract 5 tests均通过。

## 来源与范围

工程日志位于`.supreme/logs/plan15-*`。`eval,improve` all-target Clippy以`-D warnings`通过；lib/bin panic-policy Clippy通过；`--no-default-features --features eval` check通过；`cargo fmt --all -- --check`通过；`contracts/sdk`与`sdks/shared`相对`29a00f66`零diff。

## 已知缺口

当前证据证明bounded settlement、timeout结果保留、late Completed不越过deadline、unsettled trace/criteria gate、React producer终态释放与失败覆盖、settled cleanup和unsettled retain，不证明操作系统级强杀或第三方Agent自建后台任务已停止。三轮独立复审最终0 findings；完整workspace合并门禁、远端CI与远端main交付尚未完成。
