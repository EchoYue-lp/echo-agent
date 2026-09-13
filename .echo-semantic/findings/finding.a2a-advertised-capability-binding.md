---
schema_version: 1
id: finding.a2a-advertised-capability-binding
kind: finding
type: intent_gap
status: open
severity: high
primary_focus: contract_evidence
focus: [trigger_input, result_side_effect, state_authority]
boundary_ref: boundary.protocol-surfaces
behavior_refs: [behavior.protocol-projection]
rule_refs: [rule.protocol-role-separation]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.protocol-surfaces.contract-evidence]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# A2A宣告的file/push capability未绑定实现

## 问题

AgentCard可宣告任意input modes与push_notifications，A2AMessage支持File；Server实际只压缩text并实现send/subscribe/get/cancel，无push delivery。

## 触发条件与影响

Peer按宣告发送file或注册push时可能被静默降为文本/无实现，capability negotiation不再反映真实surface。

## 证据

`src/a2a/types.rs`的card/message与`src/a2a/server.rs`的method/input路径形成合同反例。

## 处理记录

Contract Audit确认；需semantic-decide选择明确text+SSE或实现file projection/push，再绑定capability生成。
