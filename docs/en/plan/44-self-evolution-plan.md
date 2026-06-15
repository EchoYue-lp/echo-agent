# 44 — Self-Evolution & Self-Improvement Implementation Plan

## Context

The echo-agent system has a solid foundation for memory (Store trait, MemoryPromoter, UnifiedMemory, AutoMemory) and skills (SkillRegistry, SkillTelemetry, Curator, file-based SKILL.md), but lacks the **closed-loop self-evolution** mechanism: memories don't have structured types or confidence scoring, there's no layered hot/warm/cold system, no automatic write triggers, no memory review/GC, no skill creation from observed patterns, no skill merge/health/patching, and no rule promotion.

This plan adds self-evolution incrementally: each phase is independently valuable, testable, and respects the framework/product boundary.

## Key Design Decisions

1. **MemoryMeta storage**: Embedded in `StoreItem.value` JSON — backward compatible, no Store trait or schema changes needed
2. **MEMORY.md / AGENTS.md**: Human-readable files, both Agent and human can edit; Agent must respect human edits (detect and preserve manual changes)
3. **Scope**: All 5 phases, implemented sequentially
4. **Evolution module**: New `echo-agent/src/evolution/` module, parallel to existing `echo-agent/src/improve/`

## Compatibility with Existing improve Module (Important)

The 8 existing components in `improve/` must be handled individually. See [Compatibility Analysis](./44-self-evolution-compatibility.md) for details.

| Component | Action | Reason |
|-----------|--------|--------|
| `TrajectorySaver` | Keep as-is | Fully independent, no conflict |
| `BackgroundReviewer` | Extend | Write path changes to TypedMemoryStore |
| `Curator` | Extend | SkillLifecycle gains Candidate/Draft/Deprecated |
| `Analyzer` | Keep + integrate | Failure detection serves new system |
| `ImprovementLoop` | Keep as-is | Different function (eval-driven), complementary |
| `SelfEvolution` | Rename to `EvalDrivenImprovement` | Name collision, functionally distinct |
| `PromptGenerator` | Keep + reuse | Reused by SkillDraftGenerator |
| `CritiqueStore` / `DualLayerCritiqueStore` | ❌ Delete | Replaced by ChangeLog + TypedMemoryStore |

---

## Phase 0: Typed Memory Foundation + Change Audit

**Why first**: Everything depends on memory entries having structured metadata. Without types, confidence, status, and audit trails, no layering, review, or promotion can work.

### Framework (echo-agent)

**1. MemoryType + MemoryMeta** — `echo-core/src/memory/types.rs` (new)
```
MemoryType: UserPreference | ProjectFact | ArchitectureDecision | DebuggingLesson | ErrorResolution | CommandPattern | ToolUsage | WorkflowPattern | SkillCandidate | DeprecatedNote
MemoryMeta { memory_type, confidence: f32, stability: f32, risk: MemoryRisk(Low/Medium/High), status: MemoryStatus(Draft/Active/Superseded/Archived), source: MemorySource(UserCorrection|ErrorResolution|RepeatedWorkflow|ExplicitSave|AutoExtracted), topic: String, revision_count: u32, superseded_by: Option<String> }
```
- Serialization: `MemoryMeta` embeds into `StoreItem.value` JSON alongside `content` field. No `StoreItem` schema change needed.

**2. TypedMemoryStore** — `echo-state/src/memory/typed_store.rs` (new)
- Wraps `Arc<dyn Store>`, provides typed read/write: `put_typed()`, `get_typed()`, `search_typed(filter: MemoryFilter)`
- `MemoryFilter`: by type, status, min_confidence, topic, source
- Backward-compatible: untyped entries get `MemoryMeta::default()` on read

