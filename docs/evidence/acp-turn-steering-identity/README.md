# ACP canonical Turn steering identity — source review candidate

This candidate fixes missing execution-context identity in the shared framework ACP runtime. It is not a new SDK identity map or Run authority. Standard ACP Prompts and extension Run starts retain the original Session-reserved Turn. The original Agent expected-Turn steering check remains intact. Conflicting run/request/invocation Turn identities fail before Run publication, while caller TaskRun run_id, lineage and policy remain in the original invocation value.

## Observable compatibility boundary

Adding canonical invocation metadata makes ACP text use the driver's existing Text + Chat + Some path, which invokes the structured method. A third-party Agent implementing both legacy text and structured overrides may therefore observe a different method and different output. The trait's same-conversation requirement concerns append/preserve context rather than reset; it does not require those overrides to return identical output. The candidate does not promise unchanged output for every dual implementation and does not globally redirect invocation-aware text to the legacy method.

The existing lower default only adapts an exact Role::User / MessageContent::Text with tool_calls, name, tool_call_id, reasoning_content and reasoning_blocks all None. Some(empty) is rejected as metadata present. Parts/ResourceLink, Empty and other roles retain their unsupported error; there is no as_text flattening. The outer invocation default and existing concrete structured overrides are unchanged. No new trait method, forwarding path, public DTO, wire operation or capability is added. The owned fallback retains borrowed text through later stream polls and preserves cancellation and invocation guards. Legacy setup begins at first poll on this new fallback path; its original setup error remains an error stream item and produces one canonical Failed receipt. Invalid structured input still fails immediately.

The original RecordingAgent fixture had its cancellation/barrier/tool/failure scripts only in the legacy method and immediately returned a different labelled answer from its structured method for plain text. After invocation propagation, the first complete ACP integration run was genuinely 17 passed / 9 failed. Its raw output is `final-all-acp-adapter.log`. The fixture correction reuses that same legacy script only for its exact metadata-free User/Text branch and retains the cancellation token; all original assertions, time bounds and real Parts/ResourceLink rendering are unchanged. `before-recording-fixture-pure-text-delegation.rs` and `recording-fixture-only-correction.patch` retain the exact delta. Independent substantive review confirmed this as a valid fixture correction. That review does not impose identical dual-override output as a product requirement. An independent core test still observes structured=1/legacy=0 for a dual implementation, and the original driver structured-dispatch contract remains unchanged.

## Exact source and root-cause evidence

`review-ready-source-manifest.json` is the current review cut; `candidate-scope-manifest.json` is a preserved earlier snapshot before the lower-default compatibility repair. The current manifest binds base be5 source hashes, all three current production/test source files, ADR 0084, the complete source delta and the exact raw command receipts. Production edits are limited to the shared ACP `prepare_run` identity propagation and the existing lower default chat method. All other new Rust code is tests. The original branch is `fix/Echoyue/acp-turn-steering-identity`, based on published `be5f0a8405b593c4ef964cd6fd2df4beabf4956c`.

The captured native failure chain is: Host reserves Session Turn and creates TurnRequest without invocation metadata; original framework runtime passes it unchanged; driver TextChat+None chooses legacy stream; ReactAgent has no runtime.turn_id and allocates a UUID; Host tracked steering correctly rejects the canonical Run/UUID mismatch. The initial runtime capture tests observe actual AgentInvocationContext without that Turn, not an assertion against a reconstructed getter or a second controller.

## Actual bounded verification

| Source/command epoch | Actual result |
|---|---|
| Old production plus five real context-capture tests | 0 passed / 5 failed, all missing original reserved Turn in actual Agent context. |
| Fixed ACP preparation capture tests | 8 passed; text/structured Chat/Execute, caller TaskRun/lineage/policy/tools/history preservation and pre-publication identity conflict checks. |
| Old lower default core tests | 2 passed / 2 failed. Strict admission and structured override pass; legacy owned text/setup-error support fail. |
| Corrected before-fallback ACP fixture | 0 passed / 2 failed: ordinary text Agent unsupported and original setup failure replaced by unsupported. |
| Fixed lower default core tests | 4 passed. |
| Fixed text-only ACP focused tests | 2 passed. |
| Complete echo_core library | 422 passed, zero failed/ignored, including existing invocation guard tests. |
| Original driver `text_chat_with_invocation_uses_structured_chat_contract` | 1 passed. |
| First complete ACP adapter integration | 17 passed / 9 failed, preserved unchanged before the RecordingAgent fixture correction. |
| Same complete ACP adapter command after reviewed fixture correction | 26 passed, zero failed/ignored. |
| Complete ACP library subset | 38 passed, zero failed/ignored, including all eight identity tests. |
| Affected echo_core/echo_orchestration/echo_agent library Clippy | Exit 0 with warnings and unwrap/expect/panic/unreachable denied. |

