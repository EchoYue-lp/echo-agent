# ADR 0090: Retire only A2A and LSP after consumers remove their dependencies

## Status（2026-10-24 历史原记录）

Accepted product direction on 2026-10-24; implementation has not started in
this documentation contribution. At framework baseline
`51726454985fd6f064b21a7e9c539e22d494c987`, A2A and LSP still exist and remain
available. This record neither removes an API nor proves consumer migration,
merged-main CI, or release acceptance.

## 2026-10-28 后继源码实施同步

本节与已批准的完整F30物理删除包共同采用，采用前须由Framework唯一writer核对真实source。
A2A／LSP的feature、模块、公开facade、专属example及LSP插件专属字段退出，不使用空成功shim。
旧`lsp.yaml`仍由原cold prepare的`PreparedPluginDocument`冻结、hash并在同generation私有
容器持有，通用diagnostics保完整错误/路径/身份；缓存和invalidation仍原owner。健康同级
组件与publication/rollback/shutdown保留，ProjectRoot/Instruction/scope及其它合理API不删。
SDK接入此删后源时应只原样投影diagnostics，不再依赖`lsp_config_file`或追加自造退役事实。

EKO独立TUI/REPL/CLI/JSONL退役属应用包；GUI内置终端、Agent shell、桌面最小参数、IM、
cron、共享恢复和独立SDK Host stdio继续保留。没有新增executor、Store、CRUD或权限闸。

以下原5172645基线表与早期桥接段落均属历史对照，不再表示当前可调用API。固定tag/分支、
Exa独立提交与用户数据不改。源码采用／文档同步与删后构建、正常锁、依赖策略、SDK正式
删除源Git pin、native、main CI和发布是独立资格；本节不据旧focused或受控Host宣告完成。

## Context and authority

The approved EKO product direction keeps the desktop GUI as its user entry
point, with IM receive/send and scheduled tasks. EKO's independent TUI,
REPL/interactive CLI and JSONL product surfaces are to retire. Its embedded
terminal, Agent shell execution, MCP, shared Task/Subagent execution and
recovery remain. Desktop startup arguments and the independent SDK Host's
stdio protocol are not those retired EKO product surfaces.

The same explicit decision retires A2A and LSP from the framework, SDK and EKO.
It is a narrowly approved exception to the framework's public-API preservation
rule, not an inference that anything unused by EKO is dead. The framework
continues to serve independent consumers. In particular, ACP is a different
protocol and remains supported; A2A retirement does not retire ACP.

This record implements the already approved product-boundary-contract outcome
of the cross-repository EKO GUI convergence design. The immutable execution
inputs were checked with these SHA-256 values:

| Input | SHA-256 |
| --- | --- |
| `design.md` | `1add234fe9bd5bbf6865cbddcad63f59b7c7264eba2e8fa245d13fd4daf12031` |
| `plans/plan_01_product-boundary-contract.md` | `22773834501db005dd567c99d23b338c659e752a5bd08e3ee9eb417a4c7f24a1` |
| `plans/delivery-map.md` | `cd50060800b1d9faa28076815139a6f690489ecb8f4424f6672d54b3ffbacd61` |

Those files belong to the workspace's phase materials under
`docs/supreme/specs/eko-gui-product-convergence/`; this framework ADR owns the
long-term framework retirement boundary. It records an approved choice rather
than choosing a new protocol or deriving product policy from another product.

The user-approved Delivery Map revision replaces the initial digest
`3437b86a4263a479037c6a6df793d213fe5eaaf52789704152ec473207fb74d4`:
it requires evidence-preserving cleanup after each build/test batch and lets
the separately authorized legacy-resource-cleanup outcome run independently.
The Design and Plan01 bytes and this capability-retirement scope are unchanged.

## Alternatives and decision

1. Retain both optional features indefinitely. This preserves every existing
   consumer but does not deliver the approved retirement.
2. Disable features or hide menus while retaining the implementations and
   plugin controllers. This leaves callable or advertised surfaces behind and
   is not the target state.
3. Remove framework APIs before changing consumers. This breaks EKO's CI,
   which checks out framework `main`, and ignores the SDK's independent pin.
4. First remove consumer dependencies, then physically retire only the two
   capabilities, with the full pre-retirement version recoverable. **Selected.**

