# ADR 0024: Unified Subagent Prompt Compilation

## Status

Accepted

## Context

Subagent dispatch previously assembled context in several places: role system
prompts were built by applications, the framework executor appended inherited
system/history text and result instructions, TaskRuntime formatted another task
envelope, and some plugin roles bypassed product policy. The result was duplicate
language/result sections and different behavior across sync, fork, teammate,
team, planned, and plugin paths.

Prompt compilation is a reusable framework mechanism, while concrete language,
task-envelope, capability wording, and follow-up policy belong to the embedding
product.

## Decision

1. `SubagentPromptCompiler` is the only registration/dispatch prompt extension
   point. `compile_system` owns the stable role prompt; `compile_invocation`
   returns the exact structured messages executed by the runtime.
2. `SubagentSystemPromptInput` includes a `ToolCapabilitySnapshot` produced
   after concrete tool registration. The snapshot contains bounded
   descriptions plus visible and disabled sets; product capability text must
   derive from that surface, not a static role table or the parent Agent's
   advertised tools.
3. `SubagentInvocation` combines the current task, an owned
   `SubagentTaskContext`, optional product metadata, context-transfer policy,
   structured history, and the current typed `Message`. The compiler replaces
   only its text framing and preserves attachments. The executor does not
   append prompt text or reconstruct a message after compilation.
4. `Fresh` transfers no transcript. `InheritStructured` filters once and keeps
   only safe user messages and complete assistant final messages. Parent system
   prompts, tool traffic, reasoning, and runtime projections never become user
   text.
5. Sync, fork, teammate, and team use the same compiler contract. Team members
   receive the already-filtered parent history instead of a separate textual
   inheritance format.
6. The framework owns terminal `SubagentOutcome` parsing and generic embedded
   JSON framing. Applications normalize only genuinely product-specific fields.
7. `DefaultSubagentPromptCompiler` remains a product-neutral SDK default. EKO
   injects one `EkoSubagentPromptCompiler` for built-in and plugin roles.
8. Agent-wide disabled tools are exposed through a shareable
   `ToolVisibilityPolicy`. Lazy Subagent factories may share that policy while
   each run still captures an immutable effective snapshot combined with its
   invocation-specific allowlist.
9. Invocation-specific capability narrowing is carried by
   `SubagentInvocation.capability_override`, not by task context. It is absent
   for ordinary calls and therefore cannot duplicate the stable registered
   capability catalog.
10. `SubagentDefinition.access_mode` is the typed mutation authority used by
    prompt publication. Free-form tags remain discovery metadata and must not
    be decoded into read/write behavior.

## Consequences

- Applications can provide rich product policy without forking the executor.
- Prompt cardinality and provenance are observable through compiler diagnostics.
- Adding a dispatch mode does not create another prompt builder.
- A concrete Subagent's system prompt can truthfully describe its actual tools.
- Product catalogues remain projections of registration definitions; they do
  not become a second dispatch registry.
