# ADR 0095: Structured remote Tool failures

## Status and scope

Accepted implementation direction; source implementation and actual Rust/SDK
qualification are pending. Date: 2026-10-25.

The project coordinator selected this technical direction within the authorized
tool-governance Goal. It does not accept a new SDK value, change the SDK's pinned
framework revision, or authorize publication. The stage design is the workspace
`docs/supreme/specs/tool-failure-classification/design.md` (SHA-256
`2396bd58ae3f66a66ae5decc7ba649d5c7081243cc49320f2c53a9e67e26163c`);
the precise implementation source is Plan `8a4ad068-ab49-43c3-b29d-92bbd0b8a6c7`
at workspace checkpoint `b47ef77223f59c9ab9e98888fd317e4fd75535a8`.

## Context

Search providers return the existing `Result<Vec<SearchResult>, ReactError>`.
They currently erase HTTP/transport facts into `ToolError::ExecutionFailed`
messages; WebSearchTool then labels every provider failure Transient/Retry with
a fabricated one-second hint. Authentication, quota, malformed responses and
network failures therefore cannot retain their actual recovery contracts.

ToolFailure already owns category, recovery, side effects, retry_after_ms,
idempotency and postconditions. ToolManager already owns retries, permits,
admission, cancellable waiting and started/draining settlement. Neither is
replaced. ToolFailure does not contain tool identity or a display message.

## Decision

Add `ToolError::RemoteFailure` with `tool: String`, `message: String`,
`http_status: Option<u16>` and `failure: Box<ToolFailure>`. Tool/message each
have one owner; the existing ToolFailure owns recovery facts. The box contains
the additional payload without putting reqwest types in echo-core. Existing
ExecutionFailed/Timeout/Cancelled and SearchProvider signatures stay intact.

Providers and their original body-reading boundaries classify facts while the
HTTP status, headers or typed reqwest error still exist. Core receives ordinary
values. `ToolFailure::from_error` retains the structured payload and can only
narrow recovery for the caller's side-effect scope; Possible/Confirmed effects
are never downgraded and idempotency is never invented. The original AgentFailure
projection keeps Tool domain, terminal cause and optional HTTP status.

WebSearchTool preserves the resulting typed failure instead of overriding it.
Timeout and cancellation use existing error variants. Unclassified legacy
ExecutionFailed remains subject to the original conservative classification;
neither exception text nor provider names determine retryability.

### Classification

- Existing pre-request query/limit/cursor-type checks retain InvalidArguments;
  nonempty cursor decoding/version/fingerprint/offset checks remain after the
  original fetch in paginate. No cursor validator or cache is added.
- Invalid endpoint/request construction, HTTP401/402/403/404 and other permanent
  client failures stop. HTTP408 is Timeout; HTTP429 and HTTP500..599 are Transient.
  Known timeout/connection causes retain their typed classification. Original
  content/decoding failures stop; original size-limit errors retain their type.
- Cancellation/interruptions are Cancelled; ExecutionNotSettled stays an
  observation, not a terminal failure or permission to repeat physical work.
- Diagnostics exclude credentials, raw error bodies and sensitive URL details.

DDG retains its existing successful-HTML anti-bot/empty-results fallback to the
Instant Answer API. One provider/manager logical attempt can therefore perform
one or two original HTTP sends. Strategy fallback and manager failure retry are
counted separately. DDG retains reqwest text/json decoding and body-size behavior;
shared readers retain their own limits. Body-size governance is a separate task.

### Retry hints and lifecycle

Normalize valid Retry-After seconds or HTTP-date to the existing retry_after_ms.
Missing/malformed hints stay None; valid zero/past dates are Some(0). HTTP-date
parsing reuses a mature parser; a direct dependency must be declared and checked
even if its package already exists transitively in Cargo.lock.

ToolManager remains the only retry executor for typed results and Err paths.
Both retain the hint, retry count, admission, cancellation, side-effect policy,
stream-output replay restrictions and original drain owner. Keep the existing
30-second maximum delay: when the server minimum cannot fit it, return the
original typed failure instead of retrying early. No scheduler or queue is added.

## Alternatives and external references

Adding optional fields to existing ExecutionFailed would change accepted field
shapes and every existing constructor. Parsing error strings loses facts and
does not solve classification. Borrowing LLM/Channel errors misstates ownership.
All are rejected in favor of one additional carrier using ToolFailure.

Official [reqwest Error](https://docs.rs/reqwest/0.12.28/reqwest/struct.Error.html)
retains typed timeout/connect/status facts and supports stripping URLs;
[Response](https://docs.rs/reqwest/0.12.28/reqwest/struct.Response.html) exposes
status/headers before body consumption. Smithy's
[ProvideErrorKind](https://docs.rs/aws-smithy-types/latest/aws_smithy_types/retry/trait.ProvideErrorKind.html)
separates optional retry facts from display text. This framework reuses its
existing failure taxonomy and execution owner rather than importing another one.

## Consequences

Rust consumers that exhaustively match ToolError must handle the added variant;
existing constructors and provider trait signatures do not change. HTTP facts
become available to the existing recovery and adapter paths without another
network dependency in core. Public inventory changes must be distinguished from
explicitly accepted SDK identities. Source review alone does not qualify this
change or remove the original effect/settlement obligations.

## SDK, documentation and validation

ToolError is already in the SDK accepted contract. A new Rust variant and its
fields do not automatically become accepted source identities. The existing
collector records variant/field identities separately and removes structural
child IDs from retained signatures; only actual candidate collection can prove
the complete retained contract. The existing ToolFailureWire/AgentFailureWire
can project failure facts without assuming new wire fields or a protocol bump.
The current SDK pin and language acceptance stay unchanged until separately
qualified. Static inspection is not 5352-item/431-route byte parity.

Before/after regression must exercise real provider/Tool/manager behavior and
count logical attempts and endpoint HTTP sends. Cover statuses, valid/invalid
Retry-After, cancellation, content errors, original DDG fallback, cursor rejection
and successful pagination; also cover Err/typed-result side effects and both
ReAct and a Host built from the actual candidate. Old-pinned Host tests cannot
qualify the new source. Public API/feature/dependency gates and examples remain
required. This ADR does not claim any such run has occurred.

Exa's independent4343 commit is unchanged and unmerged. The separate P1-A output
budget source and original evidence remain preserved; sharing a final validation
batch does not merge their acceptance claims or create a completed old contract.
