# ADR 0037: Eval timeout waits for Turn settlement

- Date: 2026-09-14
- Owners: `eval/runner`, `runtime/turn_driver`

## Status

Accepted.

## Context

`EvalRunner` currently applies `tokio::time::timeout` to a helper that opens
and consumes the raw Agent stream. When the deadline expires, the helper future
is dropped, the cancellation token is signalled, and Eval immediately reads
trace state and returns. Cancellation is only a request; a ReactAgent stream
owns a spawned producer and uses a separate bounded reaper after its consumer
is dropped.

ADR 0036 already isolates every Eval in a unique workspace generation and
retains that directory after timeout. Isolation prevents a later case from
sharing files, but it does not establish the terminal state needed for trace
scoring or safe cleanup.

`AgentTurnDriver` is the framework's existing authority for a finite Agent
invocation. It consumes one raw stream, enforces one terminal, and returns a
typed `TurnReceipt`. Eval should consume that authority instead of retaining a
second event-to-terminal reducer.

ReactAgent needs one additional implementation guarantee. Its public stream is
backed by a spawned producer, while `AgentEvent::is_terminal()` promises that a
terminal ends the invocation stream. The managed stream therefore cannot
release a buffered terminal while the owned producer is still running. Doing
so would let a receipt race the producer's remaining lifecycle work.

Tokio documents that timeout expiration cancels the wrapped future and performs
no additional resource cleanup. `CancellationToken` signals a request that the
task must observe. OpenAI Codex acknowledges a turn interrupt when
`TurnAborted` arrives, not when the interrupt is merely submitted. Inspect AI
likewise separates interrupted cleanup from ordinary completion.

- <https://docs.rs/tokio/latest/tokio/time/fn.timeout.html#cancellation>
- <https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html>
- <https://github.com/openai/codex/blob/a505c71490885a44979df056284badbfdd75b3fb/codex-rs/app-server/src/request_processors/turn_processor.rs>
- <https://github.com/openai/codex/blob/a505c71490885a44979df056284badbfdd75b3fb/codex-rs/core/tests/suite/abort_tasks.rs>
- <https://github.com/UKGovernmentBEIS/inspect_ai/blob/b6589d81f449112bb9942b0003b6fd9b54f1c48e/docs/extensions-sandboxes.qmd#L133-L149>

## Options Considered

1. Keep raw stream consumption and treat `cancel()` as settlement. This is the
   confirmed defect.
2. Rely only on ReactAgent's stream reaper. Eval supports arbitrary Agent
   implementations and cannot observe that reaper's completion.
3. Spawn a new Eval-owned driver task and track JoinHandles. `run` borrows its
   Agent and does not need another registry or task authority.
4. Pin one `AgentTurnDriver::drive` future, borrow it for the primary deadline,
   cancel on timeout, and borrow the same future for a bounded settlement grace.
5. Await settlement without a bound. An Agent may ignore cancellation and make
   the Eval suite hang indefinitely.

## Decision

Choose option 4.

- Eval builds a `TurnRequest` in Execute mode with the current generation cwd
  and CancellationToken. A stateless sink accepts every driver envelope.
- `AgentTurnDriver::drive` is created exactly once and pinned. The primary
  timeout and cancellation grace both poll that same future by mutable
  reference; the Agent invocation is never restarted.
- The existing ReactAgent stream-reaper settlement duration, six seconds, moves
  to one root-crate internal
  `AGENT_CANCELLATION_SETTLE_PERIOD`. ReactAgent and Eval share it.
- ReactAgent's managed stream buffers a terminal item until its producer
  `JoinHandle` settles. Dropping a consumer before terminal delivery still
  requests cancellation and transfers the handle to the existing bounded
  reaper. A producer panic becomes a stream failure instead of releasing a
  previously buffered success terminal.
- A receipt before the primary deadline is mapped normally. Completed requires
  a final answer; Cancelled, Failed, and EOF are typed non-success terminals.
- At the primary deadline, Eval signals cancellation and waits up to the shared
  grace. Any receipt proves settlement, but the Eval result remains a Timeout
  failure even if the late receipt is Completed.
- A settled timeout may load terminal trace metrics and explicitly close its
  workspace generation. It does not run success criteria on a late final
  answer.
- If grace expires, Eval records an unsettled violation, does not load RunStore
  or run criteria, and retains the workspace generation. Dropping the drive
  future then leaves any Agent-specific producer cleanup with its existing
  owner.

## Framework And Application Boundary

Turn driving and terminal receipts remain framework orchestration. Eval owns
only its deadline policy, quality result, and workspace disposition. EKO does
not add an application timeout state machine or duplicate receipt.

## Consequences

- A timeout can take up to six additional seconds before returning.
- Cooperative Agents yield an observable terminal receipt, terminal trace
  metrics, and safe workspace cleanup after cancellation.
- Non-cooperative Agents do not block indefinitely. Their trace is not scored
  as terminal and their isolated workspace is retained for diagnosis.
- Eval no longer interprets raw `AgentEvent` terminals or EOF independently.
- A ReactAgent terminal now follows producer settlement. Other Agent
  implementations remain responsible for the existing contract that no
  invocation work continues after their terminal stream item.
- The EventEnvelope sink adds no persistence or product projection.
- Public Rust and SDK shapes do not change.

## Compatibility And Rollback

`timeout_secs`, Eval result types, Agent APIs, and TurnDriver APIs remain
source compatible. The observable timing and timeout cleanup behavior are
documented in both languages.

Rollback must restore the raw helper and local six-second constant together.
Treating cancellation request as settlement is not an acceptable partial
rollback.

## Verification

A cancellation-responsive Agent records its workspace, waits for cancellation,
performs a final observable action, and emits Cancelled. The repaired Eval must
wait for that receipt, retain Timeout as the result, and remove the settled
workspace. The old helper drops the stream future and fails this test.

A managed React stream buffers a terminal while its producer is deliberately
held open. Polling the consumer must remain pending until the producer settles,
then return that terminal exactly once. A late Completed timeout must still
fail, run no success criteria, and create only one Agent stream. An unsettled
timeout with a run ID and counting RunStore must perform zero trace loads.

An Agent that ignores cancellation must return after the six-second grace with
an unsettled violation, no terminal trace scoring, and a retained workspace.
Existing Completed, Failed, Cancelled, EOF, criteria, trace, workspace,
Improvement, TurnDriver, bilingual documentation, feature, SDK zero-diff,
semantic, Issue, and review gates remain required.
