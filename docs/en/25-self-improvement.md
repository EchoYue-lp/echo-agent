# Self-Evolution System — Analyze, Evolve, Skill Creation

## Overview

The self-evolution system lets the Agent **continuously improve itself** from run experience: analyze failures, accumulate structured memory, **auto-create skills** from repeated patterns, merge/maintain stale skills, and promote high-confidence knowledge into permanent rules.

It consists of two complementary modules:

- [`improve`](../../src/improve) — **eval-driven** offline improvement: trajectory analysis, prompt suggestions, iterative tuning.
- [`evolution`](../../src/evolution) — **runtime evolution** loop: layered memory, change audit, skill lifecycle (candidate→draft→active), skill merge/health/patch, rule promotion, security.

```
Run the Agent
   │
   ├─ during run ──── TriggerDetector (online memory discovery) ──┐
   ├─ compress/evict ─ memory_promoter (lifecycle mgmt) ──────────┤
   └─ session/task end ─ BackgroundReviewer (deep review) ────────┤
                                                                  ▼
                                          MemoryLayerManager (hot/warm/cold tiers)
                                                                  │
                  ┌──────────────┬───────────────────┬────────────┴──────────┬──────────────┐
                  ▼              ▼                   ▼                       ▼              ▼
            MemoryReviewer  SkillCandidate     SkillHealth/             RulePromoter
           (review/merge/GC) Detector→Draft    Merge/Patch            (→ AGENTS.md)
```

---

## Problem It Solves

As tasks get harder and edge cases accumulate, Agent performance degrades. Without systematic improvement:

- **Repeating failure modes**: Agent writes before reading, over-retries failing tools, misses obvious tools
- **No feedback loop**: past failures don't affect future behavior
- **No experience memory**: every session starts from scratch
- **Skill staleness/redundancy**: skills go stale unnoticed, or multiple skills overlap
- **Knowledge never settles**: learned high-value experience stays ephemeral and never becomes a permanent rule

---

## Safety Model

All advisory changes require human review. The system **never automatically**:

- Modifies core runtime code
- Relaxes safety policies / changes permission rules
- Applies skill merges, patches, or rule promotions (it only generates proposals, applied by humans via commands)
- Promotes memory from untrusted sources (tool output) into the hot layer or rules

Every mutation to memory/skills/rules is written to the change audit log (`change-log.jsonl`), which is queryable and rollback-capable. Writes also undergo secret scanning and prompt-injection detection.

---

## Module Map

| Module | Responsibility | Location |
|--------|---------------|----------|
| **Analyzer** | Detect failure modes in run trajectories | `improve/` |
| **ImprovementLoop** | evaluate→critique→suggest→re-evaluate tuning | `improve/` |
| **EvalDrivenImprovement** | One-switch full eval-improvement loop (formerly `SelfEvolution`) | `improve/` |
| **PromptGenerator** | LLM-driven prompt improvement | `improve/` |
| **TrajectorySaver** | Convert runs into ShareGPT fine-tune data | `improve/` |
| **TypedMemoryStore** | Typed memory read/write with metadata | `echo-state` |
| **MemoryLayerManager** | Hot/warm/cold tiered memory management | `evolution/` |
| **ChangeLog** | Change audit and rollback | `evolution/` |
| **TriggerDetector** | Online conversation signals → new memory | `evolution/` |
| **MemoryReviewer** | Staleness scoring, conflict detection, merge, archival (GC) | `evolution/` |
| **Curator** | Skill lifecycle state machine | `evolution/` |
| **SkillCandidateDetector** | Discover skill candidates from repeated patterns | `evolution/` |
| **SkillDraftGenerator** | Generate draft SKILL.md from candidates | `evolution/` |
| **SkillSimilarityDetector / SkillMerger** | Detect overlapping skills and merge | `evolution/` |
| **SkillHealthMonitor** | Skill health scoring (drives deprecation) | `evolution/` |
| **SkillPatcher** | Generate skill patches from failure telemetry | `evolution/` |
| **RulePromoter** | High-confidence memory → AGENTS.md rules | `echo-agent-app-core` |
| **ReviewIntegration / Dashboard** | Product-layer review scheduling and status dashboard | `echo-agent-app-core` |

