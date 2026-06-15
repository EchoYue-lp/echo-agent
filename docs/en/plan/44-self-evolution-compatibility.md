# 44 — Self-Evolution System: Existing Component Compatibility Analysis

## Overview

The existing `echo-agent/src/improve/` module contains 8 components, 4 of which are integrated in the CLI and 4 are experimental (not integrated). The new `echo-agent/src/evolution/` module must properly coexist with these: reuse where possible, delete where superseded.

---

## Per-Component Analysis

### 1. TrajectorySaver (`trajectory.rs`) — ✅ Keep As-Is

**Function**: Converts Run traces to ShareGPT JSONL format for model fine-tuning.

**Relationship to new system**: Completely independent. TrajectorySaver does data format conversion and persistence; it doesn't involve typed memory, layering, or skill lifecycle. No conflict.

**Decision**: **Keep as-is**, no modifications.

---

### 2. BackgroundReviewer (`background_review.rs`) — 🔄 Extend

**Function**: Uses LLM to review conversations, extract memory and skill suggestions. Currently writes raw JSON (no typed metadata) to `["background_reviews"]` namespace.

**Overlap with new system**:
- Review prompts (MEMORY_REVIEW_PROMPT, SKILL_REVIEW_PROMPT, COMBINED_REVIEW_PROMPT) have direct value — reuse
- But writes raw JSON without `MemoryType`, `confidence`, `stability` metadata
- New `TriggerDetector` needs to classify BackgroundReviewer output as `UserCorrection` or `SkillCandidate`

**Decision**: **Keep LLM review mechanism, extend write path**.
- After Phase 0: Change `store.put()` calls to `TypedMemoryStore::put_typed()` with `MemoryType`, `MemorySource::AutoExtracted`, `confidence` metadata
- Keep `ReviewOutcome` return type
- Keep `build_transcript()` method
- Keep three review prompts
- Add: Call `TriggerDetector` for trigger classification after review completes

**Specific change**:
```rust
// Old: store.put(&["background_reviews"], &key, value)
// New: typed_store.put_typed(
//     &["agent", "typed_memories"],
//     &key,
//     content,
//     MemoryMeta {
//         memory_type: MemoryType::UserPreference, // or classify based on content
//         source: MemorySource::AutoExtracted,
//         confidence: 0.6, // AutoExtracted default
//         ..Default::default()
//     },
// )
```

---

### 3. Curator (`curator.rs`) — 🔄 Extend

**Function**: Manages skill lifecycle Active → Stale → Archived. Only operates on agent-created skills.

**Overlap with new system**:
- New system needs 6-state lifecycle: `Candidate → Draft → Active → Stale → Deprecated → Archived`
- Current has only 3 states: `Active, Stale, Archived`
- Need to add `Candidate, Draft` before Active, and `Deprecated` between Stale and Archived

**Decision**: **Extend SkillLifecycle enum**.
- Add 3 new variants to the existing enum
- Extend `apply_transitions()` with new transition paths
- `CuratorStatus` gains new counters
- Keep `SkillMeta`, `CuratorConfig`, `CuratorState` structure
- Keep `pin_skill`, `unpin_skill`, `touch_skill` methods
- Add `promote_candidate()`, `activate_draft()`, `deprecate_skill()` methods

**Specific change**:
```rust
// Old:
pub enum SkillLifecycle { Active, Stale, Archived }

// New:
pub enum SkillLifecycle {
    Candidate,  // New: pattern discovered from memory
    Draft,      // New: SKILL.md created but not activated
    Active,     // Existing
    Stale,      // Existing
    Deprecated, // New: superseded or known outdated
    Archived,   // Existing
}
```

---

### 4. Analyzer (`analyzer.rs`) — ✅ Keep + Integrate

**Function**: Detects failure patterns in run traces (WriteWithoutRead, ExcessiveRetries, ExcessiveToolCalls), generates `ImprovementSuggestion`.

**Relationship to new system**:
- Failure pattern detection directly serves Phase 2 `MemoryReviewer` (write failures as `ErrorResolution` typed memories)
- `CritiqueIssue` enum can be consumed by Phase 4 `SkillPatcher`
- `ImprovementSuggestion::PromptChange` overlaps with `SkillPatch::RefineInstruction` but serves different targets

**Decision**: **Keep as-is, integrate into new system**.
- Analyzer continues as independent run analyzer
- Phase 2 `MemoryReviewer` converts `CritiqueIssue` into `MemoryType::ErrorResolution` memories after analysis
- Phase 4 `SkillPatcher` consumes `CritiqueIssue` and `ImprovementSuggestion` to generate `SkillPatch`
- No modifications needed to Analyzer itself

---

### 5. ImprovementLoop (`loop.rs`) — ✅ Keep As-Is

