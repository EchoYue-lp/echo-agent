---
{
  "schema_version": 1,
  "id": "evidence.framework-history-current-reobservation",
  "kind": "evidence",
  "observed_at": "7110a27e7b0e9841dcfa0d4cca1cdbc053d96177",
  "source_refs": [
    "echo-state/src/journal/mod.rs",
    "echo-state/src/journal/file.rs",
    "echo-state/src/journal/segmented.rs",
    "src/state/mod.rs",
    "tests/acp_agent_adapter.rs",
    "docs/en/41-persistence-concepts.md",
    "docs/zh/41-persistence-concepts.md",
    "docs/adr/0055-checkpoint-journal-identity.md",
    "echo-orchestration/src/tasks/command_cell.rs",
    "docs/adr/0025-deterministic-command-cell-watcher.md",
    "echo-integration/src/redaction.rs",
    "echo-integration/src/mcp/server_config.rs",
    "echo-integration/src/mcp/config_loader.rs",
    "echo-integration/src/mcp/client.rs",
    "echo-integration/src/mcp/transport/mod.rs",
    "echo-integration/src/mcp/transport/http.rs",
    "echo-integration/src/mcp/transport/sse.rs",
    "echo-integration/src/mcp/transport/stdio.rs",
    "echo-integration/src/channels/channels/qq/channel.rs",
    "echo-integration/src/channels/channels/qq/api.rs",
    "echo-integration/src/channels/channels/qq/gateway.rs",
    "echo-integration/src/channels/channels/feishu/channel.rs",
    "echo-integration/src/channels/channels/feishu/api.rs",
    "echo-integration/src/channels/channels/feishu/long_poll.rs",
    "echo-integration/src/channels/channels/feishu/webhook.rs",
    "docs/en/08-mcp.md",
    "docs/zh/08-mcp.md",
    "docs/en/15-im-channels.md",
    "docs/zh/15-im-channels.md",
    "echo-core/src/utils/retention.rs",
    "src/trace/mod.rs",
    "echo-state/src/audit/file.rs",
    "echo-integration/src/lib.rs",
    "echo-execution/src/skills/hooks.rs",
    "src/agent/react/run/pipeline.rs",
    "docs/en/07-skills.md",
    "docs/en/23-hooks.md",
    "docs/zh/07-skills.md",
    "docs/zh/23-hooks.md",
    "echo-integration/src/mcp/types.rs",
    "echo-integration/src/mcp/tool_adapter.rs",
    "echo-orchestration/src/human_loop/service.rs",
    "src/agent/snapshot.rs",
    "docs/adr/0050-mcp-tool-local-classification.md",
    "echo-execution/src/skills/registry.rs",
    "echo-execution/src/skills/mod.rs",
    "echo-execution/src/skills/external/activate_tool.rs",
    "echo-execution/src/skills/external/resource_tool.rs",
    "echo-execution/src/skills/external/run_script_tool.rs",
    "src/skills/mod.rs",
    "src/agent/react/capabilities.rs",
    "src/agent/react/mod.rs",
    "src/agent/react/run/context.rs",
    "src/agent/react/tests.rs",
    "CHANGELOG.md",
    "docs/adr/0044-skill-activation-authority.md",
    "echo-orchestration/src/workflow/graph.rs",
    "echo-orchestration/src/workflow/node.rs",
    "echo-orchestration/src/workflow/pipelines/data_pipeline.rs",
    "echo-orchestration/src/workflow/pipelines/writing_pipeline.rs",
    "docs/adr/0052-workflow-entry-loop-authority.md",
    "docs/en/17-graph-workflow.md",
    "docs/zh/17-graph-workflow.md",
    "echo-orchestration/src/workflow/checkpoint_store.rs",
    "echo-orchestration/src/workflow/mod.rs"
  ],
  "supports": [
    "behavior.observation-persistence",
    "rule.fact-projection-separation",
    "behavior.task-subagent-execution",
    "behavior.effect-permission-execution",
    "rule.permission-effect-order",
    "behavior.extension-publication",
    "rule.extension-generation-authority",
    "behavior.workspace-composition",
    "rule.framework-layer-ownership"
  ],
  "limitations": [
    "New current native audit, not restoration or before/after equivalence of unavailable historical commits.",
    "SDK accepted mutation/classifier/inventory owner retained; new Host/three-language runtime verification remains unverified here.",
    "Actual tests are the existing7110 full05; old counters/reviews remain historical; no new Rust execution."
  ]
}
---

# Current source re-observation at7110

## 来源与范围

The user explicitly directed: “我们的项目目前处于快速迭代中，既然旧的提交没法找到，你就按照最新的代码进行审计、处理。” This is a new bounded audit of fixed7110, not reconstruction of unavailable commits. Twenty-two existing Evidence/Audit headers now identify this current observation; their old execution/review prose and immutable875 documents remain historical. The old workflow typed-equivalence header is retained through its875 document, not asserted for the current protected-claim implementation.

Root dispatched three independent read-only reviews of current state/workflow, protocol/hooks/local classification, and redaction/Skill. Each returned C0/I0/M0 for its bounded current paths and the named existing regressions. They compared current source bytes with7110 and the same1763-input maps used by the actual full05 run. No new historical commands or tests were run for this metadata repair.

