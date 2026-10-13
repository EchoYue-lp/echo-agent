---
schema_version: 1
id: evidence.lsp-response-value-fidelity-investigation
kind: evidence
observed_at: 8843afd5029afba9be8a6551de4ab0a93951311f
source_refs:
  - echo-integration/src/lsp/client.rs
  - echo-integration/src/lsp/jsonrpc.rs
  - echo-core/src/lsp/types.rs
  - echo-integration/src/lsp/manager.rs
  - docs/adr/0082-lsp-response-value-fidelity.md
supports: [behavior.protocol-projection, rule.protocol-role-separation]
limitations:
  - Focused red/green does not prove combined framework full gates or final delivery
  - Three-language SDK and exact published-main consumer verification remain pending
---

# LSP response projection investigation

## 支持的结论

At framework base 95ec66c6, hover constructs range None; definition/references/
completion use unwrap_or_default after serde errors; Option<Value> deserializes
null into None; completion core.kind is mandatory. Whole-framework/CLI searches
found no additional CompletionItem constructors or kind consumers.

## 来源与范围

Official Microsoft LSP 3.17 source was retrieved via GitHub contents API,
repository revision f8c4bc9834703b7317c98c8e8053a28fa4b6b997. Content blob SHAs:
hover 258dd88366a9d82452e415a380c22c7c9af0dc56;
definition bf9693bf99e75205b657fea67c427a021fc8e5db;
completion 9a76ade2a0923e7b2d16c259afbe1c4ffaa08de1;
references e7d2e030383c581e32b8abf1335542779feaf8e0;
position 49f13964bc12c6e65c35a349c33e7e3314245c60.
ADR 0082 links immutable official sources. Public browser lookup returned 404 and
raw HTTP timed out; successful API retrieval supplies the actual references.

Existing Python MOCK_LSP was extended instead of introducing a second test
server implementation. Python syntax and ten actual Content-Length framed
responses passed: initialize, null, missing-result, Unicode hover/range,
standard completion insertText, absent/null completion kind, coordinate
overflow, invalid JSON-RPC version and ambiguous result+error. These prove fixture validity only. Six prepared
Rust tests must run against old and repaired request paths before repair claims.

## Focused implementation proof (uncommitted task candidate)

Old implementation against the six prepared tests: 1 passed, 5 failed,
exit 101 (`/tmp/framework-lsp-response-red.log`). Failures directly expose
range discarded, null treated as missing, completion discarded and malformed
hover silently succeeding. The unchanged Location regression passed.
After the framework conversion repair, the same six tests pass, 0 failed,
0 ignored (`/tmp/framework-lsp-response-green.log`, exit 0).

`cargo test -p echo_integration --features lsp lsp --locked`: 26 passed,
0 failed, 1 existing ignored installed-LSP opt-in; exit 0,
`/tmp/framework-lsp-focused.log`.
`cargo clippy -p echo_integration --lib --tests --features lsp --locked --
-D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic
-D clippy::unreachable`: exit 0, no warnings,
`/tmp/framework-lsp-clippy.log`.

SHA receipts, source files and official inputs are recorded in
`/tmp/framework-lsp-response-verification.json`. These commands mistakenly used
an isolated cold target path under echo-agent-worktrees rather than the shared
SDK warm target. The mistake created 521 MiB of task-owned output; no source
was removed. The logs remain actual command proof, but are not SDK shared-cache
or final published-revision evidence. Later root isolated LSP feature, demo55 example and fmt-all/check commands
used the correct shared warm target; all exited 0 without warnings. Independent
review, combined full merge gates and accepted SDK signature synchronization
remain open.

## 已知缺口

Root isolated-feature and formatting/check passed; demo55 passed 1/1 without
skips. Independent review remains pending.
Final combined framework and accepted SDK three-language delivery are separate
required proof. No historical Finding closure is inferred from focused evidence.

## Independent review counterexamples and repair

The original six-test green candidate did not cover raw error presence or
Serde sequence visitors. Reviewer fixed source hashes and found two Important
counterexamples. With two new real stdio tests, that candidate passes six and
fails two, exit 101 (`/tmp/framework-lsp-review-red.log`). A null error member
incorrectly returns hover success; malformed code/missing message frames wait
for the test's 2-second bound; a tuple range incorrectly returns a Hover range.
This is real code execution, not protocol fixture simulation.

Repair keeps the existing framework reader/pending map and sends a typed local
ServerError for a malformed matching response. Present error members require
object shape, including null rejection. Native Location/Range/Position object
shapes are checked before existing serde types, preserving additive fields and
rejecting mixed tuple arrays atomically. No second reader, runtime, SDK parser
or public type was added. After every malformed error, a valid next request
succeeds on the same initialized client.

Final eight response-fidelity tests: 8 passed, 0 failed, 0 ignored;
`/tmp/framework-lsp-review-green.log`, exit 0. Full LSP subset: 28 passed,
0 failed, 1 existing installed-server opt-in ignored;
`/tmp/framework-lsp-final-focused.log`, exit 0. Focused Clippy and root isolated
LSP feature checks exited 0 without warnings. Final source/official/log SHA
manifest is `/tmp/framework-lsp-response-verification.json`; the official API
blobs are independently verified with Git blob SHA1 and content SHA256 in
`/tmp/framework-lsp-official-inputs-provenance.json`. Historical two-object
reanchoring compares 27 unique unchanged base blobs (scope-specific count).

## Diagnostic follow-up candidate continuity

Current isolated LSP source includes the separately documented diagnostic repair.
The final full LSP32 suite also executes all eight request-fidelity regressions.
Historical red/green logs retain their original scope and are not renamed as
notification proof. This current source anchor tracks the combined local LSP
candidate, while delivery and complete SDK consumer evidence remain pending.

历史定位说明：原未提交 source8896e508 的846个源码/文档文件已逐字节重建，唯一 source_digest 精确匹配，并保存到本地 recoverable Git snapshot `8843afd5029afba9be8a6551de4ab0a93951311f`（refs/echo-semantic/snapshots/sdk-framework-reviewed-8896）。这是内部审查证据快照，不是交付分支提交、公开 dependency 或 main。原结论与限制保持；新 parent-drop 源码不借此历史 pass。
