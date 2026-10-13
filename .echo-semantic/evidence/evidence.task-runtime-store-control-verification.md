---
schema_version: 1
id: evidence.task-runtime-store-control-verification
kind: evidence
observed_at: 8843afd5029afba9be8a6551de4ab0a93951311f
source_refs:
- echo-orchestration/src/tasks/mod.rs
- echo-orchestration/src/planning/validator.rs
- echo-orchestration/src/tasks/revisioned.rs
- echo-orchestration/src/tasks/runtime.rs
- echo-orchestration/src/tasks/runtime_service.rs
- echo-orchestration/src/tasks/runtime_executor.rs
- src/agent/subagent/team/mod.rs
- src/agent/subagent/types.rs
- src/acp/extension.rs
- docs/adr/0060-task-execution-store-cas-and-local-interruption.md
- docs/en/09-tasks.md
- docs/zh/09-tasks.md
- docs/en/lifecycles.md
- docs/zh/lifecycles.md
- AGENTS.md
- echo-agent-learning/tests/example_contracts.rs
- echo-agent-learning/tests/documentation_contract.rs
supports:
- behavior.task-subagent-execution
- rule.task-subagent-authority
- behavior.protocol-projection
- rule.protocol-role-separation
limitations:
- 只证明本框架修复候选及列出的故障假设，不证明 SDK/CLI/website consumer 或远端 main/CI 已完成
- 既有其他 open Finding 与 needs_review 场景保留；完整 Rust public inventory 不自动成为三语言 API
- 2026-10-12 独立完整复审重新绑定当前候选，核对等价类型别名/条件合并/文档平铺、ADR 格式和完整门禁；见当前 rereview Audit
- ADR Status 仅规范已确认决策的机器可读格式；本地 Cargo 全门禁运行于该格式调整之前，runtime 源码未再变化
gate_baseline_revision: 95ec66c6083a8feb17a1c28de0d01e1a24ca8a28
gate_tracked_diff_digest: c7e98cefe25ed9370d08d9f490fa19d86bfa5e964ef767bb03f1cce443138efe
gate_source_digest_before_adr_status_format: 3c27b87a419b6ca88643f81c8a0bcc2ac747d427691f02397e4bde5fb8ba285e
current_source_digest: 0f404ee705bc17985778b8df30b26d43ec47c42e26e27bcea580f355f1ab0a37
gate_command_results:
- label: format
  command:
  - cargo
  - fmt
  - --all
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/format.log
  logSha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
  bytes: 0
- label: format-check
  command:
  - cargo
  - fmt
  - --all
  - --
  - --check
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/format-check.log
  logSha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
  bytes: 0
- label: clippy-all
  command:
  - cargo
  - clippy
  - --workspace
  - --all-targets
  - --all-features
  - --locked
  - --
  - -D
  - warnings
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/clippy-all.log
  logSha256: d23818de5e4e4e56c33a27a8e20d677625a54eaba8f047db067fedb08482555f
  bytes: 1006
- label: clippy-production
  command:
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
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/clippy-production.log
  logSha256: 619504d599a903172784e46c109e252417e7a0e30a44f8556d4f6bda4ef6cb73
  bytes: 1274
- label: tests-all
  command:
  - cargo
  - test
  - --workspace
  - --all-targets
  - --all-features
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/tests-all.log
  logSha256: 4d7cc299912655b550e5a275b954a5ba5547d13f316a8074098d4825edcc1e1f
  bytes: 345274
- label: no-default-library
  command:
  - cargo
  - check
  - --workspace
  - --lib
  - --no-default-features
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/no-default-library.log
  logSha256: ee51036688c20fd024d2d69c8771dbdb912e0a790d42568da9b3ac2ec4c23662
  bytes: 3266
- label: feature-acp
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - acp
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-acp.log
  logSha256: 06df72764b89f0b8d4be850efecc8000d959f3f7b04f2ab7940aab969d72031d
  bytes: 1936
- label: feature-a2a
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - a2a
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-a2a.log
  logSha256: e2f80e3d65add6ec1b5032b1206c9ce1e606ebc829b403dfde43c4229f420838
  bytes: 1602
- label: feature-mcp
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - mcp
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-mcp.log
  logSha256: 9e9a6a9ba4a63706b517a7b92c7e6b680bd8479aa13adf801d1a6109fd79ef95
  bytes: 1149
- label: feature-lsp
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - lsp
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-lsp.log
  logSha256: 8bf91f4940fb05a5200084a8cfb7570a4fb103f986b2ddf784ffbf1d17d3f3bb
  bytes: 994
- label: feature-sqlite
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - sqlite
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-sqlite.log
  logSha256: 53b5a3f7fe7eb0b626c5fd1749786e1f45194ae768bde027cf1ec456cc86f555
  bytes: 890
- label: feature-telemetry
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - telemetry
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-telemetry.log
  logSha256: 56204f732c1b421fa69978d3955cc469e5d710bfb9095ff40793ddd2ef9be211
  bytes: 1580
