---
schema_version: 1
id: finding.lsp-response-value-fidelity
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: contract_evidence
focus:
- trigger_input
- result_side_effect
- failure_concurrency
boundary_ref: boundary.protocol-surfaces
behavior_refs:
- behavior.protocol-projection
rule_refs:
- rule.protocol-role-separation
evidence_refs:
- evidence.lsp-response-value-fidelity-investigation
- evidence.framework-parent-drop-final-verification
- evidence.framework-final-34d5-full-verification
audit_refs:
- audit.lsp-response-value-fidelity-rereview
- audit.framework-parent-drop-rereview
- audit.framework-final-34d5-gate-rereview
decision_refs: []
repair_evidence_refs:
- evidence.lsp-response-value-fidelity-investigation
verification_evidence_refs:
- evidence.framework-parent-drop-final-verification
- evidence.framework-final-34d5-full-verification
rereview_audit_refs:
- audit.lsp-response-value-fidelity-rereview
- audit.framework-parent-drop-rereview
- audit.framework-final-34d5-gate-rereview
discovered_at: 95ec66c6083a8feb17a1c28de0d01e1a24ca8a28
---


> 以下处理文字是历史发现/修复阶段记录（历史审查快照 8843afd5029afba9be8a6551de4ab0a93951311f）；其中 open、门禁进行中和待复审描述已由下方当前结论取代，未完成交付仍单独保留。


# LSP accepted value projection loses range, null and optional completion facts

## 问题

The real framework request paths force Hover.range to None; malformed Location
or Completion results become successful empty arrays. JSON-RPC Option<Value>
drops the distinction between result:null and a missing result. Completion
requires a kind although LSP makes it optional and uses numeric values and
camel-case insertText. The SDK's existing accepted values inherit these defects.

## 触发条件与影响

A conforming language server returns Unicode hover with a range, nullable
results or optional/numeric completion kind; callers lose accepted values or
receive false errors. A malformed response can look like successful absence.

## 唯一权威与边界

Framework StdioLspClient owns protocol decoding and existing LSP lifecycle.
SDK Host/languages only project the framework values. ADR 0082 records the
existing kind field's optional signature correction; no new public identity
or parallel SDK parser is accepted.

## 证据

Source review at base 95ec66c6 and official Microsoft LSP 3.17 references are
recorded in investigation evidence and ADR 0082. Eight real stdio Rust regressions pass, including two independent-review
counterexamples after genuine failures. Final independent rereview and combined
full-gate/published-main/SDK consumer verification remain pending. This Finding remains open through published successor,
SDK contract generation, three-language evidence and exact-main acceptance.

## 处理记录

Focused red/green and Clippy pass for this uncommitted candidate; full combined
gates, independent review and consumer proof remain pending. Status stays open.


## 当前处理结论

本地问题当前闭合：新组合 23 门禁、精确源码及独立复审 pass/action0 已归档；历史证据保持各自快照。公开 pin、SDK/CLI/官网与远端 main 为独立交付义务，本地 resolved 不表示它们完成。


## 当前候选最终复审

audit.framework-final-34d5-gate-rereview 将本地此故障的精确源码/既有红绿证据与同source34d5新完整23门禁绑定；独立implementation review PASS/action0。当前可以进入授权framework里程碑交付，SDK/remoteCI/公开pin及精确main仍另验，其它Finding与全risk lenses不随此关闭。
