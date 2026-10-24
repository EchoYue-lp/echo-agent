# ADR 0091: Preserve explicit hook approval intent through the shared service

## Status

Accepted direction for Plan05 `hook-ask-semantics` on 2026-10-24, implemented
from framework `c3b56b4ee8a73565e6a2a621b9a7ddfb644ed3aa`. The focused-only
lease has qualified the behavioral regressions below. Full matrices, example
and consumer qualification, final gates and delivery remain pending their
separate resource allocation and acceptance.

## Context

The hook registry already reduces decisions with deny priority. The old
`PermissionStage` instead consumed Ask/RequireApproval into empty branches,
then entered ordinary permission checks. A later PermissionRequest Allow,
allow rule, approval cache, full-auto mode, or default read policy could
therefore authorize execution without the requested interaction. No service
and the feature-disabled stub also returned no check. Logging `ask` did not
prove that a provider was called.

This is a generic framework decision boundary. EKO presents the existing
requests; it does not own a second permission policy, queue or store. The
framework remains independently reusable without requiring another current
consumer. Existing provider/context/cache/receipt semantics are retained.

## References and alternatives

Official sources queried on 2026-10-24:

- [Claude Code PreToolUse](https://code.claude.com/docs/en/hooks#pretooluse-decision-control)
  separates explicit ask/deny from ordinary automatic approval; Ask in auto
  mode cannot be silently approved by the classifier.
- [Claude Code PermissionRequest](https://code.claude.com/docs/en/hooks#permissionrequest-decision-control)
  has its own allow/deny output contract. It is not this framework's identical
  event schema and does not remove our approved Ask/RequireApproval variants.
- [Claude Agent SDK permissions](https://code.claude.com/docs/en/agent-sdk/permissions#how-permissions-are-evaluated)
  puts explicit ask rules before ordinary bypass/allow and explains that an
  ordinary callback can be skipped by earlier automatic decisions.

These references support separating mandatory interaction from ordinary
permission resolution. They do not establish another product's unpublished
cache key, TTL or internal implementation, and do not replace our confirmed
requirement that hook Ask survive allow/cache/full-auto.

1. Add an explicit method to the existing PermissionService, sharing the
   handler and approval post-processing. **Selected.** It provides a real
   cross-crate entry without changing accepted context values or old check
   signatures.
2. Add intent fields to PermissionInvocationContext. Rejected for this slice:
   the struct is an accepted SDK value with explicit Host DTO construction;
   this unnecessarily expands the existing value contract.

Temporarily changing modes/rules, clearing approvals, or calling a separately
constructed provider from snapshot would change shared policy or duplicate
the service's receipt/rewrite/cache authority. Those are not substitutes.

## Decision

`PermissionStage` retains `Some(suggestions)` across the two hook events.
`Some([])` preserves RequireApproval; `None` retains ordinary behavior.
PermissionRequest Allow does not clear a prior Ask, while a Deny still
short-circuits. Suggestions from both events retain their text and order.

Snapshot's existing public checks and PermissionInvocationContext fields stay
unchanged. A crate-local adapter supplies the intent while reusing pending
rules, invocation identity/environment and the same cancellation select.

`PermissionService::request_approval_with_context` is the additive generic
public entry. It applies protected-path and applicable mode/rule denials,
requires a real handler, then uses the shared handler processing. It does not
read cached grants or accept an automatic mode/rule Allow for this call.
DontAsk without a matching rule still denies before the handler; a matching
Allow only permits requesting the fresh explicit approval, not skipping it.
Missing service, disabled human-loop, or a NullHandler explicitly fails a
mandatory interaction. Ordinary calls retain their previous behavior.

The shared handler path continues to apply user-updated arguments, recheck
protected paths, record explicitly approved scope, apply rule updates and issue
a receipt bound to final effective input. A later ordinary call can use the
resulting scope cache; another explicit Ask must request a fresh decision.
Hook strings become descriptive `SuggestedAction::Custom` entries alongside
the standard choices. Their text never implies a session/permanent grant.

## Impact and scope

The implementation belongs in `src/agent/react/run/pipeline.rs`,
`src/agent/snapshot.rs` and
`echo-orchestration/src/human_loop/service.rs`. No new queue/store, public
context field, feature, protocol, EKO policy or default approval provider is
introduced. The old registry deny reduction and resolved Finding remain.

The Rust method is available through the existing human-loop facade. It is not
automatically an accepted TypeScript/Python/Java SDK operation. The SDK owner
must distinguish new Rust inventory telemetry from accepted source/kind,
signatures, values, routes and its independently pinned Host. This slice does
not change SDK accepted JSON, generated output, wrappers or pin. New API
delivery is framework first, then explicit consumer adoption.

Cancellation keeps the existing snapshot owner and error projection; no
Other-to-Cancelled unification is claimed. Timeout/Deferred/rejection retain
the existing provider-to-denial mapping. Snapshot still supplies no default
deadline; provider deadlines and GUI pending-request cleanup are outside this
slice. NeedMoreInfo remains an unresolved approval result, not permission to
execute or a new interactive retry loop.

## Verification and recovery

Main-path regressions cover both hook events and both decision variants,
default reads, allow rules, matching cache, full-auto and cross-event Allow.
An observable provider entry precedes answers/cancellation; tool execution is
counted and must remain zero until approval. Further cases cover denials,
unavailable service/feature/handler, ordered suggestions/context, response
settlement, scope writes, rewritten-input protection and final receipts.
The old trace test remains trace coverage, not proof of interaction.

The test-only patch ran against the original three production files at the
fixed base, using only existing pipeline/permission APIs. Compilation succeeded;
the two regressions failed on absent provider entry and silent no-service
execution (Cargo exit 101). This is behavioral red evidence, not a missing-new-
API compilation failure. All nine candidate files were restored byte-identically
before green validation.

Focused green results, each Cargo exit 0 and compiler warnings 0:

| Selected scope | Passed |
| --- | --- |
| `echo_orchestration` service tests, including seven new cases | 28 |
| `echo_agent` explicit hook tests with human-loop and shell | 7 |
| Existing permission, shell receipt and cancellation controls | 9 |
| No-human-loop explicit approval failure | 1 |
| Exact baseline feature/filter command on the candidate | 5 |

These 50 executions cover 14 distinct new behavior tests. The new cancellation
case observes provider entry before cancelling; the old first-poll cancellation
test remains a narrower control, not evidence that it waited in the provider.
Runtime source required no compile/test repair after the source review.

Original logs, commands/exits, candidate hashes, test patch and cleanup receipt
remain in `.echo-semantic/local/hook-ask-semantics/`. After all readers ended,
the owned target was cleaned (exit 0, about 1.3 GiB). No Host artifact was built
or needed for this library-test batch. These focused results do not qualify
examples, the full feature/workspace matrix, pinned SDK consumers, final
independent review, semantic gates or exact-main CI. No old contract receipt
qualifies this implementation.

Recovery is a task-scoped revert against the source base above; it must not
delete user data or move protected baseline refs. English/Chinese HumanLoop
and Hook guides and demo03 use the same public entry. The coordinator owns
website mirroring. No source merge, push or release is asserted here.


## Focused development contribution boundary

On 2026-10-24 the coordinator separately authorized a local development commit
of these nine files on parent `c3b56b4ee8a73565e6a2a621b9a7ddfb644ed3aa`, under
Plan05 `plan:2e0b8f47-7ef8-4117-89bb-f1a2bc36cca8`, SHA-256
`7749cc72f8e358d43292fb3567977d2e477527d080fc91d4a2382e44b95c72be`.
This saves the focused-qualified source contribution; it does not merge source
or complete the Plan. The earlier pending-final-gates wording records the
focused validation checkpoint. Saving the contribution requires the normal
full task semantic/reference and staged-artifact gates plus final review.

The three runtime files, executable example and four bilingual guides still
match their pre-focused hashes. Only this ADR's factual evidence/qualification
text changed afterward; no behavior, public API or test semantics changed in
this closeout. The existing 50 executed checks therefore retain their stated
scope, without a new Rust run or a new runtime acceptance claim. Direct rustfmt
checks cover the four modified Rust/example files without compiling them.

HumanLoop and Hook guides are synchronized in both languages. `demo03_approval`
calls the same real service method and remains an explicit Cargo example target
covered by the existing workspace all-target verification chain. Its new bytes
have not yet been compiled or executed by this focused library-test batch;
example qualification, the complete workspace/feature/public-API matrices,
pinned SDK consumer checks and exact main/CI remain for the later separately
leased integration. No SDK accepted contract/pin or generated language surface
is changed here. Website source mirroring remains with the coordinator, after
actual delivery. Plan05 remains `ready`.

No compile/test lease is held during this closeout, so the same-source 50 tests
are not repeated and no new Cargo artifacts are produced. The earlier clean
exit0/1.3GiB receipt and outside-target originals remain recoverable; target
absence is rechecked before saving. This stage needs no additional cargo clean.
The independent LSP bridge and ZIP closeout contributions are neither combined
nor modified in this tree. Push, main merge and publication remain excluded.
