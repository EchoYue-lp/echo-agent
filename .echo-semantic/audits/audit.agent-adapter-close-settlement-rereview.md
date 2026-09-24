---
schema_version: 1
id: audit.agent-adapter-close-settlement-rereview
kind: audit
boundary_ref: boundary.agent-session-turn
lens: time_lifecycle
freshness: examined
revision: 733d352fc719f922b21bab1cd46206139564367f
finding_refs: [finding.agent-adapter-close-settlement]
challenges:
  react-turn-close-debt:
    revision: 733d352fc719f922b21bab1cd46206139564367f
    source_refs: [src/agent/react/lifecycle.rs, src/agent/react/run/react_loop.rs, src/agent/react/run/stream_channel.rs, src/agent/react/mod.rs]
    evidence_refs: [evidence.agent-adapter-close-settlement-repair, evidence.agent-adapter-close-settlement-verification]
  headless-and-driver-terminal:
    revision: 733d352fc719f922b21bab1cd46206139564367f
    source_refs: [src/headless.rs, echo-orchestration/src/runtime/turn_driver.rs]
    evidence_refs: [evidence.agent-adapter-close-settlement-repair, evidence.agent-adapter-close-settlement-verification]
  protocol-adapter-close-owners:
    revision: 733d352fc719f922b21bab1cd46206139564367f
    source_refs: [src/acp/adapter.rs, src/acp/session.rs, echo-integration/src/channels/manager.rs, echo-integration/src/channels/session.rs]
    evidence_refs: [evidence.agent-adapter-close-settlement-repair, evidence.agent-adapter-close-settlement-verification]
---

# Agent adapter close settlement 独立复审

## 审查范围

独立 reviewer 在 `main@69dd0e85` 的 Issue #36 worktree 上读取完整 framework diff、ADR 0066、
ReactAgent direct/stream lifecycle、Headless、ACP、ChannelManager/Session、TurnDriver、双语文档、
examples、focused tests、完整 gate 与 semantic evidence。A2A、SDK、CLI、website 被明确排除。

## 已检查故障假设

检查了 close 早于 active/queued Turn、caller token 取消 sibling、queued admission 在 close 后进入、
preparation/producer panic 或 abort 被 Drop 伪装成 settlement、close waiter 取消丢失 owner、Headless
task 首次 poll 前 runtime shutdown、close retry 提前关闭 active run、typed cancellation 被改写为
Failed、transport stop 与 handler close 重试 phase 混淆，以及正常 stop 后虚报 cleanup debt。

## 实际实现路径与证据

`ReactAgentCloseAuthority` 在 execution mutex admission 后、任何可观察 preparation await 前建立
settlement obligation；显式 terminal/failure path 释放 lease，异常 Drop 记录 persistent debt并阻断
MCP cleanup。Headless 同步返回 retained handle，owned task 发布结果并保留失败 close owner。
ACP 使用既有 Session/Run receipt。ChannelManager 单独保存 transport-stop phase，handler retry 不再
依赖 `ChannelPlugin::stop` 幂等。TurnDriver 保持 execution terminal 与 sink delivery 分离。

## 问题记录

前三轮实现复审先后发现 React close 未覆盖 Turn、Headless pre-poll owner、caller token 反向取消、
retry phase、异常 Drop 假 settlement 与 preparation 早期 await 窗口；逐项修复后定向复审通过。
最终复审又发现 Channel transport-stop phase 未保存、Drop warning 误报以及两个 canonical semantic
对象仍绑定旧快照。修复后增量复审结论为 pass，Critical 0、Important 0、Minor 0。

## 验证

最终 `./scripts/verify.sh`、17-feature matrix、formatter、diff check、两档 Clippy、no-default check、
strict semantic snapshot/change-evidence 全部通过。

## 残余风险

异常终止后的 persistent React close debt 不会被自动清除；这是 fail-closed 事实，调用方必须
保留并报告 owner。A2A 保持独立开放 Findings。

## 未检查项

未执行真实第三方 MCP/IM provider、SDK Host、EKO、三语言 SDK 或 website 验收；它们不属于
framework Finding #36 的完成边界。远端 PR/main CI 与 post-merge strict 由 delivery 阶段补充。
