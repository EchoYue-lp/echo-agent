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
