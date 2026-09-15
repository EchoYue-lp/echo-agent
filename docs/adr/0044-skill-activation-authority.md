# ADR 0044: One Runtime Authority for Skill Activation

## Status

Accepted

## Context

`ReactAgent` owns a primary `SkillRegistry` for code skills and catalog
operations, while progressive-disclosure tools were given a second
`SkillRegistry` containing copied file descriptors. The two registries each
owned an activation set and sandbox-policy map. Direct API activation and
checkpoint restore updated the primary registry, while
`activate_skill`, `read_skill_resource`, and `run_skill_script` used the
progressive registry. A restored or directly activated Skill could therefore
appear in context while resource and script access still reported that it was
inactive.

The framework needs to retain the distinction between catalog/definition data
and runtime activation data. File descriptors may be copied into the existing
progressive tool adapter so it can resolve resources without holding an agent
lock; that copy must not become a second activation authority.

## Decision

1. `SkillRegistry` remains the canonical owner of activation state for an
   agent runtime.
2. `SkillRegistry::activation_view` constructs an empty definition view backed
   by the same private activation-state handle. The handle contains the
   activated-name set and activation-derived sandbox policies. It is shared by
   the primary registry and the progressive tool adapter; it is not a second
   state machine or a public domain concept.
3. Catalog descriptors, prepared documents, code-skill definitions, source
   indexes, and filesystem lookup remain registry-local definition data. The
   existing registration/reconciliation paths keep both registry views in
   sync where resource lookup requires a descriptor.
4. API activation, `ActivateSkillTool`, resource/script checks, visibility
   snapshots, and checkpoint save/restore all read or mutate the shared
   activation state. Repeated activation remains idempotent, and reset/remove
   clears the shared state.
5. No new public protocol or serialized field is introduced. Checkpoints keep
   their existing `active_skills` field; restoring it populates the shared
   runtime state once.

## Alternatives Considered

1. Keep two independent activation sets and synchronize every call site.
   Rejected because missed paths would recreate the current authority split.
2. Replace the public `SkillRegistry` API with an `Arc<RwLock<_>>` handle.
   Rejected because it would change existing framework consumers and make
   code-skill registration unnecessarily asynchronous.
3. Remove the progressive registry entirely and make tools borrow the
   primary registry. Rejected because tools execute asynchronously and need a
   concurrent handle while the public agent API retains synchronous access to
   code-skill definitions.

## Consequences

- Direct activation and progressive resource/script tools observe the same
  active names and sandbox policies.
- Definition copies remain an implementation adapter, not an independent
  lifecycle authority.
- Checkpoint round trips and repeated activation can be tested at the shared
  state boundary without changing the wire contract.
- Future registry adapters must derive from the primary registry's
  `activation_view`; creating a fresh runtime activation authority is a
  semantic regression.
