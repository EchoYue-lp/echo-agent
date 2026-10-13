---
schema_version: 1
id: finding.lsp-diagnostic-notification-fidelity
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: contract_evidence
focus:
- state_authority
- result_side_effect
- failure_concurrency
boundary_ref: boundary.protocol-surfaces
behavior_refs:
- behavior.protocol-projection
rule_refs:
- rule.protocol-role-separation
evidence_refs:
- evidence.lsp-diagnostic-notification-repair-verification
- evidence.framework-parent-drop-final-verification
- evidence.framework-final-34d5-full-verification
audit_refs:
- audit.lsp-diagnostic-notification-fidelity-rereview
- audit.framework-parent-drop-rereview
- audit.framework-final-34d5-gate-rereview
decision_refs: []
repair_evidence_refs:
- evidence.lsp-diagnostic-notification-repair-verification
verification_evidence_refs:
- evidence.lsp-diagnostic-notification-repair-verification
- evidence.framework-parent-drop-final-verification
- evidence.framework-final-34d5-full-verification
rereview_audit_refs:
- audit.lsp-diagnostic-notification-fidelity-rereview
- audit.framework-parent-drop-rereview
- audit.framework-final-34d5-gate-rereview
discovered_at: 95ec66c6083a8feb17a1c28de0d01e1a24ca8a28
---


> 以下处理文字是历史发现/修复阶段记录（历史审查快照 8843afd5029afba9be8a6551de4ab0a93951311f）；其中 open、门禁进行中和待复审描述已由下方当前结论取代，未完成交付仍单独保留。


# LSP diagnostic notifications fabricate empty success

## 问题

The real reader uses generic serde for official Diagnostic notifications and
unwrap_or_default on every failure. Numeric optional severity and i32|string
code do not match the old types. Invalid document updates replace old results
with false []. The initialize capability advertises unsupported related data.

## 触发条件与影响

Any real server publishing normal numeric severity or integer codes can cause
callers to see a clean document. Missing/invalid envelope identity silently
fails to settle a visible failure; malformed ranges may also be accepted as
Serde tuples. SDK already accepts the Diagnostic fields and inherits the facts.

## 证据

Official Diagnostic/publishDiagnostics documents, actual client reader, core
fields and tools/lsp consumer are described in ADR 0082's follow-up. Four real
stdio tests are prepared; fixture protocol frames are independently validated.
These are not production green or exact-main evidence.

## 处理记录

Accepted plan keeps a single framework per-URI Result cache and explicit typed
optional severity/closed code union. Missing URI uses existing connection
terminal owner; no SDK decoder or second error authority. Red/green, consumer
checks, independent review, final successor and SDK behavior proof are pending.

Four real stdio regressions failed before and pass after production repair;
whole LSP32 tests and focused Clippy/consumer checks pass. The Finding remains
open until independent review and published successor/SDK proof complete.


## 当前处理结论

本地问题当前闭合：新组合 23 门禁、精确源码及独立复审 pass/action0 已归档；历史证据保持各自快照。公开 pin、SDK/CLI/官网与远端 main 为独立交付义务，本地 resolved 不表示它们完成。


## 当前候选最终复审

audit.framework-final-34d5-gate-rereview 将本地此故障的精确源码/既有红绿证据与同source34d5新完整23门禁绑定；独立implementation review PASS/action0。当前可以进入授权framework里程碑交付，SDK/remoteCI/公开pin及精确main仍另验，其它Finding与全risk lenses不随此关闭。
