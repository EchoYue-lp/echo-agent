# ADR 0046: Separate Turn Execution and Delivery Settlement

## Status

Accepted

- Date: 2026-09-14
- Owners: `echo-orchestration::runtime`, `src/acp`, and the external SDK Host consumer

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
8. `AgentChannelHandler` is also a driven Turn consumer. It creates a fresh
   transport stream/Turn identity for each inbound message, scopes the
   conversation by channel, conversation, and sender, and exposes the
   `TurnReceipt` through `drive_turn`. Its standard `MessageHandler` path
   produces an outbound reply only for `Completed + Delivered` with a final
   answer. The channel sink accepts envelopes without inventing a second
   terminal reducer; `SessionHandler` still owns session generation and the
   transport delivery fence. The raw Rust `Agent::chat` and stream APIs remain
   valid low-level contracts for callers without a driven Turn promise.

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
- Treating a channel's returned text or raw stream EOF as a Turn terminal
  leaves cancellation, producer failure, and usage without a receipt.

## Consequences

The framework exposes two explicit, non-overwriting facts for every driven
turn. Adapters must check both before claiming successful execution and
delivery. `RunStatus` and `RunTerminal` remain execution projections; a run may
be `Completed` while its delivery is `Failed`, which is an observable and
recoverable delivery condition rather than a second execution terminal.

The driver watermark (`TurnReceipt.last_event_sequence`) is the last envelope
observed by the driver. The ACP Ledger watermark is the last envelope accepted
by that ledger; Journal or projection failure may make these watermarks differ.
Channel receipt delivery refers to the channel event sink, not a remote QQ or
Feishu delivery ACK. The transport's generation fence remains responsible for
local queue/network admission and reset ordering; a remote send failure cannot
retroactively change the Agent's producer terminal.

## Industry Basis for Channel Adoption

Codex exposes a finite Turn completion result and separate progress/usage
notifications; Claude Agent SDK reports a final ResultMessage after streamed
progress. Both keep the terminal independent of merely observing output or EOF
([Codex app-server protocol](https://github.com/openai/codex/tree/main/codex-rs/app-server-protocol/schema/json/v2),
[Claude Agent SDK streaming output](https://platform.claude.com/docs/en/agent-sdk/streaming-output)).
Tokio's graceful-shutdown model separates signalling cancellation from waiting
for tasks to finish ([Tokio graceful shutdown](https://tokio.rs/tokio/topics/shutdown)).
The channel adapter applies these existing framework principles by awaiting
one `TurnReceipt`; it does not introduce another lifecycle authority.

## Verification

Focused driver tests cover all terminal kinds, terminal and pre-terminal sink
failures, Closed, missing terminal, and stream-start error delivery. ACP adapter
tests in this repository and external SDK Host tests cover Journal/observer
failure, wire validation, persistence, and legacy receipt decoding. Full
framework and independent SDK gates remain required before closing Finding
#108.
Channel adapter tests additionally cover a real driven receipt with identity
and usage, standard reply projection, producer failure, and cancellation.
