# ADR 0084: Carry the reserved ACP Turn into the original Agent invocation

## Status

Accepted implementation decision; validation and delivery are tracked separately.

## Context

Standard ACP Prompts and SDK Run starts reserve one Turn through AcpSession.
AcpConnectionServices::prepare_run already records that identity in RunEntry,
ledger envelopes and the driver receipt. A text Turn without an invocation
context nevertheless selected the Agent's legacy stream method. ReactAgent
then allocated a different internal Turn UUID because runtime.turn_id was absent.
Tracked steering correctly rejected the mismatch between the original ACP Run
and that UUID, even while the real model request was active.

The SDK native counterexample parks the original loopback model request and
calls the public Run.steer method. Original Host bcb0389 reports accepted=false;
its own diagnostic log identifies the expected Session Turn and unrelated UUID.
A default Agent may legitimately not support steering, but that is not the
cause in this case: the default Host creates ReactAgent, which implements
tracked steering.

## Decision

Use the existing shared ACP preparation boundary to supply the already reserved
Turn as AgentInvocationContext.runtime.turn_id. RunStartSpec.run_id and the
TurnRequest identity must agree with the active Session lease before registry
publication. A caller-supplied conflicting runtime.turn_id is an explicit error;
it is not overwritten, ignored or mapped to a new identity. A matching value is
preserved. All other invocation fields remain in the original value, including
TaskRun run_id, lineage, working directory, cancellation, tools, history and
resource guards. Ordinary chat must not invent an application TaskRun identity.

Preserve legacy text Agent compatibility through the existing lower default
Agent::chat_stream_message_with_cancel method. Only Role::User with an exact
MessageContent::Text and all five optional metadata fields absent can use the
original legacy chat_stream_with_cancel path. Some(empty) metadata is present,
not absent. Parts, ResourceLink, Empty, non-User roles and metadata-bearing
messages retain the explicit unsupported error; no text flattening is allowed.
The outer invocation-aware default still delegates to the existing structured
method, so a concrete structured override always wins and retains its behavior.
No trait method, public DTO, wire operation or capability flag is added.

This is an observable dispatch change for ACP text callers: supplying the
canonical invocation selects the driver's already-existing Text + Chat + Some
branch, which invokes the structured method. An Agent implementing both legacy
text and structured overrides may therefore observe a different method and
output than before this identity fix. The trait's "same conversational
semantics" requirement means append/preserve context rather than reset; it
does not require those two overrides to return identical output. This decision
does not promise unchanged output for every third-party dual implementation,
and does not globally redirect invocation-aware text back to the legacy route.
The existing structured override and driver contract remain authoritative.

The fallback owns the original text in an async stream while the legacy stream
may borrow it across later polls. That fallback starts legacy setup on the
first stream poll; a legacy setup error becomes that stream's original error
item and the canonical Turn driver classifies it as Failed. It must not create
a successful terminal or replace the original error. Illegal structured inputs
remain an immediate method error. Existing cancellation and invocation guard
wrappers continue to own cancellation and lifetime.

## Alternatives

- Remove expected Turn matching: rejected, because it would steer the wrong
  original invocation instead of restoring identity propagation.
- Add an SDK-only identity map, a second Run or a task-local mutable context:
  rejected; the Session lease and value-scoped invocation already own these facts.
- Overwrite TaskRun run_id with the ACP Turn: rejected, because those are
  different identities with distinct canonical owners.
- Add a new text invocation trait method and forwarding paths: unnecessary
  API expansion when the existing exact lower fallback can retain legacy text
  behavior without bypassing structured overrides.
- Use as_text or error-string probing for fallback: rejected; that could erase
  multimodal/metadata semantics or misclassify a concrete implementation error.

## Consequences

This is a product-independent framework ACP mechanism, consumed by standard ACP
and the SDK Host. SDK clients remain thin transports; EKO policy and Task state
are unchanged. The existing turn-terminal and allocation authorities in ADR
0046 and ADR 0061 remain authoritative. There is no new persistent state,
scheduler, permission, source-acceptance obligation or error-code taxonomy.

The RecordingAgent ACP test fixture deliberately placed its cancellation,
barrier, tool and failure scripts only in the legacy method; its structured
override returned an immediate differently labelled answer for plain text.
The initial complete ACP integration run therefore passed 17 tests and failed
9 after invocation propagation. For these existing scripted tests only, the
fixture's exact metadata-free User/Text structured branch now delegates to its
own legacy script with the same cancellation token. All original assertions,
time bounds and Parts/ResourceLink rendering remain unchanged. Independent
review confirmed that test-fixture correction; it is not a new product rule
that dual overrides must behave identically. The core dual-override test still
proves structured dispatch once and legacy dispatch zero times, and the original
driver structured-chat contract remains unchanged.

Direct capture tests cover Chat/Execute and text/structured dispatch with the
reserved Turn, preservation of caller TaskRun/lineage/policy, and pre-publication
rejection of conflicting identities. Compatibility tests must prove legacy
text dispatch, structured override priority, strict message admission, owned
text/guard cancellation lifetime and original failed setup receipt. SDK native
three-language tests prove actual model-context consumption and concurrent waits
on one receipt; framework unit tests do not replace those consumer results.

No example changes are required because the APIs remain unchanged. Existing
ACP examples and the full framework gates remain delivery checks, and the
website does not acquire a new feature description. Diagnostic local path-patch
Host results are explicitly not published revision or SDK main proof; framework
publication and an exact SDK pin upgrade are still required.
