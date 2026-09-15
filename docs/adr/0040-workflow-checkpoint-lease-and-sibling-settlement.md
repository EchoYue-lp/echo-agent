# ADR 0040: Workflow Checkpoint Lease and Sibling Settlement

## Status

Accepted for the workflow checkpoint and pipeline execution paths.

## Context

Workflow resume currently claims a checkpoint by renaming it out of the
pending set, then parses and consumes the claim before node execution has
settled. A read, parse, delete, or process failure can therefore make a
continuation invisible. A concurrent tag can also load an old checkpoint and
save it after a resume claim, resurrecting a continuation. Pipeline fan-out
and DAG execution wait in submission order and only abort siblings after the
first observed failure, so a pending sibling can hide the terminal error or
outlive a cancelled caller.

## Decision

The existing `CheckpointStore` remains the sole checkpoint authority. A
successful `claim` creates a lease identified by `resume_attempt_id`; the
resume path acknowledges the lease only after the continuation reaches a
terminal result and requeues it on any failure. Claimed files remain visible
to `load` and `list`, and stale file claims are returned to the pending set
under the store's lease policy. Metadata updates use an atomic generation
compare-and-save; adapters without that primitive reject the update rather
than using a racy load-then-save fallback.

Parallel workflow paths use the existing node futures or `JoinSet`, observe
the first completed failure, cancel all siblings, and drain their handles
before returning. Dropping a `JoinSet` also aborts its child tasks when the
caller cancels the outer operation. The streaming graph entry emits
`WorkflowEvent::NodeError` before the terminal stream error and never emits
`Completed` for a failed node.

## Alternatives considered

1. Keep eager claim deletion and retry from the caller. Rejected because a
   process crash leaves no recoverable record and retry cannot distinguish a
   consumed continuation from a failed claim.
2. Preserve load-modify-save for tags. Rejected because it permits ABA
   resurrection after claim; generation CAS makes the conflict explicit.
3. Wait for every sibling before propagating an error. Rejected because a
   hung sibling can mask a failure indefinitely and cancellation drops can
   detach external effects.
4. Extract a new workflow scheduler/state machine. Rejected for this slice;
   the existing `CheckpointStore`, `Graph`, `JoinSet`, and node execution
   paths remain the authorities.

## Consequences

Resume failures remain retryable and observable, while successful resumes are
settled exactly once per lease. Tagging a claimed checkpoint returns a conflict
instead of reviving it. Parallel failures now have bounded cleanup and a
deterministic terminal error. Remote checkpoint adapters must implement the
new atomic settlement methods to support tagging and failure recovery; the
compatibility defaults fail closed where atomicity is unavailable.

## References

- GitHub Issues #109, #110, #112, and #113.
- `echo-orchestration/src/workflow/checkpoint_store.rs`.
- `echo-orchestration/src/workflow/graph.rs`.
- `echo-orchestration/src/workflow/dag.rs` and `concurrent.rs`.
