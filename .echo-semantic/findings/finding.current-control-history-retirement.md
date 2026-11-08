---
schema_version: 1
id: finding.current-control-history-retirement
kind: finding
type: consolidation_candidate
status: resolved
severity: high
primary_focus: contract_evidence
focus:
- state_authority
- data_durability
- time_lifecycle
- failure_concurrency
boundary_ref: boundary.agent-session-turn
behavior_refs:
- behavior.agent-turn-lifecycle
rule_refs:
- rule.turn-terminal-authority
evidence_refs:
- evidence.current-control-history-before-source
- evidence.current-control-history-retirement
- evidence.current-control-history-retained-contracts-equivalence
audit_refs:
- audit.current-control-history-retirement-rereview
decision_refs: []
repair_evidence_refs:
- evidence.current-control-history-retirement
verification_evidence_refs:
- evidence.current-control-history-retained-contracts-equivalence
rereview_audit_refs:
- audit.current-control-history-retirement-rereview
discovered_at: 684ad1d9d0de9bff57e27e4c04f04232c6af9f1f
candidate_asset_refs:
- asset.current-control-history-compatibility
- asset.agent-execution-contract
- asset.runtime-state-store
canonical_asset_ref: asset.agent-execution-contract
replacement_refs:
- asset.agent-execution-contract
- asset.runtime-state-store
delete_paths:
- src/state/legacy_control_history.rs
- src/state/legacy_control_history/pending_recovery.rs
rollback_ref: 684ad1d9d0de9bff57e27e4c04f04232c6af9f1f
decision: retire
---

# 当前两文件闭合状态

原未改validate_task在正常repair路由切换后实际返回errors=[]，SourceSnapshot为`task:ae377fd33de65865359fb7bbd921b28241b036cb8beccabc815d45ddb4d88815`。Eq.after_revision取此真实值；当前Finding仅本两D为resolved，R1—R9有限共同不变量与T1—T6取消／C1—C4 after-only分列，原Git684 before39职责与source-backed rereview保原。旧20路径Finding/Eq无修改，外部工具合同、真实unknown、SDK/newpin/native/Git/mainCI仍独立。本状态是已取得的Task结构与变更依据检查，不预填尚未执行的最终Source/final。

# 第一份Task报告绑定过程记录

原未改validate_task在23份采用后实际返回task:ae377fd33de65865359fb7bbd921b28241b036cb8beccabc815d45ddb4d88815。本Finding由已经审阅的Git684 before39职责分流、R1—R9有限共同不变量、当前formal full05具名after、原source-backed rereview承接；不是新增before运行或全行为相同。当前仅按本次两删除路径声明resolved，旧20路径Finding/Eq不改，取消义务和after-only/未验边界不参与matched。repair引用切换后必须按正常Task报告复验；本节不预填Source/final成功。

# 采用时点记录（Task闭合前）

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

本实体仅两文件；Git684 before39职责静态分流、R1—R9有限共同不变量、T1—T6取消和C1—C4/Source8 after-only不混同。B336/B1821/T1、U8 F1/F3未独立实跑、未知JSON无新增永久成功CAS合同等限制保原。Eq.after_revision目前只绑定真实source；正常Task report返回后才填task值，Finding仍open，尚不宣称Source/final通过。

# 原有限材料与历史证据原文

# 当前旧控制历史专用兼容的批准退出

## 问题

用户Goal取消两份专用控制历史/旧断点兼容；通用Store、普通CAS、当前恢复与原owner继续保留。

## 触发条件与影响

Git684两真实生产源与39测试拆为共同R1-R9、批准退出T1-T6及C1-C4/Source8 after-only。旧20路径0e4 Eq/Finding保持原源/路径/范围，不扩充或改写。

## 证据

新before Evidence实际Git源读取、未运行；新限定Eq仅九共同不变量source-before+R15 actual-after，命令只写真实after。普通退出事实保39台账与B336/B1821正确分流，R20旧事实及失败时态独立。a237充分性/source审查有限C0/I0/M0，rereview按原检查范围记录，不冒runtime Audit。

## 处理记录

两路径已退出且当前typed主路径实际覆盖。当前status仍open，待原validate_task实际task身份、正常type/ref/closure与最终材料复审后再resolved，不能先填resolved求绿。此前“0.5无法表达”属范围未细分时判断，现按113–117有限合同继续，不改工具/schema或假matched取消场景。
