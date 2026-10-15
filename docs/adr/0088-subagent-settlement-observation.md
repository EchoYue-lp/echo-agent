# ADR 0088: Cancellation observation does not own physical settlement

- Date: 2026-10-15
## Status

Accepted; user explicitly selected option B and authorized implementation/delivery.
- Design: [Subagent physical settlement](../supreme/specs/2026-10-14-subagent-physical-settlement/design.md).

## Context

A local dispatch JoinHandle can finish after abort while a detached producer is still cleaning. Releasing claim/interruption/permit on that local fact permits overlapping effects. Historical held-cleanup evidence exposed this conflict with the physical-stop-before-CAS guarantee of ADR0060. Existing Task lifecycle states and execution owners already express pending cleanup.

## Decision

Keep the original RuntimeTaskService/RuntimeDagExecutor/SubagentExecutor owner through original stream closure and exact CAS. The configured cancellation grace bounds only an execute/settlement observer, starting once at its first admitted root or exact cancellation scope. Exact A does not start sibling B's run deadline. Normal execution has no observation clock.

Expose the Rust observation `ReactError::ExecutionNotSettled { run_id, execution_id }`, never an Agent terminal. Observer expiry cannot fabricate Cancelled/Paused/Failed, release a claim or trigger mutation retry. The existing owner keeps permits and cleanup; repeated requests do not extend the deadline. Parent drop/unwind signals then drains, never aborts solely because observation grace elapsed. Loss of authoritative closure remains AuthorityUnknown, including restart and disconnect; it cannot launch a replacement attempt.

This refines ADR0058/0060 forced-abort wording: only physical completion permits settlement. Existing execution timeouts still determine cause but do not destroy the sole cleanup future. No new TaskStatus, scheduler, store, backend authority, SDK API or accepted wire identity is introduced.

## Alternatives

Forced abort with a new durable physical-receipt protocol would require another interface and was not selected. Retention can hold existing capacity forever for non-cooperative code, which accurately exposes the existing work rather than releasing its effects as if stopped. Operators may need authoritative backend/process shutdown. Tokio shutdown/JoinHandle and Python cancellation patterns referenced by the design separate cancellation notification from completion.

## Consequences

Examples and website do not acquire a new product feature. Existing callers must handle a Rust observation error without terminal conversion; SDK error-detail adaptation is tracked independently. Required tests cover held cleanup, root/exact isolation, caller lifecycle, cause order and unknown recovery on the current source.

## Rust migration

Replace infallible `AgentFailure::From<&ReactError>` with `TryFrom<&ReactError>` and make `AgentEvent::from_error` fallible. A nonterminal observation has no valid AgentFailure value: refusal preserves its run/execution identity and converts back to the same typed ReactError. All existing actual terminal errors keep their exact fields. Migrate direct and into-style consumers; never substitute Failed, Other, panic or a new terminal kind. Event normalization passes an original observation Err through without synthesizing missing-terminal failure. Team timeout and dispatch/result boundaries propagate it without releasing the original physical owner. Existing Rust consumers must explicitly handle the fallible API; SDK Host adaptation and accepted cross-language membership remain separately owned, and CLI compilation is part of consumer validation.

`AgentTurnDriver::drive` changes to `Result<TurnReceipt>` at its actual setup/stream/sink observation boundary; ordinary receipts remain unchanged and consumers propagate the refusal. Internal Team dispatch must not mistake a nested runtime observer timeout for physical closure: `RuntimeTaskService::execute_to_settlement` shares the one original private spawn helper with `execute`, but waits the original join without an observer timer. Public external observation remains bounded. The owner-composition method is not a language API or a second scheduler/store/registry, and callers must not execute again to recover a timed-out owner's result.

Foreign Agent setup can expose not-settled identity without returning a stream. There is no canonical future to invent or re-execute. ACP retains the original active Session lease within the existing RunEntry's uncertain-owner resource slot and keeps Running/no receipt; only authoritative receipt/fence releases it. Admission, close and restart must not infer completion from missing process-local control, disconnect or revocation. No permanent pending task or mem::forget is used to simulate ownership; other framework consumers preserve their own existing resource/fence authority.

Root interruption adds required `RuntimeDagController::request_task_interruption(run_id, task, claim, disposition) -> Result<bool>` to delegate to the existing exact-claim request CAS without terminal settlement. True is same-claim durable admission, false is supersession, and errors retain the original claim/unknown. No default success or alternate state mutation is provided. Store/Team/SDK/CLI controller source implementations migrate explicitly; no new wire operation is introduced.

Original attempt projections retain shared/local permits and physical-closure knowledge through uncertain exits; neither a string error nor durable claim disappearance releases physical ownership. First request clocks require successful admission. React retains steering and execution resources in the existing close authority, fences new turns and reports unknown on close; no new scheduler or parked replacement task is involved. All opened stream sources drain through actual EOF before ordinary terminal/cancellation receipts as well as observer errors.

Shared admission uncertainty survives observer/service destruction in the original KeyedExecutionAdmission key/generation state. Opaque owner/generation fences are queryable, and only authoritative original physical completion/backend fencing permits a matching clear; stale/wrong-owner proof fails. InputReceipt waits become fallible Result without adding public input states: the original watch carries private refusal identity, wakes existing observers, and never fabricates Dropped/Drained on sender loss. Ordinary real input and terminal states retain their fields.


### Existing Tool/Sandbox projections

The accepted nonterminal error must also bypass existing Tool/Sandbox error-to-result adapters and React callback/transcript publication. Private result plumbing may be fallible or absent for an observation, while ordinary tool results and the public ToolFailure shape remain unchanged. Unknown physical work is not a completed tool failure or permission to continue the LLM. This is the same B settlement boundary, not a new tool policy or API.

The canonical started/draining Tool path keeps its existing execution future and permit through configured timeout, React cancellation grace and batch timeout. These boundaries signal cancellation and retain the first business cause; they do not drop physical work. A streaming Complete is retained until the same stream reaches actual EOF. Only this canonical draining path changes; public ordinary non-draining bounded behavior remains unchanged.

The same started/draining owner survives output receiver closure: retain the original future and permit, signal only its child cancellation scope, stop delivery and return the first delivery cause after physical EOF. ToolManager adds an embedded resource projection through the existing KeyedExecutionAdmission mechanism for unknown/unwind retention; its existing read/write semaphores remain the sole quota. No additional threshold or execution registry is introduced.
