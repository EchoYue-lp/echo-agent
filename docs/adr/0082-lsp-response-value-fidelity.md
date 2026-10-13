# ADR 0082: Preserve accepted LSP result values at the framework boundary

## Status

accepted

The SDK governance goal authorizes this contract repair. Focused implementation and red/green validation are recorded below; this
record is not full-gate or delivery evidence.

## Context

The live `StdioLspClient` request paths discard Hover.range, turn malformed
Location and Completion responses into empty lists, and decode JSON-RPC
`result: null` as though the result member were missing. Completion responses
also use official numeric kinds and `insertText`, whereas the simplified
framework type previously required an enum kind and snake-case deserialization.
This breaks framework consumers and the already accepted SDK LSP projections.

## References and comparison

Microsoft's LSP 3.17 specification is the authority, retrieved through the
GitHub contents API at official repository revision `f8c4bc9834703b7317c98c8e8053a28fa4b6b997`:

- [Hover](https://github.com/microsoft/language-server-protocol/blob/f8c4bc9834703b7317c98c8e8053a28fa4b6b997/_specifications/lsp/3.17/language/hover.md): result is Hover or null; contents supports MarkedString, arrays and MarkupContent; range is optional.
- [Definition](https://github.com/microsoft/language-server-protocol/blob/f8c4bc9834703b7317c98c8e8053a28fa4b6b997/_specifications/lsp/3.17/language/definition.md): Location or Location array or null; LocationLink requires advertised linkSupport.
- [References](https://github.com/microsoft/language-server-protocol/blob/f8c4bc9834703b7317c98c8e8053a28fa4b6b997/_specifications/lsp/3.17/language/references.md): Location array or null.
- [Completion](https://github.com/microsoft/language-server-protocol/blob/f8c4bc9834703b7317c98c8e8053a28fa4b6b997/_specifications/lsp/3.17/language/completion.md): array, CompletionList or null; kind is optional integer 1..25; insertText is camel-case.
- [Position](https://github.com/microsoft/language-server-protocol/blob/f8c4bc9834703b7317c98c8e8053a28fa4b6b997/_specifications/lsp/3.17/types/position.md): UTF-16 code units are the mandatory default when no other encoding is negotiated.

The official Hover and Completion forms and the existing framework structs are
sufficient. No new public parser API, dependency or SDK-specific parsing layer
is required. Cached lsp-types 0.97.0 independently uses the same optional kind,
Hover range and response variants, but is not a new dependency.

## Decision

Keep all protocol decoding in the existing framework request paths. The SDK
Host projects the resulting framework values and error; it does not infer
ranges, rewrite positions or repair malformed server results.

A present JSON-RPC null remains `Some(Value::Null)`; only an absent result
member means no result. Nullable method results produce None or an empty list.
Malformed required fields, coordinates outside LSP uinteger, reversed ranges,
arrays, JSON-RPC result/error signatures or completion values return the
existing typed LspError::ServerError with the method context, without returning
partial success or copying arbitrary server contents into an error.

Preserve Hover range and original zero-based coordinates. The client uses LSP's
UTF-16 default, so a Unicode position is not a Rust byte index. Hover strings,
markup values and ordered marked strings remain textual values; language-tagged
marked strings render into Markdown with safe code fences.

Change only CompletionItem.kind to Option<CompletionItemKind>. Absent/null kind
remains None; present numeric kinds map explicitly and unknown/invalid values
fail. Never manufacture Text. Decode insertText and documentation into the
existing accepted fields. CompletionList validates isIncomplete and items, then
uses the existing Vec-returning projection; the simplified API does not expose
completion-list metadata, resolve edits or incremental result streams.

LocationLink remains unsupported because this client does not advertise
linkSupport; a server sending an unsupported shape receives an explicit error.

## Alternatives

- SDK-side decoding would duplicate the framework's protocol projection and
  leave Rust consumers broken, so it is rejected.
- Keeping mandatory kind and inventing Text destroys optional-field evidence;
  rejecting a legal missing kind loses accepted behavior. Both are rejected.
- Replacing the whole public LSP model with full lsp-types would expand the
  contract and dependency surface beyond this value-fidelity repair.

## Impact and validation

The kind field signature changes within an existing accepted identity; it does
not accept additional Rust public identities. Whole-framework and CLI searches
found no other CompletionItem constructors or kind consumers at base 95ec66c6.
SDK contract generation must explicitly record this optional signature and the
three clients must preserve null; full inventory remains telemetry.

Eight focused Rust regression tests use the existing real Python stdio fixture:
range/Unicode, legal nullable results, malformed result errors, locations,
standard completion fields/optional kind, and JSON-RPC null-versus-missing.
The fixture's Python syntax and ten framed responses passed. The original six tests pass after five genuine pre-fix failures. Independent
review found error-member presence/drop and Serde tuple-shape counterexamples;
two new stdio regressions failed before the corresponding repair, then all
eight passed. The full LSP subset passed 28 tests with one existing opt-in
ignored, and focused Clippy passed without warnings. The existing demo55 executable example, root isolated LSP feature and
workspace formatting/check passed. Combined full merge gates, accepted SDK
consumer proof and independent review remain pending. Never interpret this record as exact-main or three-language evidence.

Formal English/Chinese LSP documentation changes belong in this repository.
The existing executable demo55 has no kind constructor or altered API call, so
its code needs no duplicate example; its existing test entry still must run.
The website mirrors those formal documents and must sync after the parent
publishes a clean verified framework successor, without bypassing its dirty
source guard.

## Independent review corrections

A present error:null is invalid, not an absent error. A malformed response with
an existing numeric request ID now settles exactly that pending request with
ServerError through the existing reader; it is not dropped until a transport
timeout and does not close other requests or the process. Unknown IDs are not
redirected to another request. Error objects require object shape before typed
serde decoding; tuple/array errors are not accepted.

Range, its two Positions and every Location require native JSON object shapes
before reuse of the existing types, because serde's generic struct visitor also
accepts arrays. Mixed valid/tuple Location arrays fail as one result. Additive
unknown object fields remain tolerated. A subsequent valid request is covered
in the malformed-result tests, proving request-local errors leave the existing
initialized client available.

## Diagnostic notification fidelity follow-up

Official Microsoft [Diagnostic](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/types/diagnostic.md)
(blob 38bd1b3d3d1ce2fee270bf1759255146e41f9a6b) and
[publishDiagnostics](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/language/publishDiagnostics.md)
(blob fbba90a322b804cd296daefbcb45d6aabf05dc61) were read in full.
The notification replaces a document's complete diagnostics; valid [] clears,
not merges. Severity is optional integer 1..4 and code is signed i32 or string.
The old generic serde path silently turns legal and malformed notifications
into [] because severity is a required Rust string enum and code is String.

The user's accepted-value governance authorizes Diagnostic.severity becoming
Option<DiagnosticSeverity> and code becoming Option<DiagnosticCode>, with a
closed tagged Integer(i32)/String value union. Numeric and text codes retain
their distinction, and missing/null remains None. This is an explicit supporting
field-value contract, not automatic acceptance of all new Rust helper identities.
The actual LSP tool consumer renders missing severity as UNSPECIFIED and uses
saturating line/column display arithmetic; no false Error fact is fabricated.

The existing per-URI diagnostics cache owns Result<Vec<Diagnostic>, String>.
A complete invalid update replaces old success with a typed retrieval error,
not [] or last-good. Next valid update replaces the error; [] explicitly clears;
other URI entries are independent. Native object/range/position checks reuse
this ADR's sole framework parser, and the whole diagnostic array validates
before insertion. No separate error cache, SDK decoder or authority is added.

A malformed envelope without a nonempty string URI cannot be attributed safely.
It ends the existing reader using the existing runtime/process settlement owner,
records last_error, rejects pending requests, closes the actual child, and makes
subsequent diagnostics retrieval fail explicitly. Addressable malformed values
remain URI-local and do not end the connection. Teardown clears the same cache.

Current accepted SDK Diagnostic identities contain range, severity, code,
message and source; no relatedInformation field is accepted or implemented.
The client therefore advertises relatedInformation=false. This corrects false
capability advertising without removing an accepted Diagnostic field. Full LSP
related-information/tags/data/resolve support is not silently claimed.

Four prepared real stdio regressions cover legal optional severity/code bounds,
atomic error/recovery/clear and URI isolation, fatal unattributable notifications,
and honest initialization advertisement. They use notification followed by a
request-response ordering barrier, not timing sleeps. The four regressions fail on the previous implementation and pass after this
notification repair; the whole LSP subset passes 32 tests with one existing
installed-server opt-in ignored. Focused/consumer checks and final independent
review, combined gates and actual SDK consumer proof are tracked separately.
Earlier request-fidelity checks do not prove this notification follow-up.

## Diagnostic terminal admission review correction

Independent review found a race after begin_terminal closes admission and clears
cache but before the existing cleanup owner has awaited actual child exit.
Runtime.running/initialized still correctly describe a live process in that
interval. Checking only initialized can fabricate [] from the cleared cache.
A read that started before terminal can also wait on the cache across this race.

Diagnostics therefore acquires the existing cache, rechecks the lifecycle fence,
then checks the existing runtime admission/initialized/cause under its lock.
That synchronous section is the read's linearization point: a terminal that has
already won returns its ServerError, while running/PID are never fabricated as
settled before actual exit. No second state or error authority is introduced.
Two real stdio tests hold the actual Child mutex to control teardown; one reads
after admission closes, the other holds cache to suspend an already-started
read across fatal notification. Both fail with old false clean success and pass
after repair. All six diagnostic tests and the complete LSP subset remain green.