The implementation outcomes are separate: consumer decoupling must be accepted
while the framework still provides the old APIs; framework removal follows.
New framework APIs retain the opposite order: framework first, consumers adopt
afterward. Every mainline merge must leave its then-supported consumers
buildable. No runtime code, feature, lockfile, generated contract, example or CI
matrix changes in this documentation outcome.

## Ownership and preserved capabilities

| Layer | Continues to own | Retirement responsibility |
| --- | --- | --- |
| Framework | Generic Agent/Session, revisioned task graph, Subagent, tools/results, cancellation/recovery, HumanLoop, plugin primitives and storage options | Remove the exact A2A/LSP implementations and public/export/registration surfaces after consumer decoupling |
| EKO application | GUI product strategy, workspace/configuration, IM and scheduler coordination, terminal lifecycle and plugin activation | Remove product dependencies and diagnostics/UI/configuration consumers; retain shared execution and background triggers |
| SDK | Protocol, ACP Host adapter, accepted source/kind identities, language projections and resource/stream adapters | Retire only accepted A2A/LSP identities/routes and their consumers; manage its own framework pin |
| Workspace / website | Cross-repository coordination, gitlinks and formal-source mirrors | Synchronize corresponding rules and truthful documentation at each accepted stage |

ACP, MCP, channels, topology, Task/Subagent, tool/result APIs and the framework's
reasonable tool/storage implementation menu remain. EKO continues file-based
persistence and must not enable SQLite; this does not authorize deleting
`echo-state`'s optional SQLite implementation or other reasonable public APIs.
Existing local-personal-product and direct-user-interaction versus automatic
Agent permission boundaries remain in force. No new permission gate, state
authority, task executor or store is introduced by retirement.

## Framework disposition inventory

Paths and symbols below were inspected at the framework baseline above.
Definitions, registration helpers and direct production consumers are listed
separately; source reachability does not constitute a live-service test.

| Disposition | Source and symbols | Boundary to close during implementation |
| --- | --- | --- |
| Retire A2A feature/facade | [Cargo.toml](../../Cargo.toml): `a2a`, `full`, docs.rs feature set; [src/lib.rs](../../src/lib.rs): `pub mod a2a` and prelude A2A re-exports | Remove feature requests and exports together; assess A2A-only `axum`/`jsonwebtoken` dependencies from actual users, not package names |
| Retire A2A client/server and values | `src/a2a/mod.rs`（退役前路径）: `A2AClient`, `A2AServer`, `serve`, `serve_with_auth`, JWT and value re-exports; sibling `client.rs`, `server.rs`, `serve.rs`, `auth.rs`, `types.rs` | Includes discovery, task send/get/cancel/stream, HTTP/SSE and authentication; `serve.rs` mounts `/.well-known/agent.json`, `/`, `/health`, `/ready`. Do not retain empty facades or callable controllers |
| Retire LSP feature/facade | [Cargo.toml](../../Cargo.toml): `lsp`, `full`, docs.rs set; [echo-core/Cargo.toml](../../echo-core/Cargo.toml): `lsp`; [echo-integration/Cargo.toml](../../echo-integration/Cargo.toml): `lsp`; [src/lib.rs](../../src/lib.rs), [echo-core/src/lib.rs](../../echo-core/src/lib.rs), [echo-integration/src/lib.rs](../../echo-integration/src/lib.rs): gated modules; `src/lsp.rs`（退役前路径）: public re-exports | Remove the whole exported capability, not only the root feature flag |
| Retire LSP types and process owner | `echo-core/src/lsp/mod.rs`（退役前路径）: `LspClient`, `LspError`, `LspResult`, protocol values; `echo-integration/src/lsp/mod.rs`（退役前路径）: `StdioLspClient`, `LspConfig`, `LspManager`; sibling `client.rs`, `config.rs`, `jsonrpc.rs`, `manager.rs` | Includes language-server configuration/discovery, requests/diagnostics, child lifecycle and shutdown; preserve unrelated MCP/ACP process owners |
| Retire five LSP tools | [src/tools/mod.rs](../../src/tools/mod.rs): `pub mod lsp`; `src/tools/lsp.rs`（退役前路径）: `LspDiagnosticsTool`, `LspGotoDefinitionTool`, `LspFindReferencesTool`, `LspHoverTool`, `LspStatusTool`, `register_lsp_tools`, `SharedLspManager` | Wire names are `lsp_diagnostics`, `lsp_goto_definition`, `lsp_find_references`, `lsp_hover`, `lsp_status`; `register_lsp_tools` is a helper, while EKO actually constructs the five tools itself |
| Retire only LSP plugin members | [echo-core/src/plugin/capability.rs](../../echo-core/src/plugin/capability.rs): `PluginCapability::LspServer` and parse aliases; [echo-core/src/plugin/registry.rs](../../echo-core/src/plugin/registry.rs): capability discovery and `ResolvedComponents::lsp_config_file`; [src/plugin/prepared.rs](../../src/plugin/prepared.rs): `PreparedPlugin::lsp_document`, `PluginWiringResult::lsp_discovered`, document freezing/wiring | These members are not isolated by the LSP feature. Remove their declarations and consumers explicitly; keep `PluginIntegrator`, registry/coordinator, other components and generation publication semantics |
| Retire dedicated learning consumers; preserve remaining examples | [echo-agent-learning/Cargo.toml](../../echo-agent-learning/Cargo.toml): `full`, `a2a`, `lsp`, `demo23_a2a`; `demo23_a2a.rs`（退役前路径）; [example_contracts.rs](../../echo-agent-learning/tests/example_contracts.rs) and its `demo55_lsp_tools contract module`（退役前路径） | Remove only dedicated retired examples/feature requests and their test modules. Retain ACP `demo72_acp_agent_adapter`, IM `demo38_im_channels`, scheduler `demo70_scheduler` and unrelated learning targets. The LSP demo is a feature-gated contract test at this baseline, not a standalone Cargo example target |
| Synchronize active documentation and matrices at removal | [README.md](../../README.md), [README.zh.md](../../README.zh.md), bilingual `docs/{en,zh}/README.md`, `architecture.md`, `lifecycles.md`, `31-lsp-integration.md`, `32-plugin-system.md`; [AGENTS.md](../../AGENTS.md), [scripts/verify.sh](../../scripts/verify.sh), [.github/workflows/rust-ci.yml](../../.github/workflows/rust-ci.yml) | Current feature lists and guides remain accurate for current code. Later remove active advertising and retired feature checks, keep retained-feature coverage and historical ADR/evidence truthful; synchronize website mirrors through their owner |

