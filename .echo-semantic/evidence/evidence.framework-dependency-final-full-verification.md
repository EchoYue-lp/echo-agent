---
schema_version: 1
kind: evidence
observed_at: 77de6ca90ad842eb4b7b2921a4baa954f41cccfe
source_refs:
- Cargo.lock
- Cargo.toml
- echo-core/Cargo.toml
- echo-execution/Cargo.toml
- echo-integration/Cargo.toml
- echo-agent-learning/Cargo.toml
- docs/dependencyrepair.md
- docs/evidence/dependency-repair-20261013/receipt.json
supports:
- behavior.workspace-composition
- rule.framework-layer-ownership
limitations:
- Exact dependency-repaired local candidate; remote dependency-policy CI, PR/main and SDK final published-pin/consumer
  evidence remain separate.
- Existing three external-environment opt-in ignored tests were not executed; deny duplicate-version diagnostics
  retain the original policy.
- Old34d5 full receipts remain historical at4eb and do not prove current dependency behavior.
id: evidence.framework-dependency-final-full-verification
manifest_path: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/manifest.json
manifest_sha256: 0d45d7414739a1fdf57801bfcea536ffd672802e6cce7eece11abd5e1744fb1f
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
  startedAt: '2026-10-13T15:22:55.872Z'
  finishedAt: '2026-10-13T15:22:58.406Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/format-1.log
  logSha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/format-1.runner.json
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
  startedAt: '2026-10-13T15:22:58.409Z'
  finishedAt: '2026-10-13T15:23:00.845Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/format-check-1.log
  logSha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/format-check-1.runner.json
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
  startedAt: '2026-10-13T15:23:00.846Z'
  finishedAt: '2026-10-13T15:23:54.129Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/clippy-all-1.log
  logSha256: 1b399f2b4433e6274fc5028723ac30f328c52b5bdf10c645d27e19fc8ddbf6a5
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/clippy-all-1.runner.json
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
  startedAt: '2026-10-13T15:23:54.131Z'
  finishedAt: '2026-10-13T15:24:20.119Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/clippy-production-1.log
  logSha256: 6a1f9788412a55024f8d015699913771cd65f05a64d1fffd591a4779c765fe88
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/clippy-production-1.runner.json
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
  startedAt: '2026-10-13T15:24:20.121Z'
  finishedAt: '2026-10-13T15:27:22.048Z'
  exitCode: 101
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/test-all-1.log
  logSha256: 7fd9f5514f558dbb9e82899bec209885e577240c10aaead75c07354a3b080331
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/test-all-1.runner.json
  compilerWarnings: 0
  sourceUnchanged: true
