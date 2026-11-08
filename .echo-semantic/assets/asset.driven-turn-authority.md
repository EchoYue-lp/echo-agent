---
schema_version: 1
id: asset.driven-turn-authority
kind: asset
title: Driven Turn 终态权威
asset_type: state_authority
status: active
risk: high
observed_at: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
boundary_refs:
- boundary.agent-session-turn
code_refs:
- echo-orchestration/src/runtime/turn_driver.rs
- echo-core/src/agent/event_envelope.rs
- src/eval/runner.rs
- docs/adr/0038-eval-trace-correlation-identity.md
consumer_refs:
- src/headless.rs
- src/acp/runtime.rs
- src/eval/runner.rs
- tests/agent_handle_turn_driver.rs
- src/channels.rs
behavior_refs:
- behavior.agent-turn-lifecycle
rule_refs:
- rule.turn-terminal-authority
evidence_refs:
- evidence.agent-context-execution
- evidence.eval-timeout-turn-settlement-repair
- evidence.eval-timeout-turn-settlement-verification
- evidence.eval-trace-correlation-repair
- evidence.eval-trace-correlation-verification
- evidence.factory-background-driven-source-review
- evidence.channel-driven-turn-repair
- evidence.channel-driven-turn-verification
finding_refs:
- finding.turn-driver-entry-coverage
- finding.eval-timeout-settlement
- finding.eval-trace-identity
candidate_refs:
- asset.agent-execution-contract
---

# 当前代码身份更新

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

已采用的三菜单完整角色、caller-owned background与SDK独立迁仓历史关系不改。本次只随当前正式代码稳定后更新正常来源；旧f8观察/原代码源审范围在下文保留，不倒签旧工程或消费者。

# 既有菜单及分范围记录

# Driven Turn 终态权威

## 资产身份

AgentTurnDriver/TurnReceipt 是一次显式 driven invocation 的 envelope sequence、terminal 与计量权威，执行与 sink 投递分别归约；它不取代 raw Agent、TaskRun、Subagent 或 trace Run 的各自合同。

## 来源与消费者

Headless、ACP、Eval、经 ACP 的 SDK、显式 driver consumers，以及标准 AgentChannelHandler 使用该 driver。原 consumer refs 保留，补录 src/channels.rs；标准 Channel 已接管见 evidence.channel-driven-turn-repair/verification 和 audit.channel-driven-turn-rereview 的原范围，本轮 source review 及 R20 具名观察见 evidence.factory-background-driven-source-review。

Raw execute/chat/stream 和 custom MessageHandler 保留独立的底层合同，不因标准 adapter 修复被强制改造成 driver。历史 A2A 是否经过 driver 的观察只保历史；具名退出不产生复活 surface 的验收要求。

## 生命周期

Admit input、commit envelope、drain sink 与原 source、settle Completed/Cancelled/Failed receipt。FinalAnswer 是正文通知，自身不是执行完成；driver 等待同 owner 的真实 terminal 与原 EOF，不能用另一 source 或 EOF/取消补成功。sink 接纳与 producer 结算独立，Delivered 不等于 QQ/飞书/微信后续网络 ACK。

Eval 主 deadline 与 bounded settlement grace 复用同一次 pinned drive；收到结算 receipt 后按原 parent/turn/execution 精确关联 trace，无 receipt 不查询 trace 或补成功。Agent 构造、raw execution 及第三方外部资源关闭仍由原 owner 承担。

## 候选关系

Driver 包装 raw Agent execution，但不替代 TaskRun、SubagentAttempt、trace Run 或协议 TaskState；当前标准 adapter 与 raw API 是不同合同，不要求增加第二 driver 或删除 raw API。

## 未知与限制

旧 observed_at 81e2756cee9127fa23a9bb1023bd56aa8f954964 下“Channel/direct route 尚待审计”是历史观察。标准 Channel 已由原 repair/verification/rereview 接入 driver；本次只纠正将它和 raw/direct 一并列未审的父层文案，不把 raw API 改成失败路径或宣布所有 consumer 统一完成。本次 observed_at 绑定原 source_digest(root) caller 的实际当前输出，不以 R23/R15 manifest 代替机器来源，也不由该身份预授全任务或消费者资格。

Source EOF 不证明第三方自行 spawn 的所有外部任务已退出。本次未验证真实外部 provider/Tool、跨进程强杀、QQ/飞书/微信网络送达、所有第三方 Agent/RunStore、长期并发/Jsonl 性能、当前完整 SDK pin/Host、native 或远端 main；旧验证各保原输入与范围，不由本资产状态接管升级为这些运行资格。