**3. ChangeLog + ChangeAuditor** — `echo-agent/src/evolution/audit.rs` (new)
- `ChangeEntry { timestamp, entity_type(Memory/Skill/Rule), entity_key, change_type(Create/Update/Delete/Promote/Demote/Merge), before, after, reason, trigger }`
- `ChangeLog` trait: `record()`, `query()`, `rollback()`
- `JsonlChangeLog`: writes to `.echo-agent/evolution/change-log.jsonl`
- `ChangeAuditor`: wraps any Store to intercept writes and record changes

**4. Secret scanning** — `echo-agent/src/evolution/security.rs` (new)
- Regex patterns for API keys (AWS `AKIA...`, GitHub `ghp_...`, generic base64 tokens, `BEGIN PRIVATE KEY`, `Bearer ...`)
- Scan before persist; redact matches to `[REDACTED: <type>]`, log warning
- `UntrustedInputMarker`: sets `MemoryMeta.risk = High` for content from tool output

### Integration points
- `StoreMemoryPromoter` → use `TypedMemoryStore::put_typed()` instead of raw `store.put()`
- `AutoMemory::extract_observations()` → write via `TypedMemoryStore` with `MemorySource::AutoExtracted`
- `BackgroundReviewer` → write via `TypedMemoryStore` with `MemorySource::UserCorrection`

### New module structure
```
echo-agent/src/evolution/
  mod.rs          (new, re-exports)
  audit.rs        (new)
  security.rs     (new)
```

### Phase 0 Compatibility Operations (done in parallel with above)

1. **Delete `improve/store.rs`** (CritiqueStore / DualLayerCritiqueStore)
   - Replaced by `ChangeLog` + `TypedMemoryStore` namespaces
   - Update `improve/mod.rs` to remove re-exports

2. **Rename `improve/evolution.rs` → `improve/eval_improvement.rs`**
   - Type `SelfEvolution` → `EvalDrivenImprovement`
   - Update `improve/mod.rs` re-exports

3. **Extend `improve/curator.rs`**
   - Add `Candidate`, `Draft`, `Deprecated` to `SkillLifecycle`
   - Add new transition paths and methods
   - Keep existing tests

4. **Extend `improve/background_review.rs`**
   - `store.put()` → `TypedMemoryStore::put_typed()` in `run_review()`
   - Attach `MemoryMeta` metadata on write
   - Keep review prompts and `ReviewOutcome`

### Verification
- `cargo test -p echo-core` — MemoryMeta serde round-trip
- `cargo test -p echo-state` — TypedMemoryStore read/write/search with filters
- `cargo test -p echo-agent` — ChangeLog record/query/rollback, secret scanning catches `AKIA...`

---

## Phase 1: Memory Layering + Write Triggers

**Why**: With typed memory, we can layer it (hot/warm/cold) and automatically trigger writes from conversation signals instead of just keyword matching. This is the biggest UX improvement.

### Framework (echo-agent)

**1. MemoryLayerManager** — `echo-agent/src/evolution/layer.rs` (new)
- Three layers:
  - **Hot** (MEMORY.md): top-N active memories, always loaded in context. Path: `.echo-agent/MEMORY.md`. Max 2000 tokens.
  - **Warm** (Store KV, `["agent", "typed_memories"]`): topic-organized, loaded on-demand
  - **Cold** (archive, `.echo-agent/memory/archive/`): older/low-confidence, rarely loaded
- `promote(key)`: Cold→Warm→Hot (if space)
- `demote(key)`: Hot→Warm→Cold (when Hot exceeds limit)
- Hot = human-readable markdown (YAML frontmatter + body); Warm/Cold = Store KV

**2. WriteTrigger detector** — `echo-agent/src/evolution/triggers.rs` (new)
- Triggers: `UserCorrection`, `ErrorResolution`, `RepeatedWorkflow`, `ExplicitSave`, `AutoExtracted`
- Detection patterns:
  - UserCorrection: user message contains correction signals after assistant output on same topic
  - ErrorResolution: tool failure followed by successful retry with different approach
  - RepeatedWorkflow: same tool sequence observed ≥3 times (from telemetry)
  - ExplicitSave: `/remember` command or `remember` tool
