# ADR 0098: Location-bound file I/O and execution resources

## Status and scope

Accepted implementation direction, 2026-10-25. Focused Rust qualification has
passed for the library/feature scope below; full Rust, consumer, native GUI,
real remote and platform qualification remain pending. The two modified offline
examples are not yet qualified. Earlier qualification of parent `939b672` does
not qualify these changes.
This decision implements the workspace project-workspaces Design/Plan; the
framework owns the reusable mechanism below, not EKO's location or SSH policy.

## Context

The standard file tools performed native filesystem operations after resolving
paths. A remote-looking path, a shell executor, or an open SSH terminal could
not change those reads, searches, writes, preimages or completion evidence into
remote operations. Each workspace host already has an independent ToolManager;
a new global current-project field or another tool catalogue is unnecessary.

Existing rules and Skills also read files before model/tool invocation. A Skill
can be discovered in one target and activated later: its descriptor's path text
alone cannot select its file or command source. The original physical command
cell owner must likewise retain its invocation's executor/resources after a
foreground observer stops waiting.

## Decision

1. `echo_core::tools::filesystem::FileSystem` supplies synchronous filesystem
   operations and portable metadata/entries in one immutable target namespace.
   Paths are namespace tokens, never an SSH address or proof of local existence.
   `echo_tools::filesystem::LocalFileSystem` retains native behavior and the
   original optional Git checkpoint implementation. The default stays local.
2. `StandardToolPack::with_file_system` and `ReactAgentBuilder::file_system`
   inject one Arc into the original file tools. The original schemas, budgets,
   parsers, effect reporting and per-call working directory remain authoritative.
   Git's private registration adapter delegates the original tool algorithms
   and schemas while consuming the same provider and original SandboxExecutor;
   public local Git tool constructors remain available. `list_worktrees` shares
   its original porcelain parser and reads markers through the selected Git
   executor/filesystem. Local captured-artifact roots are not remote grep scopes.
   Managed worktree mutation retains its original checkout/admin-directory
   guards, marker publication, caller-drop owner and exact compensation. The
   local `DirectoryIdentityGuard` wraps the original no-follow directory FD and
   stable identity verification. Current non-native enter/exit calls are
   unavailable **before reservation, merge or removal**, and say so in the tool
   description. They do not execute the native algorithm on a remote path.
3. Existing blocking operations retain invocation resource guards until physical
   completion. An async transport can bridge its existing runtime from that
   boundary. No additional runtime, retry executor, store, or task owner is added.
   `ReactError::Io` retains actual local error kinds; remote failures retain the
   existing `ToolError::RemoteFailure` / `ToolFailure` recovery facts.
4. The existing apply_patch parser, preimages, atomic replacement and definite
   failure rollback run against that provider. A possibly completed write is
   not a definite failure: report the typed uncertainty and attempted paths,
   without replay or speculative rollback. Checkpoints share one original
   format/index/permission algorithm through the filesystem and existing
   structured-command callback; an unavailable Git environment is not non-Git.
5. Existing InstructionResolver exposes a provider-aware fallible entry with the
   same precedence algorithm. Applications owning an InstructionProvider retain
   that owner and prepare through their existing blocking boundary. They disable
   the framework's automatic **local** rules loader after successful provider
   preparation; file-tool injection alone does not replace pre-model context.
6. The original SkillLoader/registry retain each Skill's file and execution
   source across discovery, activation, resource reads, inline commands and
   scripts. `discover_skills_with_sources` also accepts the caller's existing
   local executor for User scopes; it passes the Arc through the original loader
   into each source without creating an executor. Hosts can instead set the
   original Agent assembly's `set_default_skill_source` configuration once;
   ordinary descriptor/prepared registration captures that immutable source,
   even after the host execution backend changes. This value owns no invocation
   leases or I/O operations. Project and user/global
   sources stay distinct, even for identical path strings. The original parsers,
   policy, activation claims and registry
   remain authoritative; missing target execution never falls back to local.
   Existing HookRegistry entries also capture each Skill's executor and source
   directory. `bind_source_executor` permits hosts to bind existing UserConfig
   or Plugin entries explicitly. Mixed UserConfig definitions use ordered
   origin groups in the same existing source slot via
   `register_user_hooks_with_sources`; each event retains the original append
   order, and clear/reload replaces that source as a unit. Descriptor/prepared
   registration also exposes `with_source` entries for consumers that do not
   use discovery. Later host-wide executor replacement does not move a bound
   source; unbound public registrations keep their legacy default.
   Target commands do not inherit the client's PATH/TMPDIR or use native exists
   to decide whether their working directory is usable.
