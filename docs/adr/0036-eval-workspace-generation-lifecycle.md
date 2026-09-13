# ADR 0036: Eval workspace generation lifecycle

- Date: 2026-09-14
- Owners: `eval/runner`, `improve/loop`

## Status

Accepted.

## Context

`EvalRunner` currently derives a fixture destination from
`workspace_root/case.id`, deleting that destination before copying. Cases
without fixtures receive `workspace_root` itself as their working directory.
Concurrent runs with the same case ID can therefore remove one another's
workspace, while fixture-free runs share all file effects.

`ImprovementLoop` constructs process-global `temp/improve_<iteration>`
directories and removes them manually only after the early-stop branch.
Independent loops can collide, and a successful early stop leaves the
directory behind. `AbComparator` creates a UUID parent directory but never
owns its cleanup.

The timeout boundary is separate. Issue #48 records that cancelling the Eval
stream does not prove its producer has reached terminal settlement. Deleting a
workspace immediately after timeout can race with a still-running Tool or file
effect.

OpenAI Evals gives each ML Agent sample a `TemporaryDirectory` containing its
workspace and logs. Inspect AI documents that every sample receives a distinct
sandbox instance and separates ordinary cleanup from interrupted or explicitly
disabled cleanup. The Rust `tempfile` crate provides random directory
creation, explicit `close()` with observable errors, and `keep()`.

- <https://github.com/openai/evals/blob/8eac7a7de5215c907fbddc30efdaf316913eccdd/evals/elsuite/hr_ml_agent_bench/eval.py#L61-L85>
- <https://github.com/UKGovernmentBEIS/inspect_ai/blob/b6589d81f449112bb9942b0003b6fd9b54f1c48e/docs/sandboxing.qmd#L176-L183>
- <https://github.com/UKGovernmentBEIS/inspect_ai/blob/b6589d81f449112bb9942b0003b6fd9b54f1c48e/docs/extensions-sandboxes.qmd#L112-L149>
- <https://docs.rs/tempfile/latest/tempfile/struct.TempDir.html>

## Options Considered

1. Continue using case IDs and add UUID suffixes manually. This addresses name
   collisions but leaves caller cancellation, cleanup errors, and ownership to
   ad hoc code.
2. Automatically delete every generation through `TempDir::Drop`. This is
   concise, but Drop ignores deletion errors and would remove a timeout
   workspace before the producer is known to be settled.
3. Add a background workspace reaper. Correct reclamation requires the missing
   Turn terminal receipt from Issue #48 and would duplicate that repair.
4. Use one private TempDir-backed generation per `EvalRunner::run`, disable
   implicit Drop cleanup, explicitly close settled runs, and retain unsettled
   runs.

## Decision

Choose option 4.

- `workspace_root` is the parent for generated directories, never an Agent
  working directory and never a path recursively reset by case ID.
- Each `run` creates an `eval-` prefixed random generation before fixture or
  Agent effects. Both fixture and fixture-free cases use that path as cwd.
- A private `EvalWorkspaceGeneration` owns `Option<TempDir>`. Its builder
  disables automatic cleanup so a dropped run future fails safe by retaining
  the directory and logging a warning.
- Fixture setup copies into the empty generation. It does not inspect or remove
  any previous case directory.
- Success, typed Agent failure, and completed grading/trace evaluation are
  settled paths. They explicitly call `TempDir::close()`; deletion errors are
  added to the same `EvalResult` and make it unsuccessful.
- Timeout triggers the existing cancellation token but calls `keep()` instead
  of cleanup. The retained path is included in a violation so a caller can
  inspect or remove it after external settlement.
- If fixture setup fails before Agent start, cleanup is safe and explicit.
- `ImprovementLoop` and `AbComparator` use the system temp directory only as
  the EvalRunner parent. They no longer name or remove child workspaces.

## Framework And Application Boundary

Per-run generation identity, fixture staging, cwd binding, and cleanup
disposition are framework Eval mechanics. They apply to every embedding
application. EKO workspace, worktree, UI, and product retention policy do not
enter this implementation.

## Consequences

- Same-ID and fixture-free runs cannot share or delete one another's working
  directory.
- Early-stop and ordinary errors use the same settled cleanup path as success.
- Timeout and caller-drop directories can remain on disk. This is deliberate
  containment of an unsettled producer, not a claim of cleanup completion.
  Issue #48 remains responsible for bounded Turn settlement and later
  reclamation.
- Cleanup failure becomes visible in EvalResult instead of being ignored.
- Callers that treated `workspace_root` itself or
  `workspace_root/case.id` as a durable result path must stop doing so.
- Public Rust types, serialization, SDK identities, and scoring semantics do
  not change.

## Compatibility And Rollback

`EvalRunner::new(PathBuf)` and its public fields remain source compatible;
the PathBuf is now explicitly a generation parent. The bilingual Eval
documentation describes the behavioral contract.

Rollback requires reverting the runner and both consumers together. Restoring
fixed case or iteration paths is not an acceptable partial rollback.

## Verification

Deterministic tests run the same fixture case and fixture-free cases
concurrently, recording the Agent invocation cwd and verifying distinct paths.
Settled paths must disappear; an injected cleanup error must make EvalResult
fail. Timeout and caller-drop paths must remain and be removed by the test.

Improvement tests cover early-stop and concurrent loops without fixed
`improve_i` directories. Source checks confirm Comparator no longer creates
an `ab_compare_<uuid>` parent. Existing Eval criteria, trace, Improve,
bilingual documentation, feature, SDK zero-diff, semantic, Issue, and
independent-review gates remain required.
