---
schema_version: 1
kind: evidence
observed_at: source:34d5f5574c9cf5a645970cd8b58367c39bce268630488b68d3ea54e996728e17
source_refs:
- echo-core/src/guard/content.rs
- echo-execution/src/tools.rs
- echo-integration/src/channels/manager.rs
- echo-integration/src/channels/mod.rs
- echo-integration/src/lsp/client.rs
- echo-integration/src/lsp/config.rs
- echo-integration/src/lsp/jsonrpc.rs
- echo-integration/src/lsp/manager.rs
- echo-integration/src/lsp/mod.rs
- echo-orchestration/src/tasks/background_state.rs
- echo-orchestration/src/tasks/background_task.rs
- echo-orchestration/src/tasks/command_cell.rs
- echo-orchestration/src/tasks/events.rs
- echo-orchestration/src/tasks/mod.rs
- echo-orchestration/src/tasks/progress.rs
- echo-orchestration/src/tasks/revisioned.rs
- echo-orchestration/src/tasks/runtime.rs
- echo-orchestration/src/tasks/runtime_executor.rs
- echo-orchestration/src/tasks/runtime_service.rs
- echo-orchestration/src/tasks/task_tools.rs
- echo-orchestration/src/tasks/time.rs
- echo-tools/src/data.rs
- src/acp/adapter.rs
- src/acp/extension.rs
- src/acp/mod.rs
- src/acp/projection.rs
- src/acp/prompt.rs
- src/acp/runtime.rs
- src/acp/session.rs
- docs/adr/0060-task-execution-store-cas-and-local-interruption.md
- docs/adr/0061-acp-recovered-turn-identity.md
- docs/adr/0082-lsp-response-value-fidelity.md
- docs/adr/0083-shared-tool-input-validation.md
- docs/adr/0084-channel-owned-operations.md
- docs/supreme/specs/2026-10-13T1430-channel-owned-operations/plans/plan_01_闭合ChannelOwned操作.md
supports:
- behavior.task-subagent-execution
- rule.task-subagent-authority
- behavior.protocol-projection
- rule.protocol-role-separation
- behavior.effect-permission-execution
- rule.permission-effect-order
- behavior.extension-publication
- rule.extension-generation-authority
limitations:
- Current exact-source local framework engineering gates only; published pin, SDK languages/Host, CLI/website,
  remote CI and exact main require separate proof.
- Three existing external-environment opt-in ignored tests were not executed.
- Full Rust public inventory is telemetry, not automatically an accepted three-language API contract; user
  Registry publication remains excluded.
- Plan stays ready until its SDK and delivery acceptance is fulfilled; no unrelated Finding or broad Behavior/Rule
  is closed.
