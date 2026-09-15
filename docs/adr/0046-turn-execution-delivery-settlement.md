# ADR 0046: Separate Turn Execution and Delivery Settlement

## Status

Accepted

- Date: 2026-09-14
- Owners: `echo-orchestration::runtime`, `src/acp`, `echo-sdk-host`

## Context

`AgentTurnDriver` owns the execution terminal of a driven turn, while a sink
may commit a Journal/Ledger fact, render a protocol projection, or notify an
extension observer. These operations happen after the driver has observed the
producer terminal. Treating every sink error as a new `TurnOutcome::Failed`
creates contradictory records: ReactAgent trace and final answer can say
Completed while the returned receipt says Failed.

## Decision

1. Keep `TurnOutcome` as the only execution terminal authority.
2. Add `TurnDeliveryOutcome` to the same `TurnReceipt` with
   `NotAttempted`, `Delivered`, `Closed`, and `Failed(AgentFailure)` values.
3. Classify a producer terminal before invoking the sink. A delivery failure
   after that point cannot rewrite execution outcome, final answer, message
   identity, usage, or compaction accounting. A non-terminal sink failure still
   cancels the invocation and yields framework `Failed` because no producer
   terminal exists.
4. `SinkControl::Closed` before a producer terminal is a driver-synthesized
   cancellation; after a producer terminal it only records delivery `Closed`.
5. ACP keeps Journal/Ledger before projection and observers, persists the
   complete receipt, and returns `end_turn` only for `Completed + Delivered`.
   Delivery failure is returned as a bounded protocol error without downgrading
   the stored execution status or terminal.
   `persist_run_settled` is the standard Prompt's single receipt writer;
   `run_spawned` cannot start a competing persistence task.
6. `RunReceiptWire` carries optional delivery fields for old persisted records.
   New records always write the delivery status and, when failed, the lossless
   `AgentFailureWire`. Missing fields remain `legacy_unknown` and are never
   inferred as successful delivery.
7. Durable write and recovery validate run identity, terminal/outcome/final
   fields, delivery/error pairing, and committed-versus-observed watermarks as
   one settlement record. A corrupt combination fails recovery instead of
   recreating conflicting terminal authorities.

## Alternatives rejected

- Letting a projector or observer own the execution terminal duplicates the
  driver and makes cancellation, trace, and provider state diverge.
- Making all projections best-effort hides delivery loss and prevents callers
  from deciding whether a result is safe to retry or display as delivered.
- Adding a new RunHandle operation for delivery status duplicates the receipt;
  SDK clients read the field from `RunGet`/`RunWait`.
- Introducing an asynchronous outbox here would also require new retry,
  retention, ACK, and cleanup policy. Those remain separate delivery-ledger
  work when a concrete consumer needs them.

## Consequences

The framework exposes two explicit, non-overwriting facts for every driven
turn. Adapters must check both before claiming successful execution and
delivery. `RunStatus` and `RunTerminal` remain execution projections; a run may
be `Completed` while its delivery is `Failed`, which is an observable and
recoverable delivery condition rather than a second execution terminal.

The driver watermark (`TurnReceipt.last_event_sequence`) is the last envelope
observed by the driver. The ACP Ledger watermark is the last envelope accepted
by that ledger; Journal or projection failure may make these watermarks differ.

## Verification

Focused driver tests cover all terminal kinds, terminal and pre-terminal sink
failures, Closed, missing terminal, and stream-start error delivery. ACP and
SDK host tests cover Journal/observer failure, wire validation, persistence,
and legacy receipt decoding. Full workspace and SDK contract gates remain
required before closing Finding #108.
