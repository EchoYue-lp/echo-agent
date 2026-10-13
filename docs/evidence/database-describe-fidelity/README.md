# Database and RAG value fidelity candidate

The candidate starts at published framework `77de6ca90ad842eb4b7b2921a4baa954f41cccfe`.
It corrects two existing Tool result conversions without adding public APIs,
query engines, stores, permissions, SDK decoding logic or RAG indexing behavior.

## Failure, authority and repair

`DescribeTableTool` calls the existing read-only query path and shared
`format_db_rows` converter. SQLx 0.8.6 preserves column declarations separately
from each runtime value. The actual local SQLite PRAGMA trace in `before.log`
shows declared `NULL` columns with runtime `BIGINT` and `TEXT` values. The old
converter trusted the declaration and returned four rows containing only nulls.
The corrected converter obtains the raw cell fallibly, recognizes actual SQL
NULL, and selects the existing typed decoder from that cell's runtime type.
Missing cells and numbers that JSON cannot represent return explicit
`database` `ExecutionFailed` errors. Binary values retain the existing base64
object representation; no default value masks a decode failure.

`RagChunkDocumentTool` already chunks and previews with Unicode scalar iterators.
Its metadata alone used byte lengths: `中文😀甲` reported 13 rather than 4, and
70 Chinese scalars reported truncation despite the complete preview. Metadata
now counts the same scalars as the original chunking and 200-scalar preview.
The original `RagStore`, embedding and search paths are unchanged.

## Reproducible scope

| Proof | Before | After |
| --- | --- | --- |
| SQLite public `DescribeTableTool`, exact six-column PRAGMA / four rows, database bytes unchanged | fail | pass |
| SQLite same-column NULL / i64 maximum / i64 minimum / finite float / Unicode / BLOB runtime values | fail | pass |
| Malformed driver row with original metadata but missing value must be a typed error | fail | pass |
| SQLite infinite runtime number must fail rather than become JSON null | fail | pass |
| Original bounded SQL pagination and read-only query validation | 2 pass | 2 pass |
| Public RAG Unicode / ASCII scalar counts | fail | pass |
| Public RAG 70 / 200 / 201 Chinese and 201 ASCII preview boundaries | fail | pass |

The final old-production database run has 2 passes, 4 behavior failures and zero
ignored tests. Its corresponding new-production run has 6 passes and zero
failures or ignored tests. The old RAG run has 2 behavior failures; the same new
RAG tests both pass. `receipt.json` lists exact commands, exits, sources and
limitations; archived before/after source-base64 text files preserve each observed fixture.
Their exact decoded SHA256 hashes are in `receipt.json`; decode them with any
standard base64 decoder to recover the original Rust source bytes.

Two unsuccessful fixture attempts remain labeled separately. The initial fixture
did not compile because the public `sqlx` crate does not re-export
`AnyValueKind`; the corrected fixture required no dependency change. The next
attempt had three real behavior failures and one invalid in-memory URL fixture.
The corrected temporary-file fixture produced the final four behavior failures.
Neither unsuccessful attempt substitutes for the final red evidence.

An initial semantic task check classified the complete `.rs` source archives as
new production APIs and a new RAG store. The archives are now reversibly encoded
text evidence with decoded hashes, rather than duplicate compilable source files.
The original failing classification log remains preserved. Actual production
differences add neither public declarations nor state authorities.

Formatting and all-target/all-feature `echo_tools` warning lint pass, as do the
four required production panic lints. This is focused candidate evidence, not
full workspace, PostgreSQL/MySQL server, SDK Host, three-language or main delivery
acceptance. The shared target has an exclusive producer lease; no parallel Cargo
command was started by this lane.

## Semantic impact and delivery obligations

The affected behavior closure is `sql_query`, `list_tables`, `describe_table`
result decoding and `rag_chunk_document` metadata. Framework SQLx runtime values
and the existing character-based chunker remain the sole authorities. SDK
facade consumers must rerun accepted operations against the published successor;
passing tests against an older Host remain historical evidence.

This is a local repair within the existing architecture, so no new ADR is
required. Formal English and Chinese database and Tool documentation now state
the exact SQLite output, parameter names and Unicode scalar convention. There
is no dedicated database or chunk-preview executable example; the persistent
public Tool regressions are included in the existing all-target test chain.
Website copies of `02-tools.md` and `42-database-tools.md` require the repository's
existing document-sync producer after the framework successor is published;
this lane does not hand-edit generated website files.

The complete workspace gates must run after integrating the actual latest main,
followed by independent review, applicable CI and exact published-main checks.
No public finding is marked resolved merely because these focused tests pass.
