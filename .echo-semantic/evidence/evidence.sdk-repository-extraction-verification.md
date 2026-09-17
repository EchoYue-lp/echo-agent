---
schema_version: 1
id: evidence.sdk-repository-extraction-verification
kind: evidence
observed_at: source:bfa5b4590c617d8286f2b80571d5c450622d47c7425d6f1ecb1978bf85743352
source_refs: [Cargo.toml, Cargo.lock, README.md, README.zh.md, .github/workflows/rust-ci.yml, scripts/verify.sh, echo-agent-learning/tests/documentation_contract.rs, tests/acp_agent_adapter.rs, tests/fixtures/acp/v1/prompt-resource-link-valid.json, tests/fixtures/acp/v1/session-relative-cwd-invalid.json]
supports: [finding.sdk-repository-extraction, behavior.workspace-composition, rule.framework-layer-ownership]
limitations: ["合流0e09324a后的focused门禁已刷新；full workspace gate、17-feature matrix、continuity和最终独立复审等待SDK完成candidate pin后执行。", "SDK protocol、Host、Wave 2 inventory和三语言 parity由独立echo-agent-sdk仓库的后续outcome验证。", "本证据未声称所有历史semantic Finding已关闭。"]
---

# SDK repository extraction verification

## 支持的结论

Framework 候选的 Cargo metadata 只包含 framework/runtime 与 learning package；root README、双语架构文档、learning documentation contract 和 framework CI 已切换到 framework-only 事实源，并链接独立 SDK 仓库。PR #124/#125 的 runtime 源码和测试已结构化保留。

## 执行证据

合流 `0e09324a` 后在当前源码摘要重新运行以下 focused 门禁，均 exit 0：

- `cargo metadata --no-deps --format-version 1 --locked`：仅9个framework/runtime/learning package；
- `cargo test -p echo-agent-learning --test documentation_contract --locked`：10 passed；
- `cargo test -p echo_agent --features acp --test acp_agent_adapter --locked`：19 passed；
- `cargo check --workspace --all-features --locked`；
- semantic strict snapshot。

未启用`acp`的首次adapter命令只编译并执行0项，未作为行为证据。初始候选`17548779`的
full gate与17-feature结果不覆盖当前合流树；最终关闭前仍须重新执行完整门禁、continuity和独立复审。

## 来源与范围

验证范围是 extraction worktree 的当前文件、Cargo workspace、文档拓扑和 CI job 定义。

## 已知缺口

独立 SDK PR #1 `6f743d1` 的8项远端CI已全绿，但仍缺 PR #124 的11个 Journal canonical
identity与2个签名变化，并继续pin初始`17548779`；framework候选在SDK吸收9724项payload、
精确pin当前候选并重新全绿前不得合入main。
