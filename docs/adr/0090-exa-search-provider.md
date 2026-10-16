# ADR 0090: Exa search behind the existing SearchProvider trait

## Status

Accepted for implementation on 2026-10-16. Publication and merged-main verification
remain separate delivery steps.

## Context and alternatives

Internet discovery (`web_search`) and fetching a known URL (`web_fetch`) are
separate tools. The framework already owns a product-independent `SearchProvider`
trait with DuckDuckGo, Brave and Tavily implementations. The standard tool pack
previously hard-coded DuckDuckGo even though `WebSearchTool::auto()` could select
other engines. Adding only an Exa constructor would leave EKO's default path on
DuckDuckGo.

Alternatives were a separate Exa tool, merging search with page fetching, or an
additional application-specific search implementation. All duplicate existing
responsibilities or change the established tool contract.

## Decision and references

Implement `ExaSearchProvider` in `echo_tools`, using the existing trait and
`SearchResult { title, url, snippet }`. Exa's official
[Search API](https://docs.exa.ai/reference/search) uses `POST /search` with a query,
result count and optional content settings. Request bounded highlights and map
them into snippets; do not request page bodies or call `/contents`.

The existing provider model and the separate Exa integration documented by
[LangChain](https://docs.langchain.com/oss/python/integrations/tools/exa_search)
support keeping vendor transport behind the shared search contract. No new
dependency, permission gate, framework trait or application-owned search loop is
needed.

Expose `WebSearchTool::with_exa(key)`. Extend `auto()` to select
Exa (`EXA_API_KEY`, nonempty) > Tavily > Brave > DuckDuckGo and use `auto()` in the
standard tool pack, making this path reachable from EKO. Selection occurs once
at tool construction; failed requests do not switch engines. Explicit provider
constructors remain available to consumers that require a fixed engine.

## Consequences and verification

With no search keys the default remains DuckDuckGo. Existing Tavily/Brave keys
now also affect the standard pack, preserving their relative priority. Restart
or rebuild the Agent after changing environment configuration. Exa's credentials,
quota and connectivity are operational prerequisites; HTTP 401/402/429 failures
are reported without echoing upstream response bodies. The tool's shared failure
classification and retry policy are unchanged.

`web_search` retains its schema, ten-result provider window and existing cursor
pagination. `web_fetch` is unchanged. Mock HTTP tests cover Exa request/response,
Unicode snippets, invalid/empty results, limits and failure responses. Separate
child-process tests cover environment precedence and actual standard-pack
selection without unsafe environment mutation or live API calls. The existing
web example stays in the learning workspace's example compilation gate.

SDK accepted contracts and the SDK Host's explicit DuckDuckGo constructor are
unchanged; its exact framework revision remains independently pinned. Bilingual
framework docs are updated at source. Website publication must use its canonical
sync script after the framework source is committed and clean; uncommitted
content must not be attributed to a clean source revision.