7. CommandCellRequest carries its actual executor and original resource guards.
   Runtime-only `SandboxCommand.resource_guards` and `cancel` are skipped by
   serde; backends transfer guards into their existing physical operation before
   spawning. The original cell supervisor requests cooperative stop and awaits
   the same backend/drain. After the existing five-second cleanup grace it
   reports an unsettled observation while retaining the active owner; finite
   wait calls keep the UI responsive. Dropping a future is not a cancellation
   receipt. Local cancellation reuses its original controlled child cleanup.

`run_code` validates persisted scripts against the same target filesystem. Its
existing minimum OsSandbox requirement and actual isolation receipt remain;
SSH alone does not establish OS isolation. Captured tool-output artifacts and
product persistence remain under their original local owners. Workspace-output
validation belongs to its real source, not a same-name local path or a model's
self-reported hash. No new Subagent registration or result-verification API is
introduced for parsing, which already clears model-claimed artifact authority.

## Alternatives and references

A parallel `remote_read`/`remote_patch` tool set duplicates schemas and policy.
Changing only path resolution leaves actual native I/O unchanged. Mirroring a
whole project invents synchronization and identity rules. Treating every host as
one global ToolManager invents unnecessary context fields. These alternatives
are rejected in favor of adapting the existing I/O and execution boundaries.