All original command arguments, times, full output byte counts, exit codes and log hashes remain in official run-command JSON. The final Clippy reached actual exit 0 while the stop/release request crossed with its completion; no successful result is fabricated for an interrupted command. No new compiler or Maven command is started for this final documentation handoff.

Preparation attempts remain separately classified: the first new integration-test pattern omitted `..` for AgentEvent::Error fields (compile error, not behavioral red), and a failed text-edit assertion left production unchanged before one repeated old-source core run (not a fix pass). The original multi-step official JSON files are preserved; the final review manifest selects the appropriate step index and separate preserved logs identify earlier failed attempts. Nothing is trimmed or replaced with a fabricated green run.

## Native proof epoch and delivery limits

The three SDK languages each use public Run.steer and two concurrent public waits. On original published-be5 Host SHA bcb0389ae8d8a946fab9bb123945100722711029bafa8d9d6cadf95a76c9c345, the same per-language source fails accepted=true while the original HTTP call is parked and Run.get is running. On the diagnostic runtime-fix Host SHA eb00038a0be7977f936514c7df8e06e329928f6735d466f292f10201a38abbf5, each passes accepted/nonempty steer ID, exact Unicode steering in the second actual HTTP model request, original user/draft context, two pending waits followed by identical terminal/receipt, original Run/stream identity, one final event, late steer rejection, and empty-input typed rejection. Actual JSON/HTTP/logs and Java source/class-bound XML receipts are retained under `native-*`.

The diagnostic binary uses an independent Admin workspace/lock and eight local framework path patches; its metadata/main/library hashes are retained in `diagnostic-source-build-manifest.json`. SDK pin/lock/config are unchanged. A first diagnostic compile with different macOS fingerprint flags was deliberately interrupted and retained as SIGINT, then the original flags were restored for the successful diagnostic build. No original immutable binary was overwritten.

That diagnostic binary predates the final lower-default compatibility change and RecordingAgent fixture correction. Its three-language results prove that earlier runtime-only fix mechanism, not final combined-source native acceptance. The independent SDK now also consumes the separately owned telemetry helper. The next final Host must therefore wait for the two framework slices to be integrated and recompiled; old diagnostic results are not relabelled as current published proof.

Full framework workspace/conditional gates, combined-source Host and complete native rerun, independent final source review, publication, exact SDK pin upgrade, SDK main/remote acceptance, and the separate twelve accepted-identity/Task-policy decisions remain root-owned. Existing public APIs and examples are unchanged; current examples remain executable delivery checks. No website feature is added. This is a bounded candidate source review packet, not an all-goal completion or physical-cleanup claim.

## Final formatting-only supplement

After the parent-dispatched source review passed with zero critical/important/minor findings, the exact repository commands `cargo fmt --all` and `cargo fmt --all -- --check` both reached actual exit 0 with zero output bytes. Their raw empty logs and official receipts are preserved as `final-workspace-fmt*`. The three reviewed production/test source hashes are identical before/after formatting; no other tracked source path became dirty. `git diff --check` also exits 0. This supplement runs no compilation, business test, submission or publication, and does not take the other producer's compile lease.

The previous 120-entry checksum and source-review manifest remain exact historical copies in `evidence-sha256-before-fmt.json` and `review-ready-source-manifest-before-fmt.json`. The current manifest adds only formatting facts, and the new checksum describes the enlarged packet rather than relabelling the old checksum. Raw logs are not trimmed. Index-level whitespace validation of newly tracked historical evidence remains a separate parent staging step; the current un-staged git diff check covers the actual tracked diff.
