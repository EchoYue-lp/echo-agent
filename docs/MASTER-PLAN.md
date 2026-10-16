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
| Full gates and feature matrix | Pending | Merge latest main, six required workspace gates and standalone feature checks |
| PR / exact main | Pending | Squash after independent review; verify exact merged-main CI |

The canonical production path is still `AcpConnectionServices::prepare_run` /
`RunEntry` / `AgentTurnDriver`; standard Prompt consumes the Run's fallible
observer while SDK Host can keep its existing prepared-task spawn. There is no
second store, receipt, task lifecycle or replacement executor.

Document and example synchronization: ADR/design, the source migration guide
and both lifecycle guides describe the observer API. Existing ACP example
call sites need no change and are compiled by the full all-target gates. The
website has no new product feature or wire contract to describe; no website
change is required. SDK/CLI consumer validation stays separately owned.

Current next step: qualify the semantic task evidence and publish the first
source checkpoint; then run full final gates. Local raw artifacts
remain ignored and must not be staged.