id: evidence.framework-final-34d5-full-verification
manifest_path: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/manifest.json
manifest_sha256: 883f0b8ad393c48c401a5557edeac896e07240465d6ae40c010abb6bef4ce69e
recoverable_source_revision: de2effeb9507ca93e8a5cfcd8cf822b9a199b9e8
command_results:
- label: format
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - fmt
  - --all
  startedAt: '2026-10-13T09:33:38.902Z'
  finishedAt: '2026-10-13T09:33:41.520Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/format-1.log
  logSha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/format-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: format-check
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - fmt
  - --all
  - --
  - --check
  startedAt: '2026-10-13T09:33:41.524Z'
  finishedAt: '2026-10-13T09:33:43.987Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/format-check-1.log
  logSha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/format-check-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: clippy-all
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - clippy
  - --workspace
  - --all-targets
  - --all-features
  - --locked
  - --
  - -D
  - warnings
  startedAt: '2026-10-13T09:33:43.989Z'
  finishedAt: '2026-10-13T09:36:11.453Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/clippy-all-1.log
  logSha256: f96b0a8d0637ecc2cd0201a50b62feaddfaa56bbcf4723e77f957f4a27173228
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/clippy-all-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: clippy-production
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - clippy
  - --workspace
  - --lib
  - --bins
  - --all-features
  - --locked
  - --
  - -D
  - clippy::unwrap_used
  - -D
  - clippy::expect_used
  - -D
  - clippy::panic
  - -D
  - clippy::unreachable
  startedAt: '2026-10-13T09:36:11.455Z'
  finishedAt: '2026-10-13T09:36:39.747Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/clippy-production-1.log
  logSha256: 7eb8b5d97441147f7c1020b229eed2d85eb19365b0078b5a650637a7ea43f818
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/clippy-production-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: test-all
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - test
  - --workspace
  - --all-targets
  - --all-features
  - --locked
  startedAt: '2026-10-13T09:36:39.748Z'
  finishedAt: '2026-10-13T09:43:06.282Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/test-all-1.log
  logSha256: 28734a86ac09557758a46a7e22cdc01fea4050509fc6727a87ce0f5d3edf7f6f
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/test-all-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: no-default
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - --workspace
  - --lib
  - --no-default-features
  - --locked
  startedAt: '2026-10-13T09:43:06.286Z'
  finishedAt: '2026-10-13T09:43:32.618Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/no-default-1.log
  logSha256: d3a8461fe1a6b5e0e480a402b86d67e4e3b29e756394675eed283e26507ce06a
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/no-default-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-acp
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - acp
  - --locked
  startedAt: '2026-10-13T09:43:32.620Z'
  finishedAt: '2026-10-13T09:43:55.145Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-acp-1.log
  logSha256: 60b15d8a336464c050129d9c37d0736964638c3f6dcdb06a00491dc9d6177503
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-acp-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-a2a
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - a2a
  - --locked
  startedAt: '2026-10-13T09:43:55.150Z'
  finishedAt: '2026-10-13T09:44:16.445Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-a2a-1.log
  logSha256: 444bb858f20d01d7857fc1735bc9250e323a654dbe181840ecee0cee7c88d09f
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-a2a-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-mcp
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - mcp
  - --locked
  startedAt: '2026-10-13T09:44:16.447Z'
  finishedAt: '2026-10-13T09:44:32.081Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-mcp-1.log
  logSha256: c8259dd545c9776175edd7b3ba353cb37f883deb2635bc912caae7c63fa2cbe8
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-mcp-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-lsp
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - lsp
  - --locked
  startedAt: '2026-10-13T09:44:32.085Z'
  finishedAt: '2026-10-13T09:44:47.853Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-lsp-1.log
  logSha256: 3c6aebd3cef90cd0cd95a1b724cff81560e3f477c2373313c4e9575b6a6d297f
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-lsp-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-sqlite
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - sqlite
  - --locked
  startedAt: '2026-10-13T09:44:47.874Z'
  finishedAt: '2026-10-13T09:45:03.180Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-sqlite-1.log
  logSha256: d0725a43bfabefe80561d3d8a0082ecc5bf9c6586e0d3f6ecf43e1383e6a53d6
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-sqlite-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-telemetry
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - telemetry
  - --locked
  startedAt: '2026-10-13T09:45:03.182Z'
  finishedAt: '2026-10-13T09:45:19.218Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-telemetry-1.log
  logSha256: 37f4d87fe211f08ea2eb0b5222b49869e62e392e1e31fd4a73a838ced84a02d0
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-telemetry-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-topology
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - topology
  - --locked
  startedAt: '2026-10-13T09:45:19.223Z'
  finishedAt: '2026-10-13T09:45:26.578Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-topology-1.log
  logSha256: fd865427942bcdf3b6f76ad0de82e70494997d5ee6cfec0ed0a29890a2fc9ba5
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-topology-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-subagent
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - subagent
  - --locked
  startedAt: '2026-10-13T09:45:26.579Z'
  finishedAt: '2026-10-13T09:45:35.455Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-subagent-1.log
  logSha256: f5b21b499672f58503abf6917e84d372c314649d30d3b4fb9237f0a08dc2c875
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-subagent-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-web
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - web
  - --locked
  startedAt: '2026-10-13T09:45:35.457Z'
  finishedAt: '2026-10-13T09:45:51.392Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-web-1.log
  logSha256: 2b6c9433f5c1cbc2c006aa2a48a5511a3f75471e9ef4ffe1e0342fdaebd6db40
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-web-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-media
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - media
  - --locked
  startedAt: '2026-10-13T09:45:51.393Z'
  finishedAt: '2026-10-13T09:46:09.961Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-media-1.log
  logSha256: 4f5f55fc75bb772ba3bc9db4bdd4c34f6068d5395831b633d7d50d37288144e0
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-media-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-data
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - data
  - --locked
  startedAt: '2026-10-13T09:46:09.963Z'
  finishedAt: '2026-10-13T09:46:46.557Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-data-1.log
  logSha256: 34f2dfad1366f16f85e1301fe162fd4379f57300b006e5a883afd9cf8df93164
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-data-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-statistics
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - statistics
  - --locked
  startedAt: '2026-10-13T09:46:46.560Z'
  finishedAt: '2026-10-13T09:46:55.324Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-statistics-1.log
  logSha256: 2fed92b158c3fe3913ed6eb5007c3524f9f5addaab3c29787b6876770c07ffc6
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-statistics-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-channels
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - channels
  - --locked
  startedAt: '2026-10-13T09:46:55.325Z'
  finishedAt: '2026-10-13T09:47:11.995Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-channels-1.log
  logSha256: f0ba1c7c1ffad94cbc8f00747d65857cc54e3c52f0db6ffdd8d1b77509869b1b
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-channels-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-git
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - git
  - --locked
  startedAt: '2026-10-13T09:47:11.997Z'
  finishedAt: '2026-10-13T09:47:20.222Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-git-1.log
  logSha256: aba0dce713ecc67749b7f5a0feb0946a21563a3d50c6e94f60b7d21720f33f7d
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-git-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-database
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - database
  - --locked
  startedAt: '2026-10-13T09:47:20.225Z'
  finishedAt: '2026-10-13T09:47:49.931Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-database-1.log
  logSha256: 49518da85aadf74f7afa6acdd1171ba3534fa9c7897841a4cb3ea7f6af1e276c
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-database-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-rag
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - rag
  - --locked
  startedAt: '2026-10-13T09:47:49.933Z'
  finishedAt: '2026-10-13T09:47:57.653Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-rag-1.log
  logSha256: b93de13508ec7a010a40771fd6790d1eb85edd9da84a57f45dcc39e7b904119c
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-rag-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: feature-chart
  attempt: 1
  command:
  - env
  - CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/echo-agent-sdk-worktrees/sdk-final-framework-pin/target
  - CARGO_BUILD_JOBS=2
  - CARGO_PROFILE_DEV_DEBUG=0
  - CARGO_PROFILE_TEST_DEBUG=0
  - CARGO_INCREMENTAL=0
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - chart
  - --locked
  startedAt: '2026-10-13T09:47:57.655Z'
  finishedAt: '2026-10-13T09:48:05.069Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-chart-1.log
  logSha256: ccab6061bf21fae2d8b32112c2fa4071bbeeb8af4d85098d4549ecd419e2b627
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/feature-chart-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
test_summary:
  passed: 3471
  failed: 0
  ignored: 3
  targets: 86
  examples: 59
