# ADR 0027: Subagent Communication Primitives (Uplink + Shared Control Plane)

- Status: Accepted
- Date: 2026-09-02
- Owners: `echo-core/tools`, `echo-agent/agent/subagent`

## Context

The Subagent control plane was strictly downward: hosts and parents could
steer/interrupt a running attempt (`SubagentControlRegistry`, tracked steer),
but a running Subagent had no way to reach its parent or siblings, and could
not even learn its own identity inside a dispatched attempt. The context chain
(`ExternalRunContext` → `AgentInvocationContext` → `AgentRunSnapshot` →
`ToolContext`) carried `run_id` / `execution_id` / `cancel` / `trace_sink`,
but nothing about lineage (own role name, parent, dispatch-tree position) —
and each `SubagentExecutor` kept a **private** `SubagentControlRegistry`,
so an `execution_id` was only addressable within the executor that created
it. Cross-executor sibling addressing (e.g. one registry shared by a primary
agent and delegation-capable Subagents) was structurally impossible.

Reference implementations (per EKO ADR 0001 §4.9/§4.12, which reverse-engineers
Codex's collaboration runtime): mature systems give running subagents a
queue-only uplink to the parent (report / escalate / completion envelope) and
tree-scoped sibling messaging, while keeping the control plane host-held —
subagents are addressed objects, not mailbox owners. Claude Code, by contrast,
keeps strict hub-and-spoke. We adopt the Codex-shaped uplink as framework
mechanism while leaving routing policy (journaling, pausing, surfacing) to
applications.

## Options Considered

1. **Application-only layer**: build the whole uplink in the product (EKO)
   with no framework support. Rejected — the identity gap (`ToolContext`
   lacks lineage) and the per-executor control split are framework problems;
   every embedder would re-implement them.
2. **Peer mailbox per Subagent**: give each Subagent its own inbox and let
   siblings address each other directly. Rejected — duplicates the existing
   `TurnSteerMailbox` + control-registry lifecycle, creates a second
   message-authority, and invites unobservable P2P graphs.
3. **Uplink sink + shared control plane (chosen)**: keep the existing steer
   mailbox as the single delivery mechanism; add (a) a `SubagentLineage`
   value carried through the context chain, (b) an injectable
   `SubagentUplinkFn` sink with a framework default (event bus + shared
   control-registry delivery), and (c) move `SubagentControlRegistry` onto
   `SubagentRegistry` so one registry owns one addressable control plane.

## Decision

1. **`SubagentLineage`** (`echo-core/tools`): self-contained identity snapshot
   (`agent_name`, own `execution_id`/`run_id`, `parent_agent`,
   `parent_execution_id`, canonical `agent_path`, `task_id`, `attempt`,
   `plan_revision`), carried as `ExternalRunContext.subagent_lineage` and
   copied into `ToolContext`. `SubagentExecutor::enrich_dispatch_context`
   stamps missing basics at dispatch; caller-stamped values (the
   `agent_tool` path chains the parent path and execution id; application
   task runtimes stamp task/attempt/revision) always win.
2. **Uplink channel**: `ExternalRunContext.uplink: Option<SubagentUplinkFn>`.
   Messages are `SubagentUplinkMessage { from, target }` with
   `target = Parent { kind: Report|Escalate, text } | Sibling {
   to_execution_id, text }`. Delivery is **queue-only and fire-and-forget**:
   the sender never waits for a reply, so parent/child mutual waiting cannot
   deadlock a dispatch tree. The framework default sink
   (`default_uplink_sink`) steers the parent (when its execution id is known
   and live) or the sibling through the shared control registry, and always
   emits `SubagentEvent::UplinkReceived` for observability. An
   application-provided sink is never replaced.
3. **Shared control plane**: `SubagentRegistry` owns exactly one
   `SubagentControlRegistry` (`registry.control_registry()`); every
   `SubagentExecutor` on that registry shares it. `execution_id` becomes
   addressable across executors. New bounded read:
   `active_snapshot(limit) -> Vec<ActiveAttemptSummary>`.
4. **Tools**: `subagent_message` (parent report/escalate + sibling
   queue-only, bounded to 8 000 chars) and `subagent_list` (bounded active
   view for sibling discovery). Registered opt-in via
   `ReactAgentBuilder::register_subagent_message_tools()` / the
   `register_subagent_message_tools` `AgentConfig` flag — deliberately
   decoupled from `register_agent_dispatch_tool`, because uplink messaging is
   useful for every dispatched Subagent, including roles without delegation
   rights.

## Consequences

- Applications own routing policy: EKO installs its own uplink sink at
  TaskRun dispatch to journal escalations and map blocking ones to a
  `NeedsInput` pause; the framework default is observability + steer only.
- Escalation never blocks the sender: `Escalate` means "host should decide",
  not "child waits". Answers return through the existing guidance/steer
  path into the same attempt.
- No second mailbox: all delivery reuses `Agent::steer_input_tracked` and
  the control-registry attempt lifecycle (admit → attach → settle).
- Sibling messages are claims, not verified evidence — enforcing that
  interpretation stays a prompt-policy concern (product layer).

## Verification

- Unit: `enrich_dispatch_context_fills_missing_lineage_and_keeps_caller_values`,
  `default_uplink_sink_delivers_to_active_attempt_and_reports_missing`
  (executor tests), tool schema tests.
- Example: `echo-agent-learning/examples/demo50_subagent_communication.rs`
  (no LLM required; end-to-end dispatch → sibling message → escalate →
  event observability → settlement).
