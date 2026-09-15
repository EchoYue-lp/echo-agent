---
schema_version: 1
id: evidence.sdk-repository-extraction-verification
kind: evidence
observed_at: source:298b7209a1d4a5151d191db785daa2e394ab43f75a3bc321d8275f6cdf56c757
source_refs: [Cargo.toml, Cargo.lock, README.md, README.zh.md, .github/workflows/rust-ci.yml, scripts/verify.sh, echo-agent-learning/tests/documentation_contract.rs, tests/acp_agent_adapter.rs, tests/fixtures/acp/v1/prompt-resource-link-valid.json, tests/fixtures/acp/v1/session-relative-cwd-invalid.json]
supports: [finding.sdk-repository-extraction, behavior.workspace-composition, rule.framework-layer-ownership]
limitations: ["SDK protocol、Host 和三语言 parity 由独立 echo-agent-sdk 仓库的后续 outcome 验证。", "本证据未声称所有历史 semantic Finding 已关闭。"]
---

# SDK repository extraction verification

## 支持的结论

Framework 当前 Cargo metadata 只包含 framework/runtime 与 learning package；root README、双语架构文档、learning documentation contract 和 framework CI 已切换到 framework-only 事实源，并链接独立 SDK 仓库。

## 执行证据

工程门禁 `./scripts/verify.sh` exit 0，覆盖 formatter、两组 Clippy、workspace all-target/all-feature 测试和 no-default check；17 项根 crate 独立 feature check 全部 exit 0；`cargo test -p echo-agent-learning --test documentation_contract --locked` 为 10/10 passed；最终 `uv run scripts/verify_semantic.py --root <extraction-worktree> --strict-snapshot` exit 0。带 `--base 7e74d1443567981f318b302845d31a5673c76462 --require-change-evidence` 的高风险变更门禁已闭合本切片的 replacement/before-revision 关系，剩余唯一失败是 framework 既有未闭合 `unresolved`/`needs_review` backlog，未命中本切片新增路径。

## 来源与范围

验证范围是 extraction worktree 的当前文件、Cargo workspace、文档拓扑和 CI job 定义。

## 已知缺口

独立 SDK 当前仍处于 source-import checkpoint；其 protocol coupling、contract drift 和语言 gate 不属于本 framework extraction 的验证结论。