source_unchanged: true
index_unchanged: true
---


# 当前34d5框架候选：完整工程门禁

## 支持的结论

23项实际命令均退出0、compilerWarnings0、sourceUnchanged true；850个源码文件逐项核验初始、最终和当前SHA一致，index初末一致。唯一校验器重算source摘要34d5，与内部可恢复Git de2effeb9507ca93e8a5cfcd8cf822b9a199b9e8 完全相同。

## 来源与范围

/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-final-gates/manifest.json（SHA 883f0b8ad393c48c401a5557edeac896e07240465d6ae40c010abb6bef4ce69e）；运行时间 2026-10-13T09:33:38.900Z 到 2026-10-13T09:48:05.078Z。fmt/fmtcheck、workspace all-target/all-feature Clippy、production panic约束Clippy、workspace all-target/all-feature tests、workspace lib no-default，以及17独立feature（含acp）全部通过。test日志重算86个target共3471 passed/0 failed/3ignored，59examples在实际执行链内。全部23原始log/runner receipt的命令、cwd、exit、signal、error、truncation、字节数和SHA已核验，原字节保存在 evidence.framework-final-34d5-gate-archive。

独立framework_final_review已有完整源码增量局部pass/action0及语义scope pass；完整新工程receipt进入其最终核对，最终审查结论由它独立确认，不能把本Evidence的日志核对冒作独立业务全风险证明。父协调者记录fetch origin/main95ec、merge Already up to date；这只说明候选前基准未前进，不是远端main交付证明。

## 已知缺口

3个ignored分别为真实LSP server、在线scholarly providers、Zotero credentials外部optin，日志保留说明，未宣称已运行。当前框架候选具完整本地工程通过；正式发布pin、SDK三语言/Host、CLI/官网、远端CI与精确main独立待验。Plan保持ready，用户Registry发布仍排除，完整Rust inventory继续遥测。


## 当前独立最终结论

audit.framework-final-34d5-gate-rereview：独立framework_final_review完成新23receipt/850源码/47archive及此前完整源码审查组合核验，implementation review PASS/action0。可进入授权framework里程碑交付；SDK/ACK/消费者/remote/main以及用户排除Registry不由此扩大完成。
