# Framework delivery checkpoints

These checkpoints track ACP Run isolation and bounded Workflow test evidence. The accepted cancellation and
physical-settlement authority remains [ADR0088](adr/0088-subagent-settlement-observation.md)
and its linked design; this file records delivery progress only.

| Milestone | State | Evidence / next step |
| --- | --- | --- |
| Root cause and official-connection before | Complete | Base9886502 compiled; two behavior failures reproduce connection closure |
| Local Run observation isolation | Implemented | Same two cases pass; expanded ACP integration 29 pass |
| Waiter and receipt priority | Complete | 43 ACP unit tests pass; existing/later waiters, receipt-only wait, receipt priority and stale-error rejection |
| Independent implementation review | Complete | Zero Critical/Important/Minor findings; focused clippy passes with warnings denied |
| Full gates and feature matrix | Complete | Latest main9886502 merged; six workspace gates and 17 standalone features pass, zero warnings/source drift |
| PR / exact main | Complete | PR191 merged6563d265, reviewed tree2875ec0d; exact CI38958501642 passed all seven jobs |

The canonical production path is still `AcpConnectionServices::prepare_run` /
`RunEntry` / `AgentTurnDriver`; standard Prompt consumes the Run's fallible
observer while SDK Host can keep its existing prepared-task spawn. There is no
second store, receipt, task lifecycle or replacement executor.

Document and example synchronization: ADR/design, the source migration guide
and both lifecycle guides describe the observer API. Existing ACP example
call sites need no change and are compiled by the full all-target gates. The
website has no new product feature or wire contract to describe; no website
change is required. SDK/CLI consumer validation stays separately owned.

Full all-target/all-feature tests: 3,627 passed / 0 failed / 3 ignored across
87 top-level harness summaries. Two real `echo_state` child-process harnesses
each passed one test (382 filtered each), counted separately. The three existing
ignored tests require installed LSP, live scholarly providers or Zotero credentials.

The accepted c0bc semantic validator passed strict source and base9886502
change-evidence qualification on a clean independent Git candidate identical to
the delivery tree. Installed0.5 task validation separately exposed historical
reference errors and a bounded-reference-closure limit; no native Hook/Stop
success is claimed and no historical material or tool limit is rewritten.

ACP delivery is accepted on exact main6563d265. The original worktree and ignored
evidence remain available; task build artifacts were cleaned after referenced
executables were archived and verified. No raw artifacts are staged.

## Workflow observation negative controls

Base6563d265; branch `test/Echoyue/workflow-observation-negative-controls`.
Only the existing Graph test fixture changes. Production Graph/Node/Store,
public API, checkpoint formats and accepted SDK pins remain unchanged.

Two named cases fill the earlier Audit's unexecuted Graph-specific paths:
foreign observation followed by matching progress/local terminal/EOF retains
the original foreign identity; matching terminal followed by a later matching
Unknown remains unresolved after EOF. Both preserve the original claim and
one physical source, reject replacement admission and publish no result or
assistant message into the actual resumed state or same-instance Memory snapshot.
The original matching-observation/later-terminal/EOF positive path stays intact.

Focused five controls: 5 passed / 0 failed / 0 ignored / 505 filtered.
Default-feature `workflow::` subset: 93 passed / 0 failed / 0 ignored / 417
filtered. Both exact selectors, source hashes and raw digests are in
`evidence.workflow-observation-negative-controls`; this is not a new full
workspace, all-feature or merged-main result. The new cases do not add File,
restart or remote Store proof. The broad Finding remains open.

Independent candidate and post-execution result reviews passed with zero
Critical/Important/Minor findings. Exact Git checkpoint qualification and file
necessity checks precede the normal focused milestone commit/push.
The coordinator owns later heavy-slot/main-gate scheduling. No additional Cargo
is authorized during SDK/CLI's current build window.

No public behavior/API changed, so existing product docs, examples and website
need no new feature text or call-site changes. This current index and the thin
Evidence/Audit/Finding chain record the added proof; raw runs and caches stay local.
