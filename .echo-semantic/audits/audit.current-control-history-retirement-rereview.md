---
schema_version: 1
id: audit.current-control-history-retirement-rereview
kind: audit
boundary_ref: boundary.agent-session-turn
lens: contract_evidence
freshness: examined
revision: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
finding_refs:
- finding.current-control-history-retirement
challenges:
  before-source-not-runtime:
    revision: 684ad1d9d0de9bff57e27e4c04f04232c6af9f1f
    source_refs:
    - src/state/legacy_control_history.rs
    - src/state/legacy_control_history/pending_recovery.rs
    evidence_refs:
    - evidence.current-control-history-before-source
    - evidence.current-control-history-retirement
  preserved-current-invariants-only:
    revision: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
    source_refs:
    - echo-core/src/llm/types.rs
    - echo-integration/src/providers/anthropic.rs
    - echo-state/src/memory/conversation.rs
    - src/agent/react/run/native_message_contract_tests.rs
    - src/agent/react/run/phases/finalize.rs
    - src/agent/react/run/phases/tools.rs
    - src/agent/react/run/phases/verify.rs
    - src/agent/react/run/stream_channel.rs
    - src/agent/snapshot.rs
    - src/state/current_native_recovery_tests.rs
    - src/state/file.rs
    - src/state/mod.rs
    - src/state/sqlite.rs
    evidence_refs:
    - evidence.current-control-history-retained-contracts-equivalence
    - evidence.current-control-history-retirement
  cancelled-and-after-only-not-matched:
    revision: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
    source_refs:
    - docs/adr/0101-native-message-execution.md
    - docs/adr/0103-current-storage-records.md
    - docs/adr/0104-canonical-agent-runtime-kernel.md
    - src/state/current_native_recovery_tests.rs
    evidence_refs:
    - evidence.current-control-history-retirement
    - evidence.current-control-history-retained-contracts-equivalence
---

# 当前两文件闭合状态

原未改validate_task在正常repair路由切换后实际返回errors=[]，SourceSnapshot为`task:ae377fd33de65865359fb7bbd921b28241b036cb8beccabc815d45ddb4d88815`。Eq.after_revision取此真实值；当前Finding仅本两D为resolved，R1—R9有限共同不变量与T1—T6取消／C1—C4 after-only分列，原Git684 before39职责与source-backed rereview保原。旧20路径Finding/Eq无修改，外部工具合同、真实unknown、SDK/newpin/native/Git/mainCI仍独立。本状态是已取得的Task结构与变更依据检查，不预填尚未执行的最终Source/final。

# 采用时点记录（Task闭合前）

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

本实体仅两文件；Git684 before39职责静态分流、R1—R9有限共同不变量、T1—T6取消和C1—C4/Source8 after-only不混同。B336/B1821/T1、U8 F1/F3未独立实跑、未知JSON无新增永久成功CAS合同等限制保原。Eq.after_revision目前只绑定真实source；正常Task report返回后才填task值，Finding仍open，尚不宣称Source/final通过。

# 原有限材料与历史证据原文

# 两删除有限职责保全复审

## 审查范围

转录独立充分性/source reviewer已经完成的限定检查：current-two-file-retirement-sufficiency-review-r01，SHA a2377e3dda507f81b731be47a04e14d4c454eecd62024cf33a02baf197994542，implementation/plan-none/用户Goal，C0/I0/M0。只审Git684两D→当前R15的证据充分性与39职责分流；不是重新执行测试的runtime Audit，也不签Source/final成功。

## 已检查故障假设

检查把取消compat/混合测试整体matched、before源trace称执行、manifest当机器after、B336 readonly不修scope和B1821旧getter混为共同义务、U7缺失以及U8任意unknown永久保存等假设。

## 实际实现路径与证据

R1-R9仅合法current字段共同不变量；A17+B22逐条保Git职责，T1-T6普通退出/C1-C4及Source8after-only分离。U7在R15raw1907实际ok；U8 typedCAS/replay/scope/epoch/ACK/digest/owner源审及同源负控足此有限范围，F1/F3未测明确保留。没有重跑已取消旧成功、修改外部contract或将旧20path Eq续票。

## 问题记录

B336/B1821原材料职责混合已纠正，旧整测试不matched。独立报告有限pass不自动resolved；writer完成normal实际task/source身份、type/ref/closure及最后材料增量确认后才处理Finding状态。

## 残余风险

只检查有限不变量；真实provider、SDK新pin/三语/native/mainCI与全平台不在本审查资格内。

## 未检查项

Before执行、U8F1/F3独立磁盘组合、未来任意unknown字段永久保留均未检查；4ignored和长期Discovery真未知保留。