Earlier LSP fidelity decisions, including [ADR 0082](0082-lsp-response-value-fidelity.md),
describe the retained historical implementation; this retirement does not
rewrite their tests, findings or acceptance into a new claim of defect repair.
A2A findings likewise require their own disposition, not automatic closure.

## Consumer decoupling and merge conditions

Consumer paths below refer to their independent repositories at the recovery
commits in the next section. They are an implementation handoff, not files
owned or modified by this framework documentation contribution.

| Consumer | Actual source path / symbol | Obligation before framework removal |
| --- | --- | --- |
| EKO build and CI | `Cargo.toml`: framework features include `lsp`; `echo-agent-app-core/Cargo.toml`: `lsp`; `.github/workflows/rust-ci.yml`: framework checkout `ref: main` | Merge removal of feature requests and production references first. Revalidate the exact new EKO main against the still-capable framework and intended removal candidate |
| EKO production tool initialization | `echo-agent-app-core/src/runtime.rs`: Agent initialization calls `register_lsp_tools`; that function constructs `LspManager`, loads configuration, starts servers and registers all five tool classes | This is a real executable path, not an unused framework helper. Remove it without losing other Agent initialization, plugin, task/recovery, IM or scheduler wiring |
| EKO plugin/configuration lifecycle | `echo-agent-app-core/src/plugin_components.rs`: `PreparedApplicationComponents::lsp_configs`; `src/plugin_runtime/types.rs`: `PluginLspRuntime`, `PluginLspBinding`, `config_for_workspace`; `src/plugin_runtime/runtime.rs`: `prepare_lsp`, command/rebind/reload paths; `src/plugin_runtime/publication.rs`: `shutdown_published_plugin_lsp`; `src/extension_commands.rs`: LSP commands/receipts | Here `src/` is relative to `echo-agent-app-core/`. Remove only LSP members and test projections; preserve shared generation publication, rebind, rollback and cleanup owners. Old configuration/plugin requests need explicit retirement diagnostics, not silent success or deletion of user files |
| EKO GUI and old console consumers | `src/tauri/commands/lsp.rs`: `lsp_control`; `src/tauri/mod.rs`: invoke registration; `web-frontend/src/components/lsp/LspPanel.tsx`, `components/layout/SettingsDialog.tsx`, `api/endpoints.ts`, `stores/uiStore.ts`, generated command/receipt projections; `src/cli/extension_surface.rs`, `src/tui/commands.rs`, `src/tui/events.rs` | Retire UI, IPC and extension requests with their authoritative types, not only a menu item. Independent console retirement remains a separate outcome; shared IM/cron/terminal paths must stay alive |
| SDK Host build and production routing | `echo-sdk-host/Cargo.toml`: `framework-a2a`, `framework-lsp`, `sdk-facade-all`; `echo-sdk-host/src/core_profile/facade/integrations.rs`: `A2aClientRecord`, `LspManagerRecord`, `a2a`/`lsp` dispatch and resource registries | Remove feature requests, dispatch, resource/stream ownership and advertised capabilities consistently; preserve ACP Host stdio, MCP, topology and other families |
| SDK accepted identities and projections | `contracts/sdk/accepted-external-contract.json`: `path`/`kind`/`source_paths`; `contracts/sdk/accepted-facade-operation-catalog.json`: families, routes and signatures; `echo-sdk-protocol/src/facade.rs`; `sdks/{typescript,python,java}` | SDK owner identifies exact accepted identities from A2A/LSP source paths, including aliases, LSP tools and their shared bridge references. Retire corresponding language wrappers/values and tests, not the entire shared bridge or full Rust inventory |

