# ADR 0099: Personal Weixin protocol and durable no-stream admission

## Status

Accepted A+A2 design. The scoped Framework source review and focused checks
below passed; application recovery, native/platform and main acceptance remain
separate.
Root reserved 0099 after checking qualified 790 and local ADR occupation. The
qualified `0098-project-file-system-adapter.md` and the old Weixin 0098 frozen
source/review originals are preserved. Source definitions and direct rustfmt do
not establish consumer, native authentication, delivery or exact main acceptance.

## Context

The framework already owns `ChannelPlugin`, `ChannelManager`, `SessionHandler`,
original generation/fences, cancellation, the bounded sender and physical joins.
Personal Weixin needs native QR/authenticated-poll/send protocol. The fixed
first-party reference is [Tencent/openclaw-weixin 874637a0dbc24050f72d1385add4fc6170a2d726](https://github.com/Tencent/openclaw-weixin/tree/874637a0dbc24050f72d1385add4fc6170a2d726),
version 2.4.10 under its [MIT license](https://github.com/Tencent/openclaw-weixin/blob/874637a0dbc24050f72d1385add4fc6170a2d726/LICENSE).
Its host is not a Rust SDK or evidence that another client is authorized.
Authentication operations, scanner policy, durable task inputs, recovery and cron
notification selection remain application responsibilities.

An opaque response stream can wait for arbitrary time on HITL. Holding transport
capacity until response EOF blocks later `/reply` behind ordinary inputs. Letting
ACK release capacity while retaining one response task per input instead allows
unbounded producers. The original host, rather than the receiver, must retain
queued DATA and create long driver/renderers only after its original claim.

## Options

1. Install another Gateway/host or create a second inbox/executor: rejected;
   duplicates existing ownership and does not implement the native channel.
2. Hold each response until EOF, or release capacity on ACK but retain unlimited
   response tasks: rejected; respectively blocks controls or violates bounded
   producer retention. A different arbitrary response quota does not fix it.
3. Explicit fallible service binding plus no-stream durable admission: selected.
   Reuse the same factory/session/host, actual permits and original close owners.

## Decision

`MessageHandler::bind_durable_ingress(service_cancel)` is synchronous and
fallible. `admit_with_ingress(msg, cancel, receipt)` returns `Result<()>` without
returning a stream or waiting for model/HITL/output. Both default to Unsupported;
there is no legacy invocation as a capability probe. The unreleased old
`handle_stream_with_ingress` path is removed. Existing scalar/stream/reply APIs
and QQ/Feishu remain available with their previous opt-in behavior.

`SessionHandler::with_durable_ingress()` declares the factory capability. The
same wrapper binds one service child before transport acceptance. Each original
session generation and inner handler receive the same session child. Reset,
expiry, admission and close reuse one setup path and the original map/fences.
Inner bind failure stays with the original map entry and close obligation; it
cannot silently drop the factory owner, execute legacy effects or publish ACK.
Another service cannot rebind a live wrapper; a closed wrapper cannot revive.

The Weixin start path binds before accepting, spawning or polling. Parent Stop
first reaches the host through that child, closing new claims. Child cancellation
does not cancel its parent. Poll's original JoinSet contains only actual admit
calls, bounded by the existing 32 ordinary + 8 control permits. The permit stays
inside the real call until return, even after ACK; completed metadata is reaped
before each spawn. No transport response loop or per-input output waiter remains.
Media rejection feedback still uses the same original bounded sender.

Only the original input/control owner publishes `DurableInput`, `ControlHandled`,
`TerminalRejected` or `ControlRecoveryRequired`. The last fact means a durably
retained uncertain control effect, not success. Return, error, EOF, drop and
observer cancellation do not ACK. Strict batch cursor advancement still requires
every original receipt; a later ACK never passes an earlier Unknown.

A durable host persists queued DATA, publishes its original reply route and wakes
its existing claim owner. It retains actual long outputs/HITL and original sender
context/fences under its own execution limits; no admission spawns an output
waiter. Immediate control replies use that same sender. A framework-intercepted durable
reset keeps the original provider message ID in the existing
`OutboundMessage.reply_to` before handing the confirmation to its inner host; the
host can resolve and validate that exact message association, including explicit
None, without borrowing latest context. Legacy scalar/stream reset paths retain
their original behavior. `has_pending_work`
protects retained host work from idle pruning. Stop awaits admissions, original
send effects and the same handler close, including host drivers/renderers and
recovery initialization. Cancelling a stop waiter never detaches saved handles;
unknown settlement keeps its original owner and prevents Stopped/replacement.

`WeixinDeliveryHandle::validate_message_context(target, message_id, expected)`
validates the existing ProtocolState association without modifying it. Explicit
None needs a real persisted record containing `context: null`; missing record or
missing field is not absence. Some must match account/credential generation/peer,
message identity and exact reference, with the original readable content-addressed
token record. Missing/corrupt identity or token fails without HTTP, latest lookup,
state repair or secret return. Proactive sends and original captured-message
references retain their separate explicit APIs and the sole send producer.

The native protocol preserves authenticated-poll evidence, -14 reauthentication,
canonical account/peer identities, safe HTTPS/origin handling and secret redaction.
Send business acceptance remains Accepted/Rejected/Unknown. HTTP200, enqueue and
start are not success, and Unknown never authorizes automatic replay.

## Consequences

No new store, inbox, task FSM, queue, quota, executor or lifecycle manager is
introduced. The generic host must actually implement durability and physical
settlement; an in-memory ACK or EKO-specific empty stream is not that contract.
Queued DATA/index/route storage is not claimed constant memory. If the original
host cannot persist or accept input, the result remains Unknown and strict cursor
stops. Control liveness is conditional on available durable admission, not a
promise that finite/full storage can accept every arbitrary prefix.

A2 application recovery keeps one non-secret binding in the original Persisted
payload, including exact context presence, intent and saved workspace generation.
The original authority must positively prove never-claimed input and stable
account/context/environment before restoring its original claim/reply path.
Claim/effect/send uncertainty remains RecoveryRequired/Unknown with no replay.
These EKO policies and TaskRun resume fields are not added to Framework models.

## Testing and synchronized surfaces

Defined regressions use the real local HTTP poll/decode and sender entry, the
actual SessionHandler and a generic file-backed host fixture: 40 inputs and 128
single-message pages precede `/reply`, queued inputs are fsynced DATA, one original
claimed long response sends multiple chunks, and Stop waits its actual join after
a first waiter is cancelled. Another case holds admission after ACK to verify all
32 actual permits remain owned. Receipt drop/business Unknown, Unsupported start,
factory mismatch, child/parent cancellation, reset fences and context presence /
None / Some / missing or changed token remain distinct controls.

The focused Framework batch passed 135 channel tests (including all 10 new or
renamed NoStream cases), 12 root-facade tests and two compile-only doctests.
The existing demo38 typecheck, channel all-target warnings lint, strict
production panic lint, channel/featureless checks and workspace fmt/check passed.
Three precise lint repairs retained the initial warning and failed Clippy
originals. A later exact reset regression actually failed with missing
`reply_to` (None versus the original message ID), then passed after the
durable-only field forwarding fix. The full 135 channels, scoped all-target
warnings/strict production lints and fmt/check passed again, with another exact
clean. Earlier root-facade/doctest/demo/featureless results keep their original
epoch: no public API, legacy branch or example source changed. These results cover the retained Rust inputs, unchanged in this
documentation-only follow-up; they do not replace the full merge matrix.
Raw receipts and source identities are available only in this authoring worktree
under `.echo-semantic/local/weixin-no-stream-build/focused-actual/`; they are
not distributed runtime assets. The QQ/Feishu demo keeps its actual handler;
comments do not opt `AgentChannelHandler` into durability. Bilingual channel docs
track the API; website mirrors wait the coordinator's qualified producer. SDK
accepted contracts, real QR/login/messages, native GUI and exact main CI remain
separate and unverified by this source change.