- label: feature-topology
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - topology
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-topology.log
  logSha256: b448aa88eaf3c73dde4b9cfe125085976153a28f39fe73d68084b31e98d2be32
  bytes: 179
- label: feature-subagent
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - subagent
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-subagent.log
  logSha256: cb9a63894b071f3ea374fa81d7967964879c81bf3912bbbfce1ce663767ec44f
  bytes: 179
- label: feature-web
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - web
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-web.log
  logSha256: 8adb59390e4b372691c8b378779366b0e94b65aa8d8abdade7f85ed0b5029ebf
  bytes: 1814
- label: feature-media
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - media
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-media.log
  logSha256: 0defeaa9189e0a7e96c2f6b6917169ece1c6ab5d435557a2d48c7988b698196f
  bytes: 1566
- label: feature-data
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - data
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-data.log
  logSha256: 994a3baa3c6d1c8307e09db9ebc1e807bdee7cfc58371ec40104d03145c7bde1
  bytes: 3246
- label: feature-statistics
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - statistics
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-statistics.log
  logSha256: 197538cad78ab0df170da758bcda31a0e7a251fcdbddda6aa5cf97b1b3ac8cd9
  bytes: 309
- label: feature-channels
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - channels
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-channels.log
  logSha256: 46395106be49515e53f3704dbddddcc3d30fd0df5c2420b1944ba63b67e14d16
  bytes: 994
- label: feature-git
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - git
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-git.log
  logSha256: 8c379728c6b4b7c8589237ee5ea726bb97ddf227e4a59a93611d7a616fc5b4fb
  bytes: 309
- label: feature-database
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - database
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-database.log
  logSha256: 7cf8b8e6be65f547e71354768b563108cea53975a468e5a3f9771b1548f5a1de
  bytes: 2003
- label: feature-rag
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - rag
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-rag.log
  logSha256: 2b0e19f39290adfb7a4239478ece77ebeec33006426f81c3d48cc540fa161902
  bytes: 309
- label: feature-chart
  command:
  - cargo
  - check
  - -p
  - echo_agent
  - --no-default-features
  - --features
  - chart
  - --locked
  exitCode: 0
  logPath: /tmp/framework-task-runtime-final-gates/feature-chart.log
  logSha256: 1850cbbfd10b40a78d2532cc48f05e2db706634ffe154779f384d422707366f7
  bytes: 309
---

# Task runtime Store/control 验证证据

## 支持的结论

最后业务源码变更后 focused Task 174/174、Subagent 169/169；ACP extension 13/13 对应 child-token 切片通过。完整框架门禁遵守 AGENTS 原命令，共 6 条必跑命令和 17 个独立 feature 检查全部退出 0，零警告。workspace all-target/all-feature 测试共 3426 passed、0 failed、3 既有 ignored；86 个 test summary 与 59 个 example binary 编译记录，example_contracts 实际执行。ignored 为仓库既有 opt-in 测试，没有新增忽略或缩小验证集合。

## 来源与范围

统一使用共享 target，CARGO_BUILD_JOBS=2，dev/test DEBUG=0、INCREMENTAL=0。最终代码 diff 摘要为 `c7e98cefe25ed9370d08d9f490fa19d86bfa5e964ef767bb03f1cce443138efe`；测试时非语义源码摘要为 `3c27b87a419b6ca88643f81c8a0bcc2ac747d427691f02397e4bde5fb8ba285e`。其后只有 ADR 状态章节机器格式和 `.echo-semantic` 材料同步，runtime/test 源码未再变化。命令日志及退出码摘要保存在 `/tmp/framework-task-runtime-final-gates/manifest.json`，每条内容 SHA 如下。