---

## Part 1: Eval-Driven Improvement (`improve`)

### Analyzer — Failure Detection

`Analyzer` inspects a finished run trajectory and detects common failure modes:

```rust
use echo_agent::improve::Analyzer;

let critique = Analyzer::analyze(&run);
println!("{}", critique.format_report());
```

Output:

```
Run: run_abc123
Success: false (score: 0.65)

Issues found:
  - Write without read: write called 2x before read_file
  - Excessive retries: shell retried 4x

Suggestions:
  [prompt] tools: add instruction: 'always read_file before editing'
  [policy] force_read_before_edit: true — reason: write-before-read 2x
```

#### Detected Issue Types

| Issue | Detection logic | Suggestion |
|-------|-----------------|------------|
| `WriteWithoutRead` | Write tool called on a file before a read tool | Add read-before-write prompt |
| `ExcessiveRetries` | Same tool errored > 2 times | Add "try a different approach" instruction |
| `ToolErrorPattern` | Same tool repeatedly fails | Generate eval cases for that tool |
| `ContextOverflow` | Context compression triggered | Suggest context-aware prompt |
| `MissingTool` | Expected tool not used | Suggest adding tool instruction |
| `ExcessiveToolCalls` | > 20 tool calls in one run | Add efficiency instruction |

### ImprovementLoop — Iterative Improvement

```rust
use echo_agent::improve::ImprovementLoop;

let loop_runner = ImprovementLoop {
    max_iterations: 5,
    improvement_threshold: 0.95,  // stop when test score >= 95%
    holdout_ratio: 0.4,           // 40% test, 60% train
};

let result = loop_runner.run(&cases, agent_factory, &run_store).await;
println!("Best score: {:.2} at iteration {}", result.best_score, result.best_iteration);
```

How it works: stratify cases by standard type to prevent overfitting → train eval → `Analyzer` critique → generate deduped suggestions → blind test eval on holdout → track best → early-stop at threshold.

### EvalDrivenImprovement — One Switch

> **Note**: the former `SelfEvolution` type has been **renamed** to `EvalDrivenImprovement` (to avoid a naming collision with the new `evolution` module).

```rust
use echo_agent::improve::EvalDrivenImprovement;

let result = EvalDrivenImprovement::new()
    .with_eval_cases(cases)
    .with_run_store(run_store)
    .max_iterations(5)
    .with_report_dir("./eval_reports")
    .enable()
    .run(|| create_agent())
    .await;
```

### PromptGenerator and TrajectorySaver

- `PromptGenerator` — uses an LLM to generate an improved system prompt based on `Analyzer` failure analysis.
- `TrajectorySaver` — converts finished runs into ShareGPT JSONL for model fine-tuning:

```rust
use echo_agent::improve::TrajectorySaver;

let saver = TrajectorySaver::default_dir()?;
saver.save(&run, "qwen3-max").await?;
let entries = saver.list(Some("2026-05-29")).await?;
```

---

## Part 2: Runtime Evolution Loop (`evolution`)

This is the core self-evolution capability added in `v0.2.x`, letting the Agent accumulate and reuse knowledge during runs.

### Typed Memory — `TypedMemoryStore`

Every memory carries structured metadata `MemoryMeta`: type, confidence, stability, risk, status, source, topic. Backward compatible — legacy untyped entries get default metadata on read.

```rust
use echo_state::memory::typed_store::{TypedMemoryStore, MemoryFilter};
use echo_core::memory::types::{MemoryMeta, MemorySource, MemoryType, MemoryStatus};

let store = TypedMemoryStore::new(arc_store);

// Write a memory with metadata
let meta = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::UserCorrection, "build-tool")
    .with_confidence(0.9)
    .with_stability(0.8);
store
    .put_typed(&["agent", "typed_memories"], "build:java8", "Project uses Java 8", meta)
    .await?;

// Filtered retrieval
let filter = MemoryFilter::new()
    .with_type(MemoryType::ProjectFact)
    .with_min_confidence(0.7);
let entries = store.list_typed(&["agent", "typed_memories"], &filter).await?;
```