The accepted SDK catalog has `a2a` and `lsp` families with legacy methods
`_echo_agent/a2a/op` and `_echo_agent/lsp/op`, source-routed
`_echo_agent/facade/invoke` operations, and `value:a2a` / `value:lsp` routes.
A2A binds `echo_agent::a2a`; LSP binds `echo_core::lsp`,
`echo_integration::lsp` and `echo_agent::tools::lsp`. These are selectors for
the SDK owner's exact source/kind review, not permission to delete unrelated
accepted entries that mention a shared type. Whole-Rust inventory remains
non-blocking telemetry, not an automatically accepted three-language contract.

At the inspected EKO baseline, the production Rust search found LSP consumers
above and no A2A reference under `src/` or `echo-agent-app-core/src/`.
That bounded negative result is not a claim that framework A2A is dead: the SDK
Host dispatch calls `A2AClient` and the learning example uses client/server APIs.

Required implementation acceptance:

- Consumer decoupling is tested and merged before framework removal. The old
  framework APIs stay intact until that prerequisite is accepted. A pin-stable
  SDK may first retire its two contracts on its original framework revision;
  a later pin upgrade is a separate, verified decision, not an implicit gitlink
  update. No consumer build may keep requesting a removed Cargo feature.
- Framework removal must pass its updated applicable fmt, Clippy, tests,
  featureless/retained-feature checks and executable-example gates, plus the
  exact merged-main CI. Removing A2A/LSP cases does not excuse dropping ACP,
  MCP, channels, storage or other retained coverage.
- EKO must test GUI startup and plugin configuration, retained Task/Subagent
  and recovery, IM send/receive, cron and embedded terminal behavior. Console
  removal cannot delete `src/cli/channels*` or desktop/shared services merely
  because of their directory name. Native acceptance remains separate from
  Rust compilation and mock tests.
- SDK must verify its independently pinned Host, accepted source/kind and route
  closure, advertisement, three-language builds and remaining contract tests.
  Dedicated A2A/LSP fixtures/tests retire with their contracts; shared protocol,
  resource/stream cleanup and other facade-family tests remain.
- User conversation, task, memory, plugin and configuration files are retained.
  Stale retired requests receive explicit diagnostics; no inert controllers or
  fabricated successful loads remain on the final mainline.
- Active formal docs, examples, CI/feature matrices and website mirrors must
  describe the accepted implementation at each stage. This ADR alone does not
  authorize claiming that runtime retirement or publication is complete.

## Recovery and independent revision identities

The annotated tag `pre-retirement-2026-10-24` and branch
`chore/Echoyue/full-capabilities-baseline` resolve locally to the following
commits in each independent repository. Both refs and the tag object type were
read-only checked on 2026-10-24; remote availability and later publication are
separate delivery checks. Do not move or delete these recovery refs.