| Command | Exit | Log SHA-256 |
|---|---|---|
| `cargo fmt --all` | 0 | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| `cargo fmt --all -- --check` | 0 | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 0 | `d23818de5e4e4e56c33a27a8e20d677625a54eaba8f047db067fedb08482555f` |
| `cargo clippy --workspace --lib --bins --all-features --locked -- -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable` | 0 | `619504d599a903172784e46c109e252417e7a0e30a44f8556d4f6bda4ef6cb73` |
| `cargo test --workspace --all-targets --all-features --locked` | 0 | `4d7cc299912655b550e5a275b954a5ba5547d13f316a8074098d4825edcc1e1f` |
| `cargo check --workspace --lib --no-default-features --locked` | 0 | `ee51036688c20fd024d2d69c8771dbdb912e0a790d42568da9b3ac2ec4c23662` |
| `cargo check -p echo_agent --no-default-features --features acp --locked` | 0 | `06df72764b89f0b8d4be850efecc8000d959f3f7b04f2ab7940aab969d72031d` |
| `cargo check -p echo_agent --no-default-features --features a2a --locked` | 0 | `e2f80e3d65add6ec1b5032b1206c9ce1e606ebc829b403dfde43c4229f420838` |
| `cargo check -p echo_agent --no-default-features --features mcp --locked` | 0 | `9e9a6a9ba4a63706b517a7b92c7e6b680bd8479aa13adf801d1a6109fd79ef95` |
| `cargo check -p echo_agent --no-default-features --features lsp --locked` | 0 | `8bf91f4940fb05a5200084a8cfb7570a4fb103f986b2ddf784ffbf1d17d3f3bb` |
| `cargo check -p echo_agent --no-default-features --features sqlite --locked` | 0 | `53b5a3f7fe7eb0b626c5fd1749786e1f45194ae768bde027cf1ec456cc86f555` |
| `cargo check -p echo_agent --no-default-features --features telemetry --locked` | 0 | `56204f732c1b421fa69978d3955cc469e5d710bfb9095ff40793ddd2ef9be211` |
| `cargo check -p echo_agent --no-default-features --features topology --locked` | 0 | `b448aa88eaf3c73dde4b9cfe125085976153a28f39fe73d68084b31e98d2be32` |
| `cargo check -p echo_agent --no-default-features --features subagent --locked` | 0 | `cb9a63894b071f3ea374fa81d7967964879c81bf3912bbbfce1ce663767ec44f` |
| `cargo check -p echo_agent --no-default-features --features web --locked` | 0 | `8adb59390e4b372691c8b378779366b0e94b65aa8d8abdade7f85ed0b5029ebf` |
| `cargo check -p echo_agent --no-default-features --features media --locked` | 0 | `0defeaa9189e0a7e96c2f6b6917169ece1c6ab5d435557a2d48c7988b698196f` |
| `cargo check -p echo_agent --no-default-features --features data --locked` | 0 | `994a3baa3c6d1c8307e09db9ebc1e807bdee7cfc58371ec40104d03145c7bde1` |
| `cargo check -p echo_agent --no-default-features --features statistics --locked` | 0 | `197538cad78ab0df170da758bcda31a0e7a251fcdbddda6aa5cf97b1b3ac8cd9` |
| `cargo check -p echo_agent --no-default-features --features channels --locked` | 0 | `46395106be49515e53f3704dbddddcc3d30fd0df5c2420b1944ba63b67e14d16` |
| `cargo check -p echo_agent --no-default-features --features git --locked` | 0 | `8c379728c6b4b7c8589237ee5ea726bb97ddf227e4a59a93611d7a616fc5b4fb` |
| `cargo check -p echo_agent --no-default-features --features database --locked` | 0 | `7cf8b8e6be65f547e71354768b563108cea53975a468e5a3f9771b1548f5a1de` |
| `cargo check -p echo_agent --no-default-features --features rag --locked` | 0 | `2b0e19f39290adfb7a4239478ece77ebeec33006426f81c3d48cc540fa161902` |
| `cargo check -p echo_agent --no-default-features --features chart --locked` | 0 | `1850cbbfd10b40a78d2532cc48f05e2db706634ffe154779f384d422707366f7` |

初次 full Clippy 的 private type complexity/collapsible-if 全部修复并原命令重跑；这些变更仅等价私有别名/条件写法。文档合同初次因 H3 破 flat shape 失败，保留 ACP 说明改 bold lead 后原 workspace test 重跑通过。所有失败历史日志保留，没有 allow、lint 阈值或结构门槛放宽。

补充确定性门禁：`uv run /Users/ls/.codex/plugins/cache/echo-semantic/echo-semantic/0.4.0/skills/semantic-contract/scripts/verify_semantic.py --root /Users/ls/MyWork/code/ylp_agent_learn/echo-agent-worktrees/task-runtime-store-control --strict-snapshot` 及同命令 `--base 95ec66c6083a8feb17a1c28de0d01e1a24ca8a28 --require-change-evidence` 均实际退出 0。它们只证明本次结构、引用、路径分类、当前摘要与高风险依据成立，不替代业务复审。

## 已知缺口

本记录不是 main merge、remote CI、SDK/CLI/website consumer、三语言真实 Host E2E 或 framework 全能力审计闭合证明。CLI 公共 TaskExecution/receipt 消费适配截至本记录仍为 pending，由协调线程在独立树验证。独立完整复审已重新检查基准到当前候选并 pass；类型别名/if/doc flat/ADR 状态格式和模型材料均核对。结论归档于 audit.task-runtime-store-control-rereview 与 audit.extension-invocation-cancellation-scope-rereview；结构检查不替代业务审查。


历史定位说明：原未提交 source8896e508 的846个源码/文档文件已逐字节重建，唯一 source_digest 精确匹配，并保存到本地 recoverable Git snapshot `8843afd5029afba9be8a6551de4ab0a93951311f`（refs/echo-semantic/snapshots/sdk-framework-reviewed-8896）。这是内部审查证据快照，不是交付分支提交、公开 dependency 或 main。原结论与限制保持；新 parent-drop 源码不借此历史 pass。
