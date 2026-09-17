---
schema_version: 1
id: evidence.sdk-repository-extraction-final-verification
kind: evidence
observed_at: 27c7701e1eb116db1076da7f84bb68898544a44c
source_refs:
  - Cargo.toml
  - Cargo.lock
  - README.md
  - README.zh.md
  - docs/adr/0051-extract-sdk-repository.md
  - docs/adr/0028-source-first-multilanguage-sdk-runtime.md
  - .github/workflows/rust-ci.yml
  - echo-agent-learning/tests/documentation_contract.rs
  - tests/acp_agent_adapter.rs
supports: [finding.sdk-repository-extraction, behavior.workspace-composition, behavior.protocol-projection, rule.framework-layer-ownership, rule.protocol-role-separation, rule.sdk-rust-authority]
limitations:
  - SDK source and remote CI evidence is from independently merged echo-agent-sdk PR #2; this Evidence does not assert binary or registry publication
  - custom downstream framework consumers remain responsible for their own dependency pin and compatibility checks
---

# SDK repository extraction final verification

## 支持的结论

Framework PR #126 is merged on `main` at `27c7701e1eb116db1076da7f84bb68898544a44c` with its
framework ACP/runtime behavior retained and SDK-owned source paths removed. The framework remote
CI was green for all seven required jobs.

The independent SDK PR #2 is merged with a real two-parent merge commit
`146f69a923e7df02417528c3dce6533312be85e1`, preserving SDK cutover head
`c8941c6033c8cd5f698502483fb28a80953be48d`. The SDK Host and lockfile pin the final framework
main revision, and generated contract telemetry is `9,724` canonical identities with scope counts
`5,622 / 1,774 / 790 / 90 / 1,448`.

Post-merge ancestry checks confirm `b80cf06`, `89d18f0`, `6f743d1`, `863cd5b` and `c8941c6` are
all ancestors of SDK `main`. SDK local and remote contract, Host, language, platform and dependency
gates passed, so the deletion boundary has a verified external owner.

## 来源与范围

Evidence covers the framework-only workspace boundary, retained ACP adapter, extraction ADR,
documentation contract, and external SDK provenance referenced by the merged delivery.

## 执行证据

- Framework PR #126: merged `27c7701e`, 7/7 remote CI checks successful.
- SDK PR #2: merged `146f69a9`, 8/8 remote CI checks successful.
- SDK post-merge: exact framework pin, `9,724` inventory, five ancestry anchors, and deleted SDK branch verified.
- Framework semantic strict snapshot and change-evidence validation passed on the extraction result.

## 已知缺口

本 Evidence 不证明尚未发布的 registry、二进制或安装器制品，也不替代下游 consumer 针对其
选定 SDK/framework 组合执行的兼容性验证。