| Repository | Full-capability commit |
| --- | --- |
| eko-workspace | `9a96ff9675c340c9d6ef18d63dd2a2b16663962e` |
| echo-agent | `51726454985fd6f064b21a7e9c539e22d494c987` |
| echo-agent-cli | `c262cfed8e77abed38a27c1fff93547997ec9486` |
| echo-agent-sdk | `0930eef9f878cf436ed88384030b6bc03d6f6c53` |
| echo-website | `a926ef7cf10bdc03cc0a19b2e406cc6164477485` |

The workspace commit's four gitlinks were checked against this table.
Separately, SDK `echo-sdk-host/Cargo.toml` pins framework
`f7bac62eee04a963f92a3e4308b6ea2336e91cc0` with `default-features = false`.
The workspace framework gitlink `5172645` does not replace that Host build pin.
Recovery starts a newly authorized task from the preserved refs and evaluates
what to port onto then-current main; it does not reset main or overwrite user
data. Preserved history is not a promise to maintain a compatibility branch.

Exa is independently retained in the framework's local
`feature/Echoyue/exa-web-search` commit
`4343f0b736beb5c8606e31146528a2fe8be80510`. It is not part of this approved
retirement change and must not be pushed or merged into main without new
authorization. Existing worktrees, stashes and historical evidence are not
cleanup targets of this decision.

## Build-batch evidence preservation and cleanup

The user's additional instruction on 2026-10-24 applies to this work and its
implementation follow-ups. It overrides the former rule that kept `target/`
by default and cleaned only at disk/cache thresholds. The framework AGENTS
now carries the same mandatory batch cleanup, concurrency and disk limits.
The approved Delivery Map revision above includes this execution constraint;
the Design and Plan01 inputs remain unchanged.

- After each compilation/test batch actually finishes, preserve the necessary
  logs, real exit status and any Host binary or artifact needed for later
  validation **outside `target/` before cleanup**. Do not delete first and
  subsequently claim the evidence or artifact is still recoverable.
- Confirm that the exact target belongs to this task and has no active
  `cargo`, `rustc`, Host or other artifact reader. Then promptly run
  `cargo clean` for that exact owned target. If it is still in use, defer
  cleanup until the users have finished; do not remove files underneath them.
  Keep worktree targets independent and do not clean another chat's target or
  the primary checkout's cache.
- Run at most two focused Rust builds concurrently; serialize complete build
  and test matrices. Use at most two Cargo jobs per build and disable dev/test
  debug information. Record `df`/`du` disk measurements before and after each
  build batch. Below **5 GiB** available disk space, stop starting new heavy
  builds and let the coordinator resolve the resource constraint.
- For documentation-only or read-only work with no compilation/test batch and
  no task-owned Cargo output, report cleanup as not applicable; do not build
  merely to create something to clean, or clean unrelated directories.

This documentation contribution ran no Cargo compilation or tests and created
no task-owned Cargo output, so no `cargo clean` applies here. The coordinator
owns the separately authorized untracked/uncommitted-file, branch and worktree
maintenance; this clause does not delegate deletion of source, stashes,
recovery refs, worktrees or original evidence to individual contributors.

## Documentation contribution and coordination seam

This contribution updates framework AGENTS and adds this ADR. Runtime code, manifests/locks, SDK
accepted JSON and generated output, examples, feature matrices and recovery
refs are unchanged; current guides continue to describe the still-present
implementation. No Rust full matrix or live A2A/LSP service validation is
claimed for a documentation-only contribution. Formal English/Chinese guides
need active-capability updates with actual retirement, not speculative deletion
now. The website owner must mirror applicable formal-source changes through
its existing synchronization mechanism; this framework task does not edit the
website or assert that its deployment changed.

The framework rules now express the clauses also required in workspace rules:
the A2A/LSP-only approved exception; preserve all other reasonable framework
public APIs; distinguish EKO GUI/IM/cron policy from reusable framework and
SDK Host capabilities; consumers first for API withdrawal, framework first for
new API delivery; SDK pin independent of workspace gitlinks; preserve recovery
refs and the separate Exa restriction; preserve evidence outside the target and
promptly clean each finished build batch's idle, task-owned target under the
concurrency and disk limits above. Rule synchronization and cross-repository
acceptance remain required before the shared product-boundary-contract outcome
can be called complete. This ADR does not independently complete that Plan.

## Consumer decoupling: frozen build diagnostics