**Function**: Iterative eval → critique → suggest → re-eval loop. Uses EvalRunner + Analyzer for prompt optimization.

**Relationship to new system**: Different function.
- ImprovementLoop is **eval-driven prompt optimization**
- New evolution system is **memory/skill lifecycle management**
- They are complementary, not overlapping

**Decision**: **Keep as-is**. In the future, the new system may consume ImprovementLoop's `LoopResult` as input for skill health decisions, but no changes needed now.

---

### 6. SelfEvolution (`evolution.rs`) — 🔄 Rename

**Function**: Thin wrapper around ImprovementLoop with HTML report generation. One `.enable()` switch.

**Conflict with new system**:
- Name `SelfEvolution` collides with new `evolution/` module name
- New `evolution/` module has much broader scope (memory, skills, rules, audit)
- Functionally no conflict: this is eval-driven improvement, new module is memory/skill-driven self-evolution

**Decision**: **Rename to `EvalDrivenImprovement`** to avoid naming collision.
- File moves from `evolution.rs` to `eval_improvement.rs`
- Type renamed from `SelfEvolution` to `EvalDrivenImprovement`
- Functionality unchanged
- Update `mod.rs` re-exports

---

### 7. PromptGenerator (`generator.rs`) — ✅ Keep + Reuse

**Function**: Uses LLM to generate improved system prompts from failure analysis.

**Relationship to new system**:
- Phase 3 `SkillDraftGenerator` can reuse `PromptGenerator` to generate SKILL.md drafts
- Phase 4 `SkillPatcher` can reference its prompt construction patterns

**Decision**: **Keep as-is, reused by new system**.
- `SkillDraftGenerator` internally calls `PromptGenerator::generate_improved_prompt()` or similar
- No modifications needed to PromptGenerator itself

---

### 8. CritiqueStore / DualLayerCritiqueStore (`store.rs`) — ❌ Delete

**Function**: In-memory + file-persisted critique storage with pattern aggregation and dual-layer (project/global) support.

**Overlap with new system**:
- New `ChangeLog` (`audit.rs`) provides much more general change tracking (not just Critique, but also Memory/Skill/Rule changes)
- New `TypedMemoryStore` + namespace conventions can replace the dual-layer concept
- `CritiqueStore` uses `.unwrap()` violating project rules (Known Pitfall #5)
- Not integrated in CLI, experimental

**Decision**: **Delete `store.rs`**.
- Replace change tracking with `ChangeLog`
- Implement dual-layer concept via `TypedMemoryStore` namespaces:
  - Project-level: `["project", "evolution", "critiques"]`
  - Global-level: `["agent", "evolution", "critiques"]`
- `CritiqueStore::top_patterns()` can be replaced by `TypedMemoryStore::search_typed()` + aggregation
- Update `mod.rs` re-exports after deletion

---

### 9. Shared Types (defined in `mod.rs`)

| Type | Decision | Reason |
|------|----------|--------|
| `CritiqueIssue` | ✅ Keep | Produced by Analyzer, consumed by SkillPatcher |
| `ImprovementSuggestion` | ✅ Keep | Used by Analyzer and ImprovementLoop |
| `RunCritique` | ✅ Keep | Core analysis data type |

---

## Decision Summary

| Component | Action | Reason |
|-----------|--------|--------|
| `TrajectorySaver` | Keep as-is | Fully independent, no conflict |
| `BackgroundReviewer` | Extend | Write path changes to TypedMemoryStore |
| `Curator` | Extend | SkillLifecycle gains 3 new states |
| `Analyzer` | Keep + integrate | Failure detection serves new system |
| `ImprovementLoop` | Keep as-is | Different function (eval-driven), complementary |
| `SelfEvolution` | Rename to `EvalDrivenImprovement` | Name collision, functionally distinct |
| `PromptGenerator` | Keep + reuse | Reused by SkillDraftGenerator |
| `CritiqueStore` | ❌ Delete | Replaced by ChangeLog + TypedMemoryStore |
| `DualLayerCritiqueStore` | ❌ Delete | Same reason |
| `CritiqueIssue` | Keep | Analyzer output type |
| `ImprovementSuggestion` | Keep | Used by Analyzer/ImprovementLoop |
| `RunCritique` | Keep | Core data type |

## Implementation Order

During Phase 0 implementation, complete these compatibility operations in parallel:

1. **Delete `store.rs`**, update `mod.rs` to remove re-exports
2. **Rename `evolution.rs` → `eval_improvement.rs`**, update `mod.rs` re-exports
3. **Extend `curator.rs`** SkillLifecycle enum
4. **Extend `background_review.rs`** write path

These changes should be part of Phase 0 to ensure new and old modules align in the same phase.
