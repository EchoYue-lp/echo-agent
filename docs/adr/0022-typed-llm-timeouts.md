# ADR 0022: Typed LLM Timeout Contract

## Status

Accepted

## Context

The provider transport implemented first-chunk, idle, and overall SSE timeouts
through three process environment variables. Chat Completions and Responses
used that helper, while Anthropic owned a separate byte loop with cancellation
but no equivalent timeout semantics. Each reqwest client also imposed a hidden
120-second total timeout, so disabling or extending the explicit overall stream
timeout did not describe actual runtime behavior. The public streaming guide
still called the feature planned.

Timeout policy is a reusable SDK concern. It must not depend on EKO config or a
process-global environment convention, and every provider implementing the
same `LlmClient` contract must observe the same boundaries.

The official OpenAI SDK exposes a client timeout default and a per-request
override, including granular connect/read/write values. Reqwest likewise
supports request-scoped timeouts. These mature APIs keep timeout policy beside
the client/request rather than hiding it in application-specific global state:

- [OpenAI Python SDK timeouts](https://github.com/openai/openai-python#timeouts)
- [reqwest RequestBuilder::timeout](https://docs.rs/reqwest/latest/reqwest/struct.RequestBuilder.html#method.timeout)

## Decision

1. `echo_core::llm::LlmTimeouts` is the single provider-neutral value for:
   non-streaming request, request-to-first-chunk, inter-chunk idle, and complete
   stream timeouts.
2. `LlmConfig` owns the client default. `ChatRequest::with_timeouts` supplies an
   optional per-request override using the same type.
3. Public API uses `std::time::Duration`; serde stores optional millisecond
   values. `None` and serialized zero disable a boundary.
4. Defaults are 120 seconds for a complete request, 30 seconds through first
   response bytes, 60 seconds between streaming byte chunks, and no complete
   stream deadline.
5. Chat Completions, Responses, and Anthropic Messages share one SSE transport
   for HTTP startup, cancellation, timeout selection, byte reads, UTF-8-safe
   decoding, event framing, and EOF validation. Provider adapters retain only
   semantic event translation.
6. The overall deadline starts before the HTTP request. First-chunk time also
   starts before the request, not after response headers arrive.
7. The old `ECHO_AGENT_STREAM_*` environment variables and provider-local SSE
   loop are deleted. The reqwest clients no longer apply an undisclosed total
   timeout to streaming bodies.

## Alternatives Considered

1. Keep environment variables and document them. Rejected because they are
   process-global, bypass typed application configuration, and cannot express
   per-client or per-request policy.
2. Add timeout fields only to `ChatRequest`. Rejected because callers would
   have to repeat defaults on every call and raw provider APIs would still need
   another policy owner.
3. Keep Anthropic's private loop and copy the timeout logic. Rejected because
   cancellation, framing, EOF, and timeout behavior would continue to drift.
4. Use one reqwest total timeout for every operation. Rejected because a
   healthy long stream and a stalled stream require different boundaries.

## Consequences

- Framework users get a typed high-level default and a low-level request
  override without application conversion helpers.
- All supported streaming protocols now fail and cancel at the same transport
  boundaries.
- Timeout errors remain ordinary typed LLM network errors, so existing retry
  classification continues to apply.
- Serialized `LlmConfig` gains a `timeouts` object. This project is in active
  development; no compatibility shim or environment fallback remains.