[VS Code Remote SSH](https://code.visualstudio.com/docs/remote/ssh) associates
files and execution with a selected workspace target. [TRAMP remote processes](https://www.gnu.org/software/emacs/manual/html_node/tramp/Remote-processes.html)
associate process execution with the remote default directory. This framework
uses the shared location-adapter principle without adopting an IDE server.
The application's standard SSH/SFTP adapter owns authentication and connection
reuse; SFTP cancellation does not recall an already-sent write.

## Consequences and boundaries

FileSystem is a Rust extension surface. Its transport, credentials, workspace
revision and connection UI are application-owned. Metadata describes the target;
unsupported primitives fail explicitly. `open_read` returns a standard bounded
reader, without first loading the file into a Vec; evidence consumers retain
their original fixed-buffer hash and size algorithm. The default is Unsupported,
not a whole-file emulation. Provider readers retain their actual operation until
physical read/close settlement, including early observer departure.

The current standard SFTP interface exposes length, ownership, mode and timestamps,
but no stable directory identity comparable across a held handle and a no-follow
path reopen. Neither `realpath`, mode/mtime nor `statvfs` establishes the original
FD guarantee. Its default `retain_directory` is Unsupported. This is a concrete
framework public-menu limit, not remote-worktree acceptance; EKO's separate
existing TaskRuntime isolation path cannot qualify these tools by substitution.
Default local worktree tests and target list/mutation-refusal controls must pass
before qualification. No remote RPC, new cleanup owner or altered marker format
is added to manufacture capability.

The optional compare-and-swap primitive
reuses the original same-sidecar cooperating-writer lock; it does not fence
external editors or add an ABA/mtime contract. An error can occur after the
replacement, so callers cannot infer that bytes stayed unchanged. A remote
adapter without that exact primitive reports Unsupported. The currently developed client/remote
platform combination must be tested before claiming support for other systems.
Existing local, Docker and Kubernetes public options are retained.

New Rust API inventory and runtime-only fields do not automatically become SDK
accepted identities. SDK pins, accepted sets and language protocols are not
changed by this decision. Serde skip describes intended runtime-only transport;
actual collector/Host evidence remains separate from source inspection.

## Validation

Meaningful regression sources exercise the original tool pack with two distinct
namespaces sharing identical visible paths: read/list/search/diff/repo-map,
Unicode patching, actual checkout selection, stale preimages, definite rollback,
disconnection and an applied write with a lost response. Source-bound Skills
exercise discovery through activation/resource/script consumers, malformed
arguments before probes, unavailable executors and resource lifetime. Hook regressions invoke same-named source entries in
independent hosts, replace the host executor, and require captured executors/cwd
without local fallback when a required executor is absent. Mixed UserConfig
controls also require original append order, one source identity/count, and
complete removal on clear. Worktree
controls require actual target Git/list/marker I/O, disconnection without a
same-name native marker fallback, zero commands/reservations for unsupported
mutations, and detection of a native directory replaced at the same path. Bounded
reader controls consume non-ASCII target bytes through small buffers. The cell
regression observes a stop request while the original backend is held, then
releases that backend before expecting a terminal receipt and guard release.

The focused 2026-10-25 batch ran the Core (434), Execution (352), Orchestration
(532), Tools with files/git/shell (164), and root with
files/git/shell/project-rules/subagent (1,112) library tests successfully. Final
changes to Execution/Orchestration/Tools were retested together. Matching old
Skill production code failed the two intended behavior controls (7 passed,
2 failed); restoring the implementation passed the same nine controls. The
related five-crate all-targets Clippy check with warnings denied, independent
featureless/files/git/shell checks, and workspace formatting passed.

This is focused evidence, not the complete feature/platform matrix. The first
batch stopped Example 08 compilation at its fixed deadline and did not start
Example 09; those original outcomes remain preserved. A separately admitted
continuation then actually ran the existing examples with `--source-only` and
`--file-system-only`; both exited naturally with code 0 and no warnings on the
same Rust source. These offline controls cover source-bound discovery and the
provider-backed Unicode file read, without model or remote calls. The first
batch precisely cleaned its own target (5,165 files, 2.1 GiB; natural exit 0),
and the continuation did the same (2,217 files, 1.0 GiB; natural exit 0); neither
kept a warm target for review. Full matrix, dependency/semantic gates and
consumer acceptance remain required.
EKO additionally owns real GUI/SSH and same-name two-machine checks; fake
providers or a terminal login cannot establish that acceptance. The website
must mirror the finally qualified framework source, not invent a source pin for
this uncommitted candidate.

## 2026-10-30：read-before-edit沿实际owned工具提供路径事实

独立源码复核发现两个相邻缺口：成功target read effect被host canonicalize重新解释，
以及snapshot cwd与合法工具with_base_dir优先级不同。只给snapshot加provider getter
仍会在cwd=A、tool base=B时核错目标；不能删除base_dir能力或用host同名路径补证。

选择在原Tool trait提供一项同步default方法resolve_file_tracking_path，接该次最终
parameters／ToolContext，返回原FileSystem Arc及其namespace内路径。缺能力默认明确
Unsupported；既有文件工具只调用自身原resolve_path，ApplyPatch沿原root／action
resolver，不建立第二路径validator或来源表。调用在原blocking模式保留资源guard并
等待真实Join，map锁不跨await。该通用身份机制不依赖EKO的SSH／项目UI策略。

成功FileRead effect路径是已经报告的事实：ReadFileTool只把它绑定自身provider，不再
事后host／target canonicalize改写。原read-map扩为路径对应source／时间观察，未知
source只保诊断而不能授权。native按paths_are_native明确声明同一进程namespace，
non-native仅认同一保留Arc。30分钟TTL与每次prepare清空保持原规则，不新增registry。

公开record_file_read显式接source，这是Rust签名变化；FileReadHistory是原map值的
非序列化Rust类型别名，不是新wire DTO。ToolContext原16个accepted字段不增，三语言
既有FileRead／FileEdit String path和ToolResult effects保留。flag关闭不调用新hook，
成功effect即使来源证明缺失也不被抹掉或改成工具失败；guard开启时没有正确write
proof须依原Unavailable工具拒绝通道返回，不能猜host／snapshot或新增自动重试。
真实provider I/O错误保typed原首因，不泛吞成“未读”。

验证包括host／target双向alias、source相同路径隔离、base_dir A/B正反控制、相对／
绝对cwd、provider失败不落host、flag关闭和默认Unsupported的custom effects保真，
以及原TTL、资源与Join；真实SSH／SDK／native资格单列。本段是当前方案说明，不
以源码或结构校验替代同源实测与消费者复审，也不自动把新trait方法接受为语言路由。

## 目录范围合同补充（2026-10-30）

FileSystem 的不可变 namespace 决定读取哪一个目标；工具的显式目录根决定该目标内哪些路径属于本次读取范围，两者不能互相替代。Glob、RepoMap、CodeSearch 的 construction-time `with_base_dir` 继续作为上界；runtime `working_dir` 只作相对路径基准。Grep 保留原 base、working_dir 与 native artifact-root 的允许根集合，non-native provider 不纳入本机 artifact 根；没有任何根时保留无约束读取。

入口及递归实际读取目录项均使用被选 provider 的 canonicalize，复用现有路径 resolver 与 Grep 的原根集合，不新增 validator、权限模式或注册表。根内链接和同根 alias 继续有效；明确越界时整次调用返回范围错误，不返回已累积的部分成功结果。Glob、RepoMap 保留原逻辑路径用于 matcher、显示与分页身份。该检查沿用现有 pathname API，不声称能够原子防止检查后由外部并发替换 symlink；没有新增文件描述符身份或写入机制。

路径校验必须解析实际 I/O 的原路径，不在 provider canonicalize 前折叠 `symlink/..`；已展开的递归目录项不再次拼接相对 base。缺失写目标沿同 provider 解析已有祖先再重建后缀；普通无显式根仍保原 best-effort 行为。保留原忽略构建目录和悬空条目跳过规则，typed provider 故障继续传播。

验收覆盖原工具API的parent path、根/child链接、合法alias、相对base、symlink/..、Grep多根及目标namespace反例，并与旧生产代码使用同一断言；既有full/Glob matcher通过票不能替代目录范围验证。不新增公开API或产品配置，现成示例及官网工具用法保持，采用时由原作者核对对应说明。