#### MemoryType categories

`UserPreference | ProjectFact | ArchitectureDecision | DebuggingLesson | ErrorResolution | CommandPattern | ToolUsage | WorkflowPattern | SkillCandidate | DeprecatedNote`

#### MemorySource and default confidence

| Source | Meaning | Default confidence |
|--------|---------|--------------------|
| `ExplicitSave` | `/remember` or `remember` tool | 1.0 |
| `UserCorrection` | Detected user correcting the Agent | 0.9 |
| `ErrorResolution` | Tool failed then succeeded with a different approach | 0.85 |
| `RepeatedWorkflow` | Same tool sequence observed ≥3 times | 0.75 |
| `AutoExtracted` | AutoMemory extracted from session archive | 0.6 |

### Tiered Memory Management — `MemoryLayerManager`

Memory is tiered by value; the hot tier is always in context, warm is retrieved on demand:

- **Hot** (`.echo-agent/MEMORY.md`): highest value, YAML frontmatter + markdown body, ~2000 token cap, editable by both humans and the Agent.
- **Warm** (Store KV `["agent","memories"]`): unified typed-memory store; organized by topic, loaded on demand. Memories can be `Active` or `Archived` (staleness is a recall-decay weight, not a layer move — Archived stays recallable with decay).
- **Cold** (optional; Store KV `["agent","cold_memories"]`): retained as pub API for consumers aligned with Letta/MemGPT archival memory (recall-on-demand, not proactively loaded). The default product path collapses cold into `Warm`+`Archived`; consumers who need a distinct cold tier can opt in via `COLD_NAMESPACE`.

```rust
use echo_agent::evolution::{MemoryLayerManager, JsonlChangeLog, MemoryMeta, MemorySource, MemoryType};
use std::path::PathBuf;

let mgr = MemoryLayerManager::new(
    PathBuf::from(".echo-agent"),
    arc_store,
    Box::new(JsonlChangeLog::new(PathBuf::from(".echo-agent/evolution/change-log.jsonl"))),
);

// Write (auto-scans secrets/injection, promotes to hot based on confidence)
let meta = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "deploy")
    .with_confidence(0.95);
mgr.write_memory("deploy:prod-script", "Build with pnpm build", meta).await?;

// Promote / demote
mgr.promote("some-key").await?;          // cold→warm→hot
mgr.demote("some-key", "stale").await?;  // hot→warm→cold

// Cross-tier search
let hits = mgr.search_layered("deploy", 10).await?;
```

### Change Audit — `ChangeLog`

Every mutation to memory/skills/rules is recorded in an append-only JSONL, filterable:

```rust
use echo_agent::evolution::{ChangeFilter, ChangeType, EntityType};

let filter = ChangeFilter::new()
    .with_entity_type(EntityType::Memory)
    .with_change_type(ChangeType::Promote)
    .with_limit(50);
// log file: .echo-agent/evolution/change-log.jsonl
```

### Memory Review and GC — `MemoryReviewer`

Memory accumulates. The reviewer scores the warm layer for staleness, detects conflicts, merges, and archives:

```
staleness = age·0.35 + low_usage·0.20 + instability·0.20 + contradiction·0.20 + source_weakness·0.05
```

| Staleness | Status |
|-----------|--------|
| < 0.40 | Active |
| 0.40–0.65 | Stale |
| 0.65–0.85 | Deprecated |
| ≥ 0.85 | Archived (demoted to cold) |

```rust
use echo_agent::evolution::{MemoryReviewer, ReviewConfig};

let reviewer = MemoryReviewer::new();
let report = reviewer
    .review(&typed_store, &layer_manager, &change_log, &ReviewConfig::default())
    .await?;
// report.archived / report.merges_applied / report.superseded_keys
```

