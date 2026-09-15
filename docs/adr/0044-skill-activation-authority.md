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
2. `SkillActivationHandle` is the single process-local runtime authority. One
   mutex atomically stores the runtime epoch, per-Skill generation, active
   records (content plus sandbox policy), and in-flight activation claims.
   `SkillRegistry::activation_view` constructs an empty definition view backed
   by that handle for progressive tools.
3. Catalog descriptors, prepared documents, code-skill definitions, source
   indexes, and filesystem lookup remain registry-local definition data. The
   ReactAgent registration/reconciliation paths keep both registry views in
   sync where resource lookup requires a descriptor. The authority-splitting
   mutable Agent accessor is removed; SDK register/tag/remove/unregister
   operations call the ReactAgent reconciliation APIs.
4. API activation, `ActivateSkillTool`, resource/script checks, visibility
   snapshots, and checkpoint save/restore all read or mutate the shared
   activation state. Run snapshots retain a clone of the canonical handle;
   telemetry is only an observer and never checkpoint authority. Restore
   atomically rebuilds names and policies from currently installed descriptors.
5. Activation identity is `(skill name, arguments, source)`. Concurrent or
   repeated activation of the same identity shares one in-flight result and
   executes inline commands once. A later, different argument/source identity
   retires the prior completed generation; two different identities cannot run
   concurrently. Reset, descriptor replacement, and remove advance an epoch or
   generation so stale async completion cannot republish. If the owning
   activation future is cancelled or panics after an external command may have
   started, the claim remains poisoned and cannot replay until an explicit
   reset or removal establishes a new generation.
6. No new serialized field is introduced. Checkpoints keep their existing
   `active_skills` field. `SkillActivationHandle` and its construction methods
   are explicit Rust/Host-only inventory entries; they are not hidden from the
   public-contract drift gate and are not mapped as language SDK lifecycle APIs.

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
4. Cache only a telemetry list or synchronize independent activation sets at
   checkpoint time. Rejected because reset/remove and SDK mutations can occur
   after a run snapshot, recreating stale resurrection and policy loss.

## Consequences

- Direct activation and progressive resource/script tools observe the same
  active names and sandbox policies.
- Definition copies remain an implementation adapter, not an independent
  lifecycle authority.
- Checkpoint save reads the live handle; restore never trusts telemetry or a
  separately persisted policy.
- Inline command execution is exactly once per activation identity while a
  completed identity remains active. Parameter changes intentionally create a
  new generation.
- Registry mutation through a Session remains source-compatible at the SDK
  operation identity while the Host routes it through ReactAgent reconciliation.
- Future registry adapters must derive from the primary registry's
  `activation_view`; creating a fresh runtime activation authority is a
  semantic regression.
