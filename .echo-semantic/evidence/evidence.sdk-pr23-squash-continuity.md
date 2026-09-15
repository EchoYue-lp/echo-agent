---
schema_version: 1
id: evidence.sdk-pr23-squash-continuity
kind: evidence
observed_at: f1e9027246760661144786e9e35615cd46d580c6
source_refs:
  - docs/adr/0031-sdk-identity-governance-scope.md
  - docs/sdk/README.md
  - contracts/sdk/parity-manifest.json
supports: [behavior.sdk-facade-routing, rule.sdk-rust-authority]
limitations:
  - 仅处置 PR #23 squash 的旧 source dependency，不闭合全 workspace inventory 或 behavior model
evidence_type: semantic_continuity
merge_base_revision: f12563c33de96b89baf9312807182f9500baa159
predecessor_revisions:
  - 37313cd5303ca21b4a232335f342b2b59da554df
  - f12563c33de96b89baf9312807182f9500baa159
result_snapshot: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
resolutions:
  "evidence.sdk-contracts#source:1239a1b718dbf0fbbacb6295516cc8a2532a5b3c3ab81957411462fe558b0d9d":
    disposition: retired
    predecessor_fingerprints:
      37313cd5303ca21b4a232335f342b2b59da554df: absent
      f12563c33de96b89baf9312807182f9500baa159: e7589827ea6f3605232ba55ccc2bb93a3a8beedb30bae74ac6db00b430a54568
    evidence_refs: []
    decision_authorities: &decision_authority
      - kind: adr
        path: docs/adr/0031-sdk-identity-governance-scope.md
        content_digest: 40cb3f1917e3fb585d88d74e98ec21355903d9bf0e22b50c9f2ca3dc400eb2ad
    compatibility_impact: 仅退役旧 TypeScript index blob 指纹，不改变公共 API、wire 或运行时行为
    rollback_ref: 恢复目标 main 的旧 source ref，移除此 resolution 并重跑双前置连续性检查
  "evidence.sdk-contracts#source:22d1578ba412e215fd4debfd0a28a4989e2248db7482688db26e021712935e9d":
    disposition: retired
    predecessor_fingerprints:
      37313cd5303ca21b4a232335f342b2b59da554df: absent
      f12563c33de96b89baf9312807182f9500baa159: ed50490dc236901cf60f147e422e5273ffd2f92d94482054953b74bd889eb39c
    evidence_refs: []
    decision_authorities: *decision_authority
    compatibility_impact: 仅退役旧 Python README blob 指纹，不改变公共 API、wire 或运行时行为
    rollback_ref: 恢复目标 main 的旧 source ref，移除此 resolution 并重跑双前置连续性检查
  "evidence.sdk-contracts#source:98fa802a0bee147cd5bb2ddc7560a6a7f50da6e11f429d196663ebb1d6d87c27":
    disposition: retired
    predecessor_fingerprints:
      37313cd5303ca21b4a232335f342b2b59da554df: absent
      f12563c33de96b89baf9312807182f9500baa159: 5064ea8c3619652746175b850732fdc1d50c3258b974ccbf7738739a7fab3b7d
    evidence_refs: []
    decision_authorities: *decision_authority
    compatibility_impact: 仅退役旧 Rust inventory blob 指纹，不改变公共 API、wire 或运行时行为
    rollback_ref: 恢复目标 main 的旧 source ref，移除此 resolution 并重跑双前置连续性检查
  "evidence.sdk-contracts#source:c39feedc79e0030a353c011d217882a8e4d5ddfad0f830a5dcec7b26bb4cdf99":
    disposition: retired
    predecessor_fingerprints:
      37313cd5303ca21b4a232335f342b2b59da554df: absent
      f12563c33de96b89baf9312807182f9500baa159: 4f50ada09338851feaba953f83631c11c72006b54b8c7a81b12b89e2ebefe26a
    evidence_refs: []
    decision_authorities: *decision_authority
    compatibility_impact: 仅退役旧 parity manifest blob 指纹，不改变公共 API、wire 或运行时行为
    rollback_ref: 恢复目标 main 的旧 source ref，移除此 resolution 并重跑双前置连续性检查
  "evidence.sdk-contracts#source:cb3f0313fe66a8635b44b30c120ad5af1501c5795582e9d036ce09d37bcd26d7":
    disposition: retired
    predecessor_fingerprints:
      37313cd5303ca21b4a232335f342b2b59da554df: absent
      f12563c33de96b89baf9312807182f9500baa159: 543d0f3620bbdb0fedd871e5337ba1fa70a1bc036a790218687fc7c6d88799ed
    evidence_refs: []
    decision_authorities: *decision_authority
    compatibility_impact: 仅退役旧 TypeScript README blob 指纹，不改变公共 API、wire 或运行时行为
    rollback_ref: 恢复目标 main 的旧 source ref，移除此 resolution 并重跑双前置连续性检查
  "evidence.sdk-contracts#source:ce8db8a82970ce189656158ad4ad70c651823774b00d7f98f9b5082e7505fa8f":
    disposition: retired
    predecessor_fingerprints:
      37313cd5303ca21b4a232335f342b2b59da554df: absent
      f12563c33de96b89baf9312807182f9500baa159: 99ac4ad9cadb4329a812e5d521b8ecc5a91cf2fd322a6c9fd1e826927006c19b
    evidence_refs: []
    decision_authorities: *decision_authority
    compatibility_impact: 仅退役旧 facade inventory test blob 指纹，不改变公共 API、wire 或运行时行为
    rollback_ref: 恢复目标 main 的旧 source ref，移除此 resolution 并重跑双前置连续性检查
  "evidence.sdk-contracts#source:db076592749cf914d34bb3989dcd15d1441ce6149788041bbb086faee05df5f5":
    disposition: retired
    predecessor_fingerprints:
      37313cd5303ca21b4a232335f342b2b59da554df: absent
      f12563c33de96b89baf9312807182f9500baa159: b3f1d7e46d2c0fe16dd10897c3371cfe547ad15f479bb8f672ff18aa8e8e79dd
    evidence_refs: []
    decision_authorities: *decision_authority
    compatibility_impact: 仅退役旧 Python package index blob 指纹，不改变公共 API、wire 或运行时行为
    rollback_ref: 恢复目标 main 的旧 source ref，移除此 resolution 并重跑双前置连续性检查
---

# PR #23 squash 语义连续性

## 支持的结论

PR #23 的 squash 结果保留两个前置版本中的行为、规则和当前 SDK 证据；目标 main 独有的七个旧内容指纹按 ADR 0031 显式退役。

## 来源与范围

本 Evidence 只覆盖 `f12563c3` 与 `37313cd5` 到候选结果的语义连续性。每个 resolution 都绑定原始 fingerprint、决策权威、兼容影响和回滚方式。

## 已知缺口

全 workspace inventory、行为模型、高风险边界审查和 intrinsic SDK backlog 仍保持开放，由后续 `semantic-discover` 与独立 SDK outcome 处理。