The product layer schedules this via `ReviewIntegration`: auto-triggers every 50 memory writes or at session end, and can be run manually via `/memory-review`.

### Skill Lifecycle and Auto-Creation

#### Full lifecycle (Curator state machine)

```
Candidate → Draft → Active → Stale → Deprecated → Archived
```

`Curator` (in `evolution/`) manages these transitions:

```rust
use echo_agent::evolution::{Curator, CuratorConfig, SkillLifecycle};

let curator = Curator::new(
    CuratorConfig { stale_days: 30, archive_days: 90, enabled: true },
    "~/.echo-agent/curator_state.json",
);
curator.register_candidate("cargo-build")?;   // candidate
curator.promote_to_draft("cargo-build")?;      // → draft
curator.promote_to_active("cargo-build")?;     // → active
curator.pin_skill("critical-skill")?;          // pin (exempt from auto-transition)
let transitions = curator.apply_transitions()?; // auto-transition by idle time
```

#### Auto-creating skills from observed patterns

1. **`SkillCandidateDetector`** scans `TypedMemoryStore` for `WorkflowPattern`/`DebuggingLesson` memory; when ≥3 entries share a topic with source `RepeatedWorkflow` → proposes a skill candidate.

   ```rust
   use echo_agent::evolution::SkillCandidateDetector;
   let detector = SkillCandidateDetector::new();
   let report = detector.detect(&typed_store, &change_log).await?;
   // report.new_candidates / report.reinforced
   ```

2. **`SkillDraftGenerator`** generates a draft `SKILL.md` from a candidate via template, saved to `.echo-agent/skills/_drafts/<name>/SKILL.md`.

   ```rust
   use echo_agent::evolution::SkillDraftGenerator;
   let gen = SkillDraftGenerator::new(".echo-agent".into(), &change_log);
   let result = gen.generate_from_candidate(&candidate).await?;
   // result.skill_md_path points to the generated draft
   ```

3. After human review, `/skill-promote <name>` moves Draft → Active and the skill appears in the skill catalog.

### Skill Merge, Health, Patch

| Component | Scoring formula / behavior |
|-----------|---------------------------|
| **SkillSimilarityDetector** | `description·0.25 + trigger·0.30 + scope·0.15 + tool·0.10 + pitfall·0.10 + co_activation·0.10`; ≥0.75 proposes merge, ≥0.90 strongly recommends |
| **SkillMerger** | Applies merge proposals: keeps the higher-activation skill as primary, absorbs the secondary's triggers and unique instructions; requires `/skill-merge <a> <b>` to apply |
| **SkillHealthMonitor** | `success_rate·0.30 + recent_success·0.20 + usage·0.10 + freshness·0.15 + approval·0.15 + cmd_validity·0.10`; ≥0.75 healthy, <0.55 unhealthy |
| **SkillPatcher** | Analyzes telemetry `common_failures` → generates `SkillPatch` (add precondition/tool/error-handling); requires `/skill-patch <name>` to apply |

```rust
use echo_agent::evolution::{SkillSimilarityDetector, SkillHealthMonitor};

let detector = SkillSimilarityDetector::new(arc_store.clone());
// pass current skill descriptors; ≥0.75 similarity yields merge proposals, applied via `/skill-merge`
let proposals = detector.scan_and_propose(&skill_descriptors, &change_log).await?;

let monitor = SkillHealthMonitor::new(arc_store);
for report in monitor.analyze_all_skills().await? {
    println!("{}: {:?}", report.skill_name, report.status);
}
```

### Rule Promotion (product layer)

`RulePromoter` (in `echo-agent-app-core`) scans high-confidence memory (confidence≥0.95, stability≥0.9, revision_count==0), generates a `RuleProposal`, and after human approval via `/rule-promote` writes it into `.echo-agent/AGENTS.md`, marking the source memory as `Superseded`.

### Security Hardening — `EvolutionSecurityGuard`