The original documentation contribution above did not remove runtime code.
Plan04 `plan:b6ef1ac6-0745-438d-a3ff-75bb8cdbbcc5`, SHA-256
`336f74dfd00de9b75f84d7101cb9d419f462a6a300bbff416db500f7d77bb070`,
adds a narrow follow-up from framework
`c3b56b4ee8a73565e6a2a621b9a7ddfb644ed3aa`: expose unsupported-build LSP
declarations through the already public generic preparation diagnostics.
No new public type, field, accessor, decoder or state authority is added.

Only the no-LSP build path changes. Successful UTF-8 document freezing keeps
the existing substitution/hash and adds a component/plugin/path Warning that
the build cannot apply LSP. It does not validate YAML or contain EKO product
policy. Invalid UTF-8/read failures stay Error. Wrong filesystem kinds,
metadata errors and broken links must not disappear into generic component
strings or optional NotFound. True absence is not an error, and known
non-regular files are never read as text. Healthy retained components remain
independent and a component error does not invalidate the whole set.

The original prepare/cache owner captures the diagnostic before constructing
the immutable set. Cached sets are returned unchanged; consumers, wiring and
rollback do not probe the package again. The existing feature-on LSP behavior,
document/hash identity inputs and other components remain. Core resolver
public data shapes and generic non-LSP inspection rules are unchanged.

This bridge lets EKO consume `PreparedPluginSet::diagnostics()` instead of
`lsp_document()` or other LSP-specific metadata. Error diagnostics cannot be
recovered from the Warning-only apply-receipt projection, so applications must
consume the complete collection. EKO owns its retirement wording; framework
messages only describe the build capability. EKO switching and acceptance
follow this framework contribution, and physical framework deletion remains
later. The SDK's original framework pin and exact 269/23 retirement boundary
are not changed by the bridge.

Focused red/green, feature-on controls, independent review and normal semantic
validation are required for this contribution. Runtime evidence is recorded
when actually executed; this record does not announce Plan04, both consumers,
the full matrix, main CI or publication complete. Existing examples need no
new API migration because the public preparation interface is unchanged;
website source mirroring belongs to the coordinator.

Local focused evidence for this bridge: the six old-API regression tests first
produced four behavioral failures and two passing controls on the unchanged
production base. The final candidate passed eight default regressions, all 28
prepared-module tests with `mcp`, and all 22 with `mcp,lsp` (58 executions,
zero failures). Both corresponding package-lib Clippy runs with `-D warnings`
exited 0. These use real temporary filesystem inputs, a mock LLM and the
existing local/mock MCP controls; they do not exercise a real language server
or downstream EKO/SDK acceptance. An initial candidate test failure came from
an empty Hook fixture and was corrected to the existing active-rule fixture
without weakening the assertion or changing production behavior. Raw results
and both cleanup receipts remain workspace-only in
`.echo-semantic/local/lsp-frozen-diagnostics/`, outside the cleaned target.


## 影响

2026-10-28，本仓唯一writer已在Root实际交还epoch18读窗后采用完整30源码，18个专属
文件删除、12处manifest/export/插件准备接缝修改；没有重复采用拆分包。当前A2A/LSP
能力面、公开出口和专属示例撤销，保留ACP、MCP、channels、Task/Subagent、合理Store、
原生消息及测试profile。本文较早的Status和构建桥接段落是历史事实，不再代表当前实现。

只检查原插件根`lsp.yaml`：合法旧声明保原文/变量替换/hash/cache/invalidate并给明确
退役诊断，错误保原component/plugin/path/severity；不声称扫描项目级`.lsp.yaml`。
旧声明只在同generation私有冻结容器保全，不恢复公开LSP getter、执行器或成功shim。

同日原`cargo metadata --locked --all-features --format-version 1`已实际自然0且stderr空，
解析前后manifest与锁SHA相同，完整图676包、公开feature25项。锁最初变化只记疑似IDE
自动reload、未捕获其原命令/出口，不能追认本线生成；该显式metadata票只证明当前锁与
当前manifests一致，不是Cargo check/test/rustc、类型检查或49组依赖风险合同已通过。
没有手改版本/来源、baseline、ignore或排除，zstd/vendor/testprofile保护保持原映射。

后继验证须在Framework、SDK薄投影与EKO源码稳定后，由Root一次排相邻focused和适用
完整矩阵；当前文档、静态检查和metadata不授全source、旧402、SDK正式Gitpin、native、
main CI或发布资格。官网由所属writer从真实来源同步；原固定tag/分支/Exa与用户数据保留。
