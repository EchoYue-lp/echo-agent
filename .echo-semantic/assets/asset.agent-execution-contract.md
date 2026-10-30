---
schema_version: 1
id: asset.agent-execution-contract
kind: asset
title: Raw Agent Execution Contract
asset_type: protocol
status: active
risk: high
observed_at: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
boundary_refs:
- boundary.agent-session-turn
code_refs:
- echo-core/src/agent/mod.rs
- echo-core/src/tools/mod.rs
- src/lib.rs
- src/agent/react/mod.rs
- src/agent/react/lifecycle.rs
- src/agent/handle.rs
- src/headless.rs
- src/eval/runner.rs
- docs/adr/0038-eval-trace-correlation-identity.md
- docs/adr/0066-agent-adapter-close-ownership.md
- src/testing/mock_agent.rs
consumer_refs:
- src/channels.rs
- src/acp/runtime.rs
- echo-orchestration/src/runtime/turn_driver.rs
behavior_refs:
- behavior.agent-turn-lifecycle
rule_refs: []
evidence_refs:
- evidence.agent-context-execution
- evidence.eval-timeout-turn-settlement-repair
- evidence.eval-timeout-turn-settlement-verification
- evidence.eval-trace-correlation-repair
- evidence.eval-trace-correlation-verification
- evidence.agent-adapter-close-settlement-repair
- evidence.agent-adapter-close-settlement-verification
- evidence.current-control-history-before-source
- evidence.current-control-history-retirement
- evidence.current-control-history-retained-contracts-equivalence
finding_refs:
- finding.turn-driver-entry-coverage
- finding.a2a-terminal-authority
- finding.eval-trace-identity
candidate_refs:
- asset.driven-turn-authority
---

# 当前两文件闭合状态

原未改validate_task在正常repair路由切换后实际返回errors=[]，SourceSnapshot为`task:ae377fd33de65865359fb7bbd921b28241b036cb8beccabc815d45ddb4d88815`。Eq.after_revision取此真实值；当前Finding仅本两D为resolved，R1—R9有限共同不变量与T1—T6取消／C1—C4 after-only分列，原Git684 before39职责与source-backed rereview保原。旧20路径Finding/Eq无修改，外部工具合同、真实unknown、SDK/newpin/native/Git/mainCI仍独立。本状态是已取得的Task结构与变更依据检查，不预填尚未执行的最终Source/final。

# 采用时点记录（Task闭合前）

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

本实体仅两文件；Git684 before39职责静态分流、R1—R9有限共同不变量、T1—T6取消和C1—C4/Source8 after-only不混同。B336/B1821/T1、U8 F1/F3未独立实跑、未知JSON无新增永久成功CAS合同等限制保原。Eq.after_revision目前只绑定真实source；正常Task report返回后才填task值，Finding仍open，尚不宣称Source/final通过。

# 原有限材料与历史证据原文

# Raw Agent Execution Contract

## 资产身份

Agent trait、ReactAgent 与 AgentHandle 提供 execute/chat/stream/cancel/close 的原始 framework execution contract，不自动生成 TurnReceipt。

## 来源与消费者

Channel、ACP与直接Rust callers消费当前typed execution；A2A已按具名批准退出，
不再列作当前consumer。Eval继续消费AgentTurnDriver，各adapter声明自身Turn终态边界。
MockAgent是公开合同的测试消费者，须实现同一typed primitive和自身真实Status，不能由默认
wrapper把FinalAnswer／EOF补成成功；String便利形式仍沿相同source归并。

## 生命周期

Construct/configure Agent、value-scoped runtime identity、execute/chat/stream、cancel、close；ReactAgent
close fence admission并等待既有Turn terminal，managed stream terminal不领先于其自有producer
settlement；preparation或producer异常Drop形成persistent close debt并阻断资源释放。product
correlation不替代producer-owned trace ID，具体Invocation resource和effect由相关运行边界结算。

## 候选关系

可由 AgentTurnDriver 包装为 driven invocation，但 raw contract 与 driver terminal authority 不是同一状态权威。

## 未知与限制

标准Channel使用原driver；raw公共API仍是独立Framework菜单。A2A具名退出仅保历史Finding，不产生恢复surface或删除合理低层API的要求。R1-R9有限保留和T/C分流由新证据单列，不声明SDK/native/全平台资格。
