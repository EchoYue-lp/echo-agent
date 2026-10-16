# Framework delivery checkpoints

This checkpoint tracks ACP Run error isolation. The accepted cancellation and
physical-settlement authority remains [ADR0088](adr/0088-subagent-settlement-observation.md)
and its linked design; this file records delivery progress only.

| Milestone | State | Evidence / next step |
| --- | --- | --- |
| Root cause and official-connection before | Complete | Base9886502 compiled; two behavior failures reproduce connection closure |
| Local Run observation isolation | Implemented | Same two cases pass; expanded ACP integration 29 pass |
| Waiter and receipt priority | Complete | 43 ACP unit tests pass; existing/later waiters, receipt-only wait, receipt priority and stale-error rejection |
| Independent implementation review | Complete | Zero Critical/Important/Minor findings; focused clippy passes with warnings denied |
| Full gates and feature matrix | Complete | Latest main9886502 merged; six workspace gates and 17 standalone features pass, zero warnings/source drift |
| PR / exact main | In review | PR191, published checkpoint30b9bf05; squash only after final review, then verify exact merged-main CI |

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

Current next step: commit the final thin validation record, complete final
review and squash PR191, then verify the exact merged-main CI. Local raw
artifacts remain ignored and must not be staged.