- **Pre-write**: secret scanning (AWS `AKIA...`, GitHub `ghp_...`, `BEGIN PRIVATE KEY`, etc.; matches replaced with `[REDACTED]`) + prompt-injection detection (e.g. "ignore previous" patterns)
- **Untrusted-input isolation**: memory from tool output gets `risk = High` and cannot be promoted to hot layer or rules without human approval
- **Rate limiting**: max 50 memory writes per session, max 5 skill patches per day
- All changes are rollback-capable via `ChangeLog`

---

## Automatic Memory Responsibility Boundaries

There are three automatic memory paths with strictly divided responsibilities to avoid redundant systems:

| System | Primary responsibility | Should NOT do |
|--------|----------------------|---------------|
| `TriggerDetector` (runtime) | Lightweight online discovery of new memory during a conversation (user preferences, corrections, verified error resolutions, repeated workflows) | Session-archive summarization, `.echo-agent/project.md` writes |
| `AutoMemory` (framework+app) | Session-end/manual-trigger archive summarization (extract observations, classify, write typed memory; app layer may write `project.md`) | Compression/eviction, runtime policy scheduling |
| `memory_promoter` (compression path) | Lifecycle management of messages compressed/evicted due to token pressure (persist, evict, demote) | New-preference discovery, UI-triggered extraction |
| `BackgroundReviewer` (app-scheduled) | Async deep review after a finished run, extracting high-value memory and improvement signals | GUI/TUI/CLI product scheduling policy |

> Key constraint: any typed memory that must enter runtime recall **must** go through the framework's `MemoryLayerManager::write_memory`; the product layer does not maintain its own category/type/key/write rules.

---

## File Layout

```
.echo-agent/
  MEMORY.md                        # hot tier (human-readable, editable by Agent and humans)
  AGENTS.md                        # auto-promoted rules
  project.md / local.md            # existing static prompt files
  memory/
    topics/*.md                    # warm-tier topic files
    archive/                       # cold-tier archive
  evolution/
    change-log.jsonl               # change audit log
    skill_candidates/              # candidate proposals
    patches/                       # skill patches
  skills/
    _drafts/<name>/SKILL.md        # draft skills
  curator_state.json               # Curator state
```

## Store Namespaces

| Namespace | Purpose |
|-----------|---------|
| `["agent", "typed_memories"]` | typed memory (warm tier) |
| `["agent", "cold_memories"]` | archived memory (cold tier) |
| `["agent", "skill_candidates"]` | skill candidate proposals |
| `["agent", "skill_telemetry"]` | skill telemetry |
| `["agent", "profile"]` | Agent profile |
| `["agent", "evolution", "patches"]` | skill patches |
| `["agent", "evolution", "merges"]` | merge proposals |
| `["agent", "evolution", "rules"]` | rule proposals |

---

## Feature Flags and Usage Modes

`evolution` ships with the framework by default; `improve` and `eval` are feature-gated:

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["improve"] }
```

> Using `EvalDrivenImprovement` / `ImprovementLoop` requires `eval` too (`improve` depends on it).

The self-evolution system is **not built into the Agent loop**; it runs as an independent analysis/evolution pass, keeping the Agent lightweight:

```
Production (no self-evolution):  agent.execute("do the task").await
Self-improvement (offline pass): EvalDrivenImprovement::new()...run(agent_factory).await
Runtime evolution:               MemoryLayerManager / TriggerDetector / ReviewIntegration integration
```

### Difference from Self-Reflection

| Dimension | Self-Reflection | Self-Evolution |
|-----------|----------------|----------------|
| When | During Agent execution | After / continuously at runtime |
| Scope | Single task | Cross-task patterns |
| Feedback | Language (LLM critique) | Structured (memory, skills, rules) |
| Feature flag | `self-reflection` | `improve` + `evolution` |
| Integration | Built into Agent loop | External batch / runtime evolution |

See also: [24 - Eval System](./24-eval-system.md) for the evaluation framework driving the `improve` pipeline; [03 - Memory](./03-memory.md) for the underlying Store.
