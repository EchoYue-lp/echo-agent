# ADR 0064: Provider Stream Semantic Terminal

## Status

Accepted

## Context

The shared SSE transport validates framing, cancellation, and timeouts, but a
well-framed EOF does not mean that the model completed its response. Responses
already requires `response.completed`; Chat Completions previously accepted EOF
or `[DONE]` without a choice finish reason, and Anthropic exposed the finish
reason and usage from `message_delta` without requiring `message_stop`.
Direct `LlmClient::chat_stream` consumers could therefore treat partial output
and premature usage as a completed response. ReAct's own finish check cannot
protect other consumers or distinguish a stopped Anthropic message from an
early `message_delta`.

This is a framework provider contract, independent of EKO product policy.
The existing `LlmClient`, `ChatChunk`, and `stream_json_sse` remain authoritative;
each provider adapter interprets only its wire protocol's semantic terminal.
The relevant wire references are [OpenAI Chat streaming](https://platform.openai.com/docs/api-reference/chat-streaming),
[OpenAI Responses streaming](https://platform.openai.com/docs/api-reference/responses-streaming),
and [Anthropic Messages streaming](https://docs.anthropic.com/en/docs/build-with-claude/streaming).

## Decision

1. Chat Completions succeeds only after a successful choice finish reason
   (`stop`, `tool_calls`, or `function_call`) followed by `[DONE]`. EOF or
   `[DONE]` without that reason and non-success reasons are typed
   `InvalidResponse` failures. A usage-only event after the finish reason is
   retained; finish and usage are published together only at `[DONE]`.
2. Responses keeps `response.completed` as its semantic terminal. Its existing
   status and payload validation remain in the Responses adapter, including
   rejection of `response.failed`, `response.incomplete`, and premature EOF.
3. Anthropic requires `message_delta` with a successful stop reason followed
   by `message_stop`. The adapter retains finish and usage until `message_stop`;
   EOF, `[DONE]`, or `message_stop` without a valid preceding delta fails.
   Outstanding tool or reasoning blocks cannot complete successfully.
4. Partial text/tool deltas remain live progress, but neither a finish reason
   nor usage may claim success before the provider-specific terminal. The
   adapter does not add another provider-neutral state machine or change the
   `LlmClient` public API. The low-level Responses raw event stream continues
   exposing raw events without this `ChatChunk` projection contract.

## Alternatives Considered

- Treat framed EOF as success for every provider. Rejected because a network
  disconnect can occur between valid SSE events and omit the semantic terminal.
- Require only a finish reason for Chat Completions or Anthropic. Rejected
  because both can emit that reason before their final protocol signal and
  before optional usage settles.
- Add a common terminal enum to `ChatChunk` or validate only inside ReAct.
  Rejected because current consumers already use its finish/usage fields, and
  direct `LlmClient` callers need the same protection without another API or
  parallel authority.

## Consequences

OpenAI-compatible endpoints omitting `[DONE]` now fail closed even if they
emitted a choice finish reason; such endpoints need to provide the expected
Chat Completions stream boundary. Valid partial output remains observable, but
callers must consume the stream through its terminal item to claim success.
Local byte-stream fixtures cover success and missing/invalid terminal signals;
real provider behavior remains part of final integration acceptance.
