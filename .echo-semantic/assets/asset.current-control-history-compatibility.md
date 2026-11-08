---
schema_version: 1
id: asset.current-control-history-compatibility
kind: asset
title: 已批准退出的旧控制历史兼容读取
asset_type: file
status: deprecated
risk: high
observed_at: 684ad1d9d0de9bff57e27e4c04f04232c6af9f1f
boundary_refs:
- boundary.agent-session-turn
code_refs:
- src/state/legacy_control_history.rs
- src/state/legacy_control_history/pending_recovery.rs
consumer_refs:
- src/state/mod.rs
- src/agent/snapshot.rs
- src/agent/react/run/context.rs
behavior_refs:
- behavior.agent-turn-lifecycle
rule_refs:
- rule.turn-terminal-authority
evidence_refs:
- evidence.current-control-history-retirement
- evidence.current-control-history-before-source
- evidence.current-control-history-retained-contracts-equivalence
finding_refs:
- finding.current-control-history-retirement
candidate_refs:
- asset.agent-execution-contract
- asset.runtime-state-store
---

# 当前两文件闭合状态

原未改validate_task在正常repair路由切换后实际返回errors=[]，SourceSnapshot为`task:ae377fd33de65865359fb7bbd921b28241b036cb8beccabc815d45ddb4d88815`。Eq.after_revision取此真实值；当前Finding仅本两D为resolved，R1—R9有限共同不变量与T1—T6取消／C1—C4 after-only分列，原Git684 before39职责与source-backed rereview保原。旧20路径Finding/Eq无修改，外部工具合同、真实unknown、SDK/newpin/native/Git/mainCI仍独立。本状态是已取得的Task结构与变更依据检查，不预填尚未执行的最终Source/final。

# 采用时点记录（Task闭合前）

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

本实体仅两文件；Git684 before39职责静态分流、R1—R9有限共同不变量、T1—T6取消和C1—C4/Source8 after-only不混同。B336/B1821/T1、U8 F1/F3未独立实跑、未知JSON无新增永久成功CAS合同等限制保原。Eq.after_revision目前只绑定真实source；正常Task report返回后才填task值，Finding仍open，尚不宣称Source/final通过。

# 原有限材料与历史证据原文

# 旧控制历史兼容读取的历史定位

## 资产身份

本对象定位基准 Git684 的两份旧控制历史／断点兼容读取。人已批准退出该兼容承诺，
不代表 generic Store、当前 checkpoint、cold/Fork/import 或 owner-first 恢复能力退出。

## 来源与消费者

code_refs 只按上述真实 Git 读取。原 blob SHA256 分别为
`c01093552354c7b514e32ba837231e82678540bdf1b4c2ef3e50770306bd188f` 和
`83c28732cbfb3180e47422d9b8e87556d029e2dafbdc1be91ab8e1e4186aef19`。
当前调用方继续消费同一 Message、RuntimeStateStore 和 ReAct owner。

## 生命周期

当前版本仍须 CAS、generation、pending/ACK、deadline、真实资源和物理结算证明；
不补成功、不重放副作用，不取消 File／SQLite 的合理公开能力。

## 候选关系

canonical 执行权威仍为 asset.agent-execution-contract，状态归 asset.runtime-state-store。
无新执行器、存储或迁移开关。

## 未知与限制

固定回滚原件保留。旧兼容读取与当前拒绝策略不等价；此前对安装0.5合同的阻断判断保其历史；本轮按真实Git684源trace、R15 actual-after与39职责穷尽分流建立有限R1-R9及独立退出事实，不把取消义务matched。正常source/final与材料增量须实际核，不关闭无关真unknown。