- Default confidence per trigger: UserCorrection=0.9, ErrorResolution=0.85, RepeatedWorkflow=0.75, ExplicitSave=1.0, AutoExtracted=0.6

**3. Hook event extensions** — `echo-core/src/hooks/types.rs` (modify)
- Add `HookEvent::PostMemoryWrite` — after any memory is persisted
- Add `HookEvent::MemoryLayerChange` — after promote/demote
- Add `HookEventCategory::Evolution` for these new events
- Update `category()`, `as_str()`, `supports_matcher()` match arms

### Product (echo-agent-app-core)

**4. Layered memory rendering** — modify `unified_memory.rs`
- `UnifiedMemory.system_prompt_context()` now includes Hot MEMORY.md content
- New tool `search_layered_memory` that searches Hot + Warm layers
- `InstructionProvider` injects Hot content alongside existing .md files

### Verification
- Unit: promote/demote cycles, Hot size limit enforcement, trigger detection on sample conversations
- Integration: conversation with user correction → captured as `UserCorrection` → written to Hot → appears in next system prompt

---

## Phase 2: Memory Review + Garbage Collection

**Why**: Memory accumulates. Without review, confidence decays, conflicts emerge, and stale entries clutter context.

### Framework (echo-agent)

**1. StalenessScorer** — `echo-agent/src/evolution/review.rs` (new)
```
staleness = age_factor * 0.35 + low_usage_factor * 0.20 + instability_factor * 0.20 + contradiction_factor * 0.20 + source_weakness * 0.05
```
- Status: <0.40→Active, 0.40-0.65→Stale, 0.65-0.85→Deprecated, ≥0.85→Archived

**2. ConflictDetector** — same file
- Finds memories with same `topic` + `memory_type` but contradictory content (different content hash)
- Validates against current repo state when possible (e.g., check `package-lock.json` vs `yarn.lock`)

**3. MemoryMerger** — same file
- Merges similar memories: keep higher-confidence as primary, merge evidence, combine use_count, update summary, mark old as Superseded

**4. MemoryReviewer** — orchestrator in same file
- Scan → Score → Detect conflicts → Merge/Supersede → Record changes via ChangeAuditor

**5. Review scheduling** — `echo-agent-app-core/src/evolution/review_integration.rs` (new)
- Auto-trigger: every 50 memory writes or session end
- Manual: `/memory-review` command
- Post-processes after `BackgroundReviewer` completes

### Verification
- Unit: staleness scoring, conflict detection, merge preserves higher-confidence content
- Integration: insert 10 memories, advance time, run review → stale demoted, conflicts merged, change log records all

---

## Phase 3: Extended Skill Lifecycle + Skill Creation

**Why**: This is the biggest self-evolution value — skills that can be born from observed patterns.

### Framework (echo-agent)

**1. Extend SkillLifecycle** — modify `echo-agent/src/improve/curator.rs`
- Add `Candidate`, `Draft`, `Deprecated` states
- Full lifecycle: `Candidate → Draft → Active → Stale → Deprecated → Archived`
- Transition rules with configurable thresholds
- `CuratorStatus` gains new counters
- Existing `apply_transitions()` gains new match arms

**2. SkillCandidateDetector** — `echo-agent/src/evolution/candidate.rs` (new)
- Scans `TypedMemoryStore` for `MemoryType::WorkflowPattern` / `MemoryType::DebuggingLesson`
- When ≥3 entries share same topic with `source == RepeatedWorkflow` → propose candidate
- Writes candidate to `["agent", "skill_candidates"]` namespace
- Candidate includes: name, description, trigger_patterns, tool_sequence, sample_count, confidence

**3. SkillDraftGenerator** — `echo-agent/src/evolution/draft.rs` (new)
- Takes candidate → generates draft SKILL.md (template-based for non-LLM path, or via `PromptGenerator` for LLM path)
- Draft SKILL.md: YAML frontmatter with `lifecycle: draft`, name, description, triggers; markdown body with synthesized instructions
- Saves to `.echo-agent/skills/_drafts/<name>/SKILL.md`