## 支持的结论

| Current native boundary | Source observation and actual regression evidence | Limit |
| --- | --- | --- |
| Checkpoint journal | Exact receipt/checkpoint identity, generation, mixed-v1/digest/prune-marker checks; current Journal module115 passed. | Does not reproduce the historical116 count or missing old tree. |
| CommandCell | Original owner cancellation, stop finalizer and lease/prune behavior; current module37 passed. | Not the historical36; does not prove underlying slow filesystem I/O physically stopped. |
| Graph workflow | Shared finish/resume/token/NodeError ordering and cancellation/owner paths; current Graph module45 and demo34/39 passed. Current protected Store and typed Unknown are included. | Not historical70 or old “Store unchanged/all errors failed” equivalence. Graph foreign/late-Unknown专项 is source coverage; real providers, Windows and SDK runtime are unverified here. |
| Hooks | run_hooks→merge_result retains deny priority; permission-bearing results do not stop later deny, ordinary stop remains. Command/HTTP share parsing. full05 rows3101/3106/3107/3171 passed. | No new live HTTP fixture. |
| MCP negotiation/local classification | Shared four-version support; unsupported version rejects before initialization. Typed failures retain cleanup owner. Conservative local Mutating/Standard/Write authority and pre-hook/live/execute checks retain effect. full05 rows3387/3466/3467/3459/3454/3455/1496/1491/1492/1508/3259 passed. | Retaining cleanup owner is not proof every close completed. Qualified probe+adapter composition is not a real external MCP server. Old SDK inventory90/5,29,75 is not a current F assertion. |
| Credential diagnostics | Exact UTF-8 longest-secret substitution precedes retention; JSON/URL/reqwest and MCP stdio/HTTP/SSE helpers plus QQ/Feishu Debug/error paths inspected. full05 rows3276–3277/3394/3420–3421/3493/3590–3595 and ordinary2239/2261 passed. | User raw serialization, arbitrary third-party subscribers and real external services are outside this proof. |
| Skill activation | One registry claim/publication epoch-generation-pointer authority, poison/view/policy-restore/singleflight and Agent validate-before-replace/remove/reset paths inspected. full05 rows1400/1484/1589/3201–3209/3219/3062/3263 passed. | Restore without cached content requires reset. No arbitrary OS-side-effect exactly-once claim. |

The existing full05 result is actual exit0,376219 bytes, SHA256 `8f2dbcd2c6f44e920679dedc26aff8b5a90285d2f81c37a7aea5fc5aa70b13f7`, complete/not truncated. Its89 result blocks contain3624 passes; excluding the two echo_state child singleton reports yields3622 main passes/0 failures/3 existing opt-in ignored. Module counts above are subsets, not additions. All6+17 commands passed with unchanged1763 source inputs at7110; the ZIP Evidence retains the complete gate/cleanup scope. Raw logs, old canonical191-error failures and review receipts remain ignored local evidence.

## Independent SDK ownership retained

The four affected semantic objects formerly referenced SDK files now maintained by the independent accepted SDK product. Their obligations are not retired or silently deleted. Root checked these source bytes against public019e; these are ownership/source links, not a new runtime execution:

- [MCP/source-family inventory](https://github.com/EchoYue-lp/echo-agent-sdk/blob/019e700279677a2eb77e265b5038d9d581bc22da/echo-sdk-protocol/tests/facade_inventory.rs#L2562), with accepted-vs-telemetry at152 and classification at2120; not all MCP behavior.
- [Skill mutation adapter](https://github.com/EchoYue-lp/echo-agent-sdk/blob/019e700279677a2eb77e265b5038d9d581bc22da/echo-sdk-host/src/core_profile/facade/source_operations.rs#L5460): Session tag/remove calls the original Agent unregister/tag authority, not a second registry.
- [Process-local classifier](https://github.com/EchoYue-lp/echo-agent-sdk/blob/019e700279677a2eb77e265b5038d9d581bc22da/echo-sdk-protocol/src/facade.rs#L4286) and [negotiated facade admission regression](https://github.com/EchoYue-lp/echo-agent-sdk/blob/019e700279677a2eb77e265b5038d9d581bc22da/echo-sdk-host/tests/core_profile_e2e.rs#L2370).

The old actual SDK execution epoch was7f14 (225 Rust inputs, workspace rows213/349/519/550); matching019e source bytes do not make those commands newly executed at019e. Current framework pin, protected Workflow/settlement behavior, native Host and three-language accepted-contract verification remain the SDK lane's subsequent acceptance responsibility. This F re-observation records them as unverified, without removing public-contract obligations.

## 已知缺口

[Issue189](https://github.com/EchoYue-lp/echo-agent/issues/189) records the unavailable historical observations and the user's current-code decision. Seven original trees were not found; cause is unknown. One separate originalf310 object was recovered and retained by its nonrelease evidence tag. Current re-observation does not restore any of the seven trees or retroactively prove their old before/after runs. Their original SHA/document links remain with each object. No source, manifest, lock, test, CI policy or validator was changed by this metadata repair.