- label: test-all
  attempt: 2
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
  startedAt: '2026-10-13T15:28:53.600Z'
  finishedAt: '2026-10-13T15:30:25.867Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/test-all-2.log
  logSha256: 982c41ee617cb22d1db8a150ac914bf90c56a06c7555d7417713be64406e5551
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/test-all-2.runner.json
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
  startedAt: '2026-10-13T15:30:25.871Z'
  finishedAt: '2026-10-13T15:30:42.435Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/no-default-1.log
  logSha256: 8b630c14a3eb07dcc9904c6692a9865ba00a0ae12cc40c83fa76e5c4afe74a67
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/no-default-1.runner.json
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
  startedAt: '2026-10-13T15:30:42.436Z'
  finishedAt: '2026-10-13T15:30:57.769Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-acp-1.log
  logSha256: 571bbd8d2384c68df72ff1ac9a13e807f490d5434bebadd5e1fbb976fa3027ec
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-acp-1.runner.json
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
  startedAt: '2026-10-13T15:30:57.770Z'
  finishedAt: '2026-10-13T15:31:13.971Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-a2a-1.log
  logSha256: 72f45f1a89188522da830d84468685142dfaad2d4b1751d2c1d102f7ffd3bb25
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-a2a-1.runner.json
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
  startedAt: '2026-10-13T15:31:13.973Z'
  finishedAt: '2026-10-13T15:31:28.788Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-mcp-1.log
  logSha256: 3373803a706258ee3a04423e520af09c9bdc4e628c758a15a7bbf1e13b8d265e
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-mcp-1.runner.json
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
  startedAt: '2026-10-13T15:31:28.790Z'
  finishedAt: '2026-10-13T15:31:43.771Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-lsp-1.log
  logSha256: 061975b7989b1b1391edc935eb1bd9cace5d9bc9d723c73a525b73707f54e48f
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-lsp-1.runner.json
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
  startedAt: '2026-10-13T15:31:43.773Z'
  finishedAt: '2026-10-13T15:31:55.771Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-sqlite-1.log
  logSha256: 04c39642425970451fa16b0ccbe774fc923ad3505bdb256de231cbd251996c7b
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-sqlite-1.runner.json
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
  startedAt: '2026-10-13T15:31:55.772Z'
  finishedAt: '2026-10-13T15:32:10.585Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-telemetry-1.log
  logSha256: 7db7ad57283ddaa81bea2e48a16a3cf1750f3f30ef9faf1b212375dd3568d0a4
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-telemetry-1.runner.json
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
  startedAt: '2026-10-13T15:32:10.588Z'
  finishedAt: '2026-10-13T15:32:17.900Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-topology-1.log
  logSha256: 00d46a16f284a9fa47785b4975b2faff5bb2d414ee59b002849d10ecafa79be2
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-topology-1.runner.json
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
  startedAt: '2026-10-13T15:32:17.902Z'
  finishedAt: '2026-10-13T15:32:26.562Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-subagent-1.log
  logSha256: 1bedc4b00327f1034c4c4fe87adbac56afe8d001f6ad5defef3e077abd399982
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-subagent-1.runner.json
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
  startedAt: '2026-10-13T15:32:26.564Z'
  finishedAt: '2026-10-13T15:32:41.611Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-web-1.log
  logSha256: c079f35d926bfcfd63d4650053bed09beaad245177d5f337f40446cec37b4c83
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-web-1.runner.json
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
  startedAt: '2026-10-13T15:32:41.613Z'
  finishedAt: '2026-10-13T15:32:56.207Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-media-1.log
  logSha256: d9d307e70dc46b41f392fa0506a77db86c023aa03c8021066bf11e79e1b178ca
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-media-1.runner.json
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
  startedAt: '2026-10-13T15:32:56.208Z'
  finishedAt: '2026-10-13T15:33:11.898Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-data-1.log
  logSha256: 456b9048fb9ee76f47a39d9f35a1edd7e17ec05a83e6f234936df2592938935c
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-data-1.runner.json
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
  startedAt: '2026-10-13T15:33:11.900Z'
  finishedAt: '2026-10-13T15:33:20.306Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-statistics-1.log
  logSha256: 191d1ecb476ebca031070d58847e77ff0dbd7ee45f379353734b210fdad7e599
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-statistics-1.runner.json
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
  startedAt: '2026-10-13T15:33:20.308Z'
  finishedAt: '2026-10-13T15:33:36.358Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-channels-1.log
  logSha256: 1cebad57dd582781e8fe92ef74c2d58b2fa6127a8fdaa75af498dbf4247901c5
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-channels-1.runner.json
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
  startedAt: '2026-10-13T15:33:36.360Z'
  finishedAt: '2026-10-13T15:33:44.033Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-git-1.log
  logSha256: 64b36cf48e2ad3230073dab8b738f99b186d930d30989377c2489b4918ad1c12
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-git-1.runner.json
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
  startedAt: '2026-10-13T15:33:44.035Z'
  finishedAt: '2026-10-13T15:33:59.761Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-database-1.log
  logSha256: 461f0c0de6dc39038027a399c56a29d3d3face5c646f31380091d7b6b9ae35c6
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-database-1.runner.json
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
  startedAt: '2026-10-13T15:33:59.763Z'
  finishedAt: '2026-10-13T15:34:07.536Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-rag-1.log
  logSha256: b93de13508ec7a010a40771fd6790d1eb85edd9da84a57f45dcc39e7b904119c
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-rag-1.runner.json
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
  startedAt: '2026-10-13T15:34:07.538Z'
  finishedAt: '2026-10-13T15:34:15.291Z'
  exitCode: 0
  log: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-chart-1.log
  logSha256: 61364eb3632c3140e0c9c4628ca08013ec268a7f962aff33f3e0010f6b2776fe
  runnerFile: /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/feature-chart-1.runner.json
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

# 依赖修复当前完整工程门禁

## 支持的结论

当前77de6ca90ad842eb4b7b2921a4baa954f41cccfe 新23完整命令最终全部exit0/warnings0，source/index不变；24次真实attempt保留一次test失败及同命令重跑成功。最新完整test日志3471 passed/0 failed/3外部optin ignored、86targets、59examples。initial/latest/current全文件SHA一致。

## 来源与范围

/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.worktrees/admin/framework-dependency-final-gates/manifest.json，SHA 0d45d7414739a1fdf57801bfcea536ffd672802e6cce7eece11abd5e1744fb1f；2026-10-13T15:22:55.870Z 到 2026-10-13T15:34:15.301Z。全部logSHA、runnerargv/cwd/logPath/exit/signal/error/truncation/bytes逐项核对。

首次test-all退出101源自语义writer误把baseline ancestor改为候选4eb，目标main95ec断言失败；已经恢复baseline祖先95ec，当前source digest不变，因为.semantic不入摘要。父协调者保留完整失败log/receipt，并在语义freeze后用相同全部test命令attempt2真实3471/0/3通过。此实际失败明确记录，不能被 sourceUnchanged 摘要掩盖。

## 已知缺口

依赖源码review PASS/action0已具；新最终semantic补充后还要strict及focused baseline3和独立最终receipt核对。远端dependency-policy CI、PR/main及SDK新published-pin消费不借旧Host。