### Product (echo-agent-app-core)

**4. Skill lifecycle commands** — extend CLI
- `/skill-candidates` — list all candidates and drafts
- `/skill-promote <name>` — move Draft → Active
- `/skill-create` — interactive creation from candidate

### Integration
- `SkillTelemetryStore` → `SkillCandidateDetector` reads telemetry for repeated tool patterns
- `Curator` → extended with new lifecycle states
- `SkillRegistry` → `register_descriptor()` already supports any SKILL.md; drafts are in subdirectory scannable with `lifecycle` filter
- `TriggerDetector` → feeds `RepeatedWorkflow` observations into candidates

### Verification
- Unit: lifecycle state machine transitions, candidate detection from mock observations, draft generation
- Integration: 3 conversations with same debugging pattern → SkillCandidate created → promote to Draft → SKILL.md exists → promote to Active → appears in catalog

---

## Phase 4: Skill Merge + Health + Patching

**Why**: As skills multiply, they overlap and degrade. Merge prevents duplication; health+patching keeps skills effective.

### Framework (echo-agent)

**1. SkillSimilarityDetector** — `echo-agent/src/evolution/merge.rs` (new)
```
similarity = description_overlap * 0.25 + trigger_overlap * 0.30 + scope_overlap * 0.15 + tool_overlap * 0.10 + pitfall_overlap * 0.10 + co_activation_rate * 0.10
```
- ≥0.75 → merge proposal; ≥0.90 → strong recommendation

**2. SkillMergeProposal + merger** — same file
- Keep higher-activation-count skill as primary, absorb secondary's triggers and unique instructions
- Human review required: merge proposals stored but NOT auto-applied
- `/skill-merge <a> <b>` command to apply

**3. SkillHealthMonitor** — `echo-agent/src/evolution/health.rs` (new)
```
health = success_rate * 0.30 + recent_success_rate * 0.20 + usage_frequency * 0.10 + freshness * 0.15 + user_approval * 0.15 + command_validity * 0.10
```
- ≥0.75→Healthy, 0.55-0.75→NeedsAttention, <0.55→Unhealthy
- Reads `SkillTelemetry` after each skill deactivation

**4. SkillPatcher** — `echo-agent/src/evolution/patch.rs` (new)
- Analyzes `common_failures` from telemetry → generates `SkillPatch`
- Patch types: `AddPrecondition`, `AddTool`, `RefineInstruction`, `AddErrorHandling`
- Patch is a suggestion, NOT auto-applied
- `/skill-patch <name>` to review and apply

### Verification
- Unit: similarity scoring, merge proposal, health score calculation, patch suggestion from failures
- Integration: register 2 similar skills → run merge → merged skill works; consistently failing skill → health check → Unhealthy + patch suggestion

---

## Phase 5: Rule Promotion + Security Hardening

**Why**: The highest form of self-evolution — learned knowledge becomes permanent rules. Security hardening ensures production safety.

### Product (echo-agent-app-core)

**1. RulePromoter** — `echo-agent-app-core/src/evolution/rules.rs` (new)
- Scans high-confidence memories (confidence≥0.95, stability≥0.9, importance≥8.0, revision_count==0)
- Generates `RuleProposal { source_memory_key, rule_text, target_tier, reason }`
- Human review required via `/rule-promote` command
- Applied rules written to instruction .md file via `InstructionProvider`
- Source memory marked `status: Superseded`

**2. AGENTS.md integration** — modify `instruction_provider.rs`
- Add `.echo-agent/AGENTS.md` as fourth tier (between Project and Local)
- Sections: `## Auto-promoted rules`, `## Learned constraints`, `## Deprecated rules`

