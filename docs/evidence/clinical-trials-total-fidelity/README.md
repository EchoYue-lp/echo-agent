# ClinicalTrials total-result fidelity candidate

This candidate follows published Channel main `554f2f50d3705d219519f306055a3d26d6a5211c`.
The existing Tool did not ask API v2 to include `totalCount`, then converted an
absent or malformed field to a successful `total_results: 0`. The captured
HTTP 200 default response contains one study but no totalCount. The same official
endpoint and query with `countTotal=true` returns one study and totalCount 386.
The four exact official response/receipt files were copied byte-identically
from the SDK lane. This is an observed response pair, not a fixed current total.

The Tool now requests the existing API flag and requires a non-negative integer
`totalCount`. A reported zero stays zero, and u64 maximum is preserved. Missing,
null, negative, fractional, string, boolean, array and object counts return the
existing `ClinicalTrialsSearchTool` execution error instead of inventing zero.
The HTTP client, timeout, body limits, endpoint, filters, fields and normalized
study projection remain the existing implementation. No nullable response API,
new SDK parameter, permission or secondary parser was added.

Private request-construction and normalization helpers make offline tests use
exactly the Tool production path. Before the two-line behavior repair, the
complete original normalization body was mechanically extracted and compared
after dedenting; it matched byte-for-byte. Three new regressions then failed
for the missing request flag, missing total and malformed total; the fourth
positive regression for 386 / one returned study, zero and u64 maximum passed.
This old-behavior fixture is not described as a byte-identical whole old-main
binary. Original, old-behavior fixture and new source bytes are recoverable
from base64 text with decoded SHA256 in receipt.json.

After the repair, the same four regressions pass with zero failures or ignored
tests. Formatting, all-target/all-feature echo_tools warning lint and the four
production panic lints pass. Exact logs are archived, including the real old
three failures. These focused results do not replace independent review,
full current-source workspace gates, remote CI, published main or three-language
new Host verification. DB/RAG evidence and its independent audit stay bound to
their own historical recoverable source and are not used to certify this delta.

English and Chinese research documentation now explains server total versus
page count and failure handling. There is no separate ClinicalTrials executable
example; deterministic production-parser regressions run in the existing
all-target test chain. Website research copies require the original sync-docs
producer once the final framework revision is published. No architecture
change occurred, so a new ADR is not applicable.
