---
schema_version: 1
id: asset.runtime-state-store
kind: asset
title: RuntimeStateStore 与 AgentCheckpoint
asset_type: state_authority
status: active
risk: high
observed_at: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
boundary_refs:
- boundary.context-memory
- boundary.observation-persistence-delivery
code_refs:
- src/state/mod.rs
- src/state/file.rs
- src/state/sqlite.rs
- src/agent/snapshot.rs
- src/state/current_native_recovery_tests.rs
consumer_refs:
- src/agent/react/run/context.rs
- src/agent/react/run/phases/compact.rs
- src/agent/react/run/phases/tools.rs
- src/agent/react/run/phases/finalize.rs
- docs/en/03-memory.md
behavior_refs:
- behavior.context-memory-lifecycle
- behavior.observation-persistence
rule_refs:
- rule.context-persistence-separation
- rule.fact-projection-separation
evidence_refs:
- evidence.agent-context-execution
- evidence.persistence-observation
- evidence.transcript-projection-settlement-repair
- evidence.transcript-projection-settlement-verification
- evidence.managed-import-generation-repair
- evidence.checkpoint-plan-authority-repair
- evidence.checkpoint-plan-authority-verification
- evidence.current-control-history-before-source
- evidence.current-control-history-retirement
- evidence.current-control-history-retained-contracts-equivalence
finding_refs:
- finding.transcript-projection-settlement
candidate_refs: []
---

# 当前两文件闭合状态

原未改validate_task在正常repair路由切换后实际返回errors=[]，SourceSnapshot为`task:ae377fd33de65865359fb7bbd921b28241b036cb8beccabc815d45ddb4d88815`。Eq.after_revision取此真实值；当前Finding仅本两D为resolved，R1—R9有限共同不变量与T1—T6取消／C1—C4 after-only分列，原Git684 before39职责与source-backed rereview保原。旧20路径Finding/Eq无修改，外部工具合同、真实unknown、SDK/newpin/native/Git/mainCI仍独立。本状态是已取得的Task结构与变更依据检查，不预填尚未执行的最终Source/final。

# 采用时点记录（Task闭合前）

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

本实体仅两文件；Git684 before39职责静态分流、R1—R9有限共同不变量、T1—T6取消和C1—C4/Source8 after-only不混同。B336/B1821/T1、U8 F1/F3未独立实跑、未知JSON无新增永久成功CAS合同等限制保原。Eq.after_revision目前只绑定真实source；正常Task report返回后才填task值，Finding仍open，尚不宣称Source/final通过。

# 原有限材料与历史证据原文

# RuntimeStateStore 与 AgentCheckpoint

## 资产身份

ReAct runtime checkpoint、revisioned pending transcript intent、scope lineage、generation tombstone 与
managed clear/delete retirement manifest 的持久权威。

## 来源与消费者

ReactAgent hydration/finalization 消费，file 与 optional SQLite 提供 backend。

## 生命周期

Load/hydrate、pending prepare/retry/proof-ack、rotate/clear incarnation、scope retirement 与 stable
conversation delete saga。Attempt result 与 dispatch 归属由 CAS revision 约束。
跨 managed import epoch 的 checkpoint CAS 还要求完整导入请求、Store 回执、消息和游标一致。

## 候选关系

不替代 ConversationStore、Task graph 或 Trace。

## 未知与限制

当前已按批准NoCompat移除checkpoint的`current_plan`旧字段，任务图仍由TaskRevisionService
持有。File／SQLite与raw CRUD菜单保留；当前typed恢复控制验证CAS、ACK、取消和owner，
旧控制历史兼容源的退出单独记录，不把取消的旧读取义务冒作等价或已完成全局审查。

Current typed owner的scope/version/epoch/replay、checkpoint/cursor、pending/ACK与原call_context/deadline沿原kernel检查。normal load可reconcile，不等于旧readonly inspection。旧B936 raw归档proof退出，不新增任意unknown ownerJSON永久成功CAS保留合同；ConversationStore消费者扩展payload完整digest继续是独立承诺。