**3. EvolutionSecurityGuard** — extend `echo-agent/src/evolution/security.rs`
- Pre-write: secret scanning (Phase 0) + prompt injection detection (patterns like "ignore previous")
- Untrusted input isolation: tool-output-originated memories → `risk: High`, never promoted to Hot or rules without human approval
- Rate limiting: max 50 memory writes/session, max 5 skill patches/day
- Rollback via `ChangeAuditor`

**4. Dashboard** — `/evolution status` command
- Memory counts by layer, type, status
- Skill lifecycle distribution
- Pending merge proposals, patches, rule candidates
- Recent changes from audit log

### Verification
- Unit: rule proposal from high-confidence memories, AGENTS.md write+reload, rate limiting
- Integration: high-confidence memory → rule promotion → appears in AGENTS.md → source memory Superseded → rollback → memory restored to Active

---

## File Layout

```
.echo-agent/
  MEMORY.md                        # Hot layer (Phase 1)
  AGENTS.md                        # Auto-promoted rules (Phase 5)
  project.md                       # Existing
  local.md                         # Existing
  memory/
    topics/*.md                    # Warm layer (Phase 1)
    archive/                       # Cold layer (Phase 1)
  evolution/
    change-log.jsonl               # Audit log (Phase 0)
    skill_candidates/              # Candidate SKILL.md drafts (Phase 3)
    patches/                       # Skill patches (Phase 4)
    critiques/                     # Existing
  skills/
    _drafts/<name>/SKILL.md        # Draft skills (Phase 3)
  curator_state.json               # Existing
```

## Store Namespaces

| Namespace | Purpose |
|-----------|---------|
| `["agent", "typed_memories"]` | New typed memory entries |
| `["agent", "skill_candidates"]` | Skill candidate proposals |
| `["agent", "skill_telemetry"]` | Existing telemetry |
| `["agent", "profile"]` | Existing agent profile |
| `["agent", "evolution", "patches"]` | Skill patches |
| `["agent", "evolution", "merges"]` | Merge proposals |
| `["agent", "evolution", "rules"]` | Rule proposals |
| `["l3_promoted"]` | Existing L3 promoted facts |
| `["background_reviews"]` | Existing review memories |

## New Hook Events (Phase 1+)

| Event | Category | Phase |
|-------|----------|-------|
| `PostMemoryWrite` | Evolution | 1 |
| `MemoryLayerChange` | Evolution | 1 |
| `SkillCandidateDetected` | Evolution | 3 |
| `SkillLifecycleTransition` | Evolution | 3 |
| `SkillHealthCheck` | Evolution | 4 |
| `SkillPatchApplied` | Evolution | 4 |
| `RulePromoted` | Evolution | 5 |

## Critical Files to Modify

