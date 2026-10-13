# ADR 0061: Recover ACP Turn Allocation Within the Session Authority

## Status

Accepted

- Date: 2026-10-12
- Owner: `echo_agent::acp::AcpSession`

## Context and references

ACP `AcpSession::begin_turn` allocates native Run identities as
`<session>:turn:<sequence>`. A newly constructed Session starts at sequence zero.
An adapter can restore the same Session and publish its historical Runs from a
durable journal; without restoring allocation, the next Turn reuses
`<session>:turn:1` and collides with the retained historical Run.

The identity owner must retain that authority across restoration:

- [Official ACP Session Setup](https://agentclientprotocol.com/protocol/v1/session-setup)
  defines loading with the existing Session ID, replay of conversation updates,
  and continuing interaction after the load response. It does not delegate
  internal Run identity allocation to a Client or adapter.
- [Official Claude Code session lifecycle](https://code.claude.com/docs/en/how-claude-code-works#resume-or-fork-sessions)
  distinguishes continuing the original Session from creating a fork. The
  original identity and history remain relevant when a Session resumes.

These references establish the continuity requirement; they do not specify the
framework's native Turn suffix. Keeping parsing and allocation together is our
implementation choice for that requirement. Both official pages were read on
2026-10-12. SDK three-language restart reproductions establish the actual
consumer failure; framework unit tests do not replace those end-to-end results.

## Options considered

1. Have the SDK parse Turn IDs and implement its own counter. Rejected: it makes
   the adapter a second allocation authority and couples it to framework syntax.
2. Mint random replacement Run IDs or delete historical Run/journal records.
   Rejected: it changes identity semantics or loses recovery evidence instead
   of restoring the existing authority.
3. Add another persistent counter schema or execution state machine. Rejected:
   the existing retained native Run identities already provide the allocation
   frontier, and `TurnSlot` already serializes allocation and admission.
4. Reserve recovered native identities through `AcpSession`. Selected.

## Decision and layering

`AcpSession::reserve_recovered_turn_ids(&[String]) -> Result<()>` is a framework
recovery primitive. It is valid for any adapter restoring native ACP Turns and
contains no EKO product policy. The adapter selects and reads authoritative
durable records, passes the unmodified Run IDs, and handles publication rollback.
It calls reservation before publishing the restored Session or admitting Turns.

Reservation holds the same `TurnSlot` mutex as `begin_turn`, rejects closed or
active Sessions, and validates every ID before changing the counter. Each ID
must have this Session's exact native namespace and a canonical positive `u64`
suffix. Other Session, Task, and Subagent identities are errors, rather than
being guessed into ACP Turns. A valid `u64::MAX - 1` suffix allows one final
checked allocation. Its resulting `u64::MAX` identity can itself be restored
without rejecting the historical Session. Only subsequent Turn admission fails
explicitly with exhaustion; it creates no active lease or new Run identity.
Values above `u64::MAX` remain invalid and leave the complete batch uncommitted.

The committed frontier is the maximum of the existing counter and the complete
validated batch. Duplicate IDs and repeated or lower reservations are idempotent.
Invalid batches leave the counter unchanged. The operation does not create a
live lease, restore cancellation, execute Effects, infer terminal state, or alter
receipts or journal contents. Session close and Turn admission keep their
existing owners.

## Impact and validation

- The framework adds one Rust recovery API without a wire/schema change or new
  dependency. It belongs to Rust/Host adapter recovery, not automatically to
  the accepted three-language API surface.
- SDK Host must explicitly adopt the API when upgrading its pinned revision;
  its recovered handle generation and historical terminal projection remain
  SDK responsibilities.
- Persistent regressions cover resumed allocation after Turn 1, Unicode Session
  namespace, duplicate/lower reservations, atomic invalid batches, overflow and
  near-exhaustion, restoration of the last actually allocated native identity,
  active/closed fencing, and reservation/admission racing on the
  one lock. An API rustdoc example is part of the executable documentation.
- Existing ACP demonstration code creates fresh Sessions and does not restore
  retained Run IDs, so no demonstration behavior changes. The website has no
  accepted SDK recovery API description to synchronize for this framework-only
  primitive. SDK documentation and real Host restart evidence are owned by the
  SDK delivery.
- Focused framework tests, full merge gates, and real Host three-language restart
  verification are required before delivery; adding tests or formatting source
  does not by itself establish those results.
