---
schema_version: 1
id: evidence.lsp-diagnostic-notification-repair-verification
kind: evidence
observed_at: 8843afd5029afba9be8a6551de4ab0a93951311f
source_refs:
  - echo-core/src/lsp/types.rs
  - echo-integration/src/lsp/client.rs
  - echo-integration/src/lsp/manager.rs
  - src/tools/lsp.rs
  - docs/adr/0082-lsp-response-value-fidelity.md
  - docs/en/31-lsp-integration.md
  - docs/zh/31-lsp-integration.md
supports: [behavior.protocol-projection, rule.protocol-role-separation]
limitations:
  - Focused isolated task checks do not prove final combined framework gates or main delivery
  - Actual SDK diagnostics three-language and supporting-value contract generation remain pending
  - Full LSP related metadata and unaccepted identities are not implicitly accepted
---

# Diagnostic notification fidelity focused repair proof

## 支持的结论

The existing framework reader parses accepted Diagnostic fields from native LSP
objects. Optional severity remains None or one of four declared values; codes
retain signed-i32 versus string through a closed tagged value. One per-URI cache
stores latest valid diagnostics or the bounded schema failure; malformed updates
replace prior success, valid updates recover, [] clears, and other URIs remain
independent. No partial array or false clean success is published.

Unaddressable malformed params/URI ends the established reader/runtime process
owner. Pending requests fail explicitly, status preserves cause after real child
exit, and subsequent diagnostics returns a ServerError. No extra error cache or
SDK parser exists. The client no longer advertises unrepresented related data.
The existing LSP tool renders missing severity as UNSPECIFIED and uses checked
non-panicking position-display arithmetic.

## 来源与范围

Official Microsoft Diagnostic/publishDiagnostics source blob SHAs are
38bd1b3d3d1ce2fee270bf1759255146e41f9a6b and
fbba90a322b804cd296daefbcb45d6aabf05dc61. Full documents and exact source/accepted
field search are retained in the task evidence. SDK accepted Diagnostic fields
are range/severity/code/message/source, with no relatedInformation field.

Four new real stdio tests fail before production changes: 0 passed, 4 failed,
exit 101, `/tmp/framework-lsp-diagnostics-red.log`. Same tests pass after repair:
4 passed, 0 failed, 0 ignored, `/tmp/framework-lsp-diagnostics-green.log`.
They use ordered notification/response barriers rather than sleeps.

The entire LSP subset passes 32 tests (0 failed, 1 existing installed-server
opt-in ignored), `/tmp/framework-lsp-diagnostics-full-lsp.log`. Focused integration
Clippy, root no-default LSP feature check, root LSP consumer Clippy all exit 0,
no warnings, with production unwrap/expect/panic/unreachable checks.
Executable demo55, formatting/check and current strict snapshot checks are
recorded in `/tmp/framework-lsp-diagnostics-verification.json` when complete.
No commits, remote main or published successor is claimed.

## 已知缺口

Independent review, combined final workspace/feature gates, accepted SDK union
value support, exact final Host three-language diagnostic evidence and precise
published-main acceptance remain required. Other optional LSP metadata is not
silently described as supported; inventory remains telemetry.

## Terminal admission race counterexample

Independent review found initialized/running remains true while real process
teardown awaits, and cache has already cleared. Two actual stdio counterexamples
hold Child mutex so the PID remains live after admission closes; one also holds
cache across an already-started retrieval. The old candidate returns Ok([]) in
both, 0 passed/2 failed (`/tmp/framework-lsp-diagnostics-cache-red.log`). The
initial new-test type compilation failure is separately retained in
`/tmp/framework-lsp-diagnostics-admission-red.log` and is not behavior proof.

Repair checks existing runtime admission/cause after cache acquisition and
rechecks the lifecycle fence. The same six diagnostic tests pass 6/6 without
skips (`/tmp/framework-lsp-diagnostics-admission-green.log`). All LSP tests pass
34 with one existing installed-server opt-in ignored. No earlier 32-test result
is described as final current-source evidence.

历史定位说明：原未提交 source8896e508 的846个源码/文档文件已逐字节重建，唯一 source_digest 精确匹配，并保存到本地 recoverable Git snapshot `8843afd5029afba9be8a6551de4ab0a93951311f`（refs/echo-semantic/snapshots/sdk-framework-reviewed-8896）。这是内部审查证据快照，不是交付分支提交、公开 dependency 或 main。原结论与限制保持；新 parent-drop 源码不借此历史 pass。