| File | Changes |
|------|---------|
| `echo-core/src/memory/types.rs` | New: MemoryType, MemoryMeta, MemoryRisk, MemoryStatus, MemorySource |
| `echo-core/src/memory/mod.rs` | Add `pub mod types` |
| `echo-core/src/hooks/types.rs` | Add new HookEvent variants + HookEventCategory::Evolution |
| `echo-state/src/memory/typed_store.rs` | New: TypedMemoryStore, MemoryFilter |
| `echo-state/src/memory/mod.rs` | Add `pub mod typed_store` |
| `echo-state/src/lib.rs` | Export TypedMemoryStore |
| `echo-agent/src/evolution/mod.rs` | New: re-exports for all evolution sub-modules |
| `echo-agent/src/evolution/audit.rs` | New: ChangeLog, JsonlChangeLog, ChangeAuditor |
| `echo-agent/src/evolution/security.rs` | New: SecretScanner, EvolutionSecurityGuard |
| `echo-agent/src/evolution/layer.rs` | New: MemoryLayerManager |
| `echo-agent/src/evolution/triggers.rs` | New: TriggerDetector, WriteTrigger |
| `echo-agent/src/evolution/review.rs` | New: StalenessScorer, ConflictDetector, MemoryMerger, MemoryReviewer |
| `echo-agent/src/evolution/candidate.rs` | New: SkillCandidateDetector |
| `echo-agent/src/evolution/draft.rs` | New: SkillDraftGenerator |
| `echo-agent/src/evolution/merge.rs` | New: SkillSimilarityDetector, SkillMergeProposal |
| `echo-agent/src/evolution/health.rs` | New: SkillHealthMonitor, HealthStatus |
| `echo-agent/src/evolution/patch.rs` | New: SkillPatcher, SkillPatch |
| `echo-agent/src/improve/curator.rs` | Extend: SkillLifecycle with Candidate/Draft/Deprecated |
| `echo-agent/src/improve/background_review.rs` | Modify: write via TypedMemoryStore with MemoryMeta |
| `echo-agent/src/improve/eval_improvement.rs` | Rename from evolution.rs, type SelfEvolution→EvalDrivenImprovement |
| `echo-agent/src/improve/store.rs` | Delete: replaced by ChangeLog + TypedMemoryStore |
| `echo-agent/src/memory_promoter.rs` | Modify: use TypedMemoryStore |
| `echo-agent/src/lib.rs` | Add `pub mod evolution` |
| `echo-agent-cli/echo-agent-app-core/src/evolution/` | New: product-level integration modules |
| `echo-agent-cli/echo-agent-app-core/src/unified_memory.rs` | Modify: add layer integration |
| `echo-agent-cli/echo-agent-app-core/src/instruction_provider.rs` | Modify: add AGENTS.md tier |

## Effort Estimate

| Phase | Weeks | Cumulative Value |
|-------|-------|-----------------|
| 0: Typed Memory + Audit | 2-3 | Structured, auditable memory with security |
| 1: Layering + Triggers | 2 | Layered memory, automatic write triggers (biggest UX gain) |
| 2: Review + GC | 2 | Self-maintaining memory |
| 3: Skill Lifecycle + Creation | 3 | Agent creates its own skills (core self-evolution) |
| 4: Merge + Health + Patch | 2 | Skills stay healthy and don't duplicate |
| 5: Rule Promotion + Security | 2 | Learning → permanent rules, full security |
| **Total** | **~13-15** | Complete self-evolution closed loop |

## Implementation Order Within Each Phase

For each phase, the implementation order is:
1. **echo-core types** (MemoryType, MemoryMeta, HookEvent variants) — foundation types
2. **echo-state implementations** (TypedMemoryStore, etc.) — concrete stores
3. **echo-agent/evolution modules** — business logic (scoring, detection, review)
4. **echo-agent-app-core integration** — product-level wiring, commands, prompts
5. **Tests** — unit → integration → full CI matrix

## Verification Strategy (Per Phase)

Each phase follows: `cargo check` → `cargo test --workspace` → `cargo fmt` → `cargo clippy`

- Phase 0: `cargo test -p echo-core -p echo-state -p echo_agent` — typed memory + audit + security tests
- Phase 1: integration test — trigger detection + layer promote/demote + system prompt injection
- Phase 2: integration test — insert memories, advance time, review, verify demotion and merge
- Phase 3: integration test — 3 similar conversations → candidate → draft → active skill
- Phase 4: integration test — similar skills merge + unhealthy skill patch
- Phase 5: integration test — rule promotion + rollback + security guard

Full CI matrix must pass before each phase is considered complete.

## Parallelism Opportunities

Within each phase, several modules can be implemented in parallel:
- Phase 0: `types.rs` + `audit.rs` + `security.rs` are independent
- Phase 1: `layer.rs` + `triggers.rs` are independent (both depend on Phase 0 types only)
- Phase 2: `StalenessScorer`, `ConflictDetector`, `MemoryMerger` can be developed in parallel then composed
- Phase 3: `candidate.rs` + `draft.rs` are sequential (draft depends on candidate)
- Phase 4: `merge.rs` + `health.rs` + `patch.rs` are independent
- Phase 5: `rules.rs` + security extension + dashboard are independent
