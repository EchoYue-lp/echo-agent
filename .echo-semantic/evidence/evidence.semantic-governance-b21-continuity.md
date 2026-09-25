---
schema_version: 1
id: evidence.semantic-governance-b21-continuity
kind: evidence
observed_at: c5f7688212d45d5bdcdbf60342605e8bfb176cae
source_refs:
  - docs/adr/0031-sdk-identity-governance-scope.md
  - docs/adr/0032-sdk-contract-scope-classification.md
  - docs/adr/0041-semantic-governance-continuity.md
  - echo-sdk-protocol/src/inventory.rs
  - echo-sdk-protocol/tests/facade_inventory.rs
  - echo-sdk-host/src/core_profile/facade/source_operations.rs
  - contracts/sdk/parity-manifest.json
  - contracts/sdk/parity-manifest.schema.json
  - scripts/check-sdk-contracts.sh
  - scripts/check-language-sdks.sh
supports: [behavior.sdk-facade-routing, rule.sdk-rust-authority, behavior.workspace-composition]
limitations:
  - 只处置b21aba01基线中的20个非保全SDK治理义务，不替代其后各repair slice自己的Finding和change-evidence
  - 当前54个open Finding仍未修复；远程CI、PR/merge、发布和docs.rs渲染由独立交付证据负责
evidence_type: semantic_continuity
merge_base_revision: b21aba01b34e74c93d783a89db895282ba831c3c
predecessor_revisions:
  - b21aba01b34e74c93d783a89db895282ba831c3c
result_snapshot: source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe
resolutions:
  "behavior.sdk-facade-routing":
    disposition: resolved_conflict
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 220f99c0b937bf677c6f2d41e595b289757de9a12082b78ba28f8be1203d6ebc
    replacement_ref: map.sdk-facade-parity#scenario:sdk-contract-scope
    evidence_refs: &equivalence_refs [evidence.sdk-governance-scope-equivalence]
    decision_authorities: &continuity_authority
      - kind: adr
        path: docs/adr/0041-semantic-governance-continuity.md
        content_digest: dda2d6bf1498ca087dfef6a27f6e8ab81252664801e56c8f1692c0d5ae04959e
    compatibility_impact: 运行route和错误边界不变，完成度口径改由identity级contract scope和capability治理表达
    rollback_ref: 按ADR 0041恢复b21aba01语义对象和源码依赖后，移除此resolution并重跑continuity
  "evidence.sdk-contracts":
    disposition: resolved_conflict
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 53e8776f1f87ead65fc6a7339609d202d25c88f9849131821e1767418400d5df
    replacement_ref: map.sdk-facade-parity#scenario:sdk-contract-scope
    evidence_refs: *equivalence_refs
    decision_authorities: *continuity_authority
    compatibility_impact: 合同证据增加scope和后续repair依据，不改变public API、wire或已交付语言行为
    rollback_ref: 按ADR 0041恢复b21aba01证据正文和source refs后，移除此resolution并重跑continuity
  "map.sdk-facade-parity#scenario:source-operation-closure":
    disposition: resolved_conflict
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: f5e522855efbade93dcfcf0d39aca2e09562c9945d513040fd54576fb624967b
    replacement_ref: map.sdk-facade-parity#scenario:sdk-contract-scope
    evidence_refs: *equivalence_refs
    decision_authorities: *continuity_authority
    compatibility_impact: source operation仍闭合到Host adapter，consumer contract acceptance由独立scope场景拥有
    rollback_ref: 按ADR 0041恢复b21aba01场景引用后，移除此resolution并重跑continuity
  "rule.sdk-rust-authority":
    disposition: resolved_conflict
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 478125ec133434b472ac82d97aae6a2ed67cf514fe9f0e57279dfd37663763de
    replacement_ref: map.sdk-facade-parity#scenario:sdk-contract-scope
    evidence_refs: *equivalence_refs
    decision_authorities: *continuity_authority
    compatibility_impact: Rust唯一运行语义权威不变，新增scope约束明确禁止Host或语言SDK建立第二权威
    rollback_ref: 按ADR 0041恢复b21aba01规则正文和引用后，移除此resolution并重跑continuity
  "discovery.sdk-facade-baseline#unresolved:intrinsic 语言行为与逐项领域/失败语义证据":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: d641f3626411e08dd87f0a134d50784f1238337a14f600856aed37a5bd1f18f1
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 旧逐identity未知项退役，未确认能力继续作为1443个deferred identity的capability backlog
    rollback_ref: 按ADR 0041恢复b21aba01 discovery unknown后，移除此resolution并重跑continuity
  "discovery.sdk-facade-baseline#unresolved:三语言整体 Parity complete 状态与最终发布检出证据":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 3910f4f5f7b7cf78f5f506d19f499226c7d1e45e483411af9b7455d74a8b45ae
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 旧总体parity口径退役，external contract语言门禁和未完成远程发布状态分别保留
    rollback_ref: 按ADR 0041恢复b21aba01 discovery unknown后，移除此resolution并重跑continuity
  "discovery.sdk-facade-baseline#unresolved:全仓 inventory 与 behavior model closure":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: daa924ae4bce943ef6550f66394f3763f1e1022bce78e0eda9c2d4b9596a94c7
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 旧SDK子图未知项退役，全workspace baseline已闭合路径和行为模型且保留具体动态unknown
    rollback_ref: 按ADR 0041恢复b21aba01 discovery unknown并移除workspace baseline后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 53151b64ae65c5b1214b88a01fc12df37ccc091b58da2db01f451482a346df29
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役inventory.rs旧blob依赖，当前inventory和合同测试继续有效
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 18ef8a0f6331c8230b0772bd9458a38626b47e6a3ca0826c89947144e3cb25d4
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役revisioned task旧blob依赖，当前CAS修复由独立Finding和Evidence拥有
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 91fe17c88c077800e27b7ccfdc13e3625456275e8a6e19c5225b2dd2321978c9
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役TypeScript catalog旧blob依赖，当前external contract gate继续覆盖
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 48f742e82a39d91ae3d4ea0688db9fd84e465b1589a3fecc4c06be5edd49fa47
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役旧operation catalog blob依赖，当前catalog仍由生成合同校验
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 75da46bd860194b12813f604c0d6982f608a77647665fbfa8de3105048bf53e0
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役Improve loop旧blob依赖，当前iteration修复由独立Finding和Evidence拥有
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 6fb826edabc49d2562b4dfefdf1b9865225d280dc99b2c2d9430022e3153d520
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役parity schema旧blob依赖，schema v2和生成物检查继续有效
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 3e27d5e3496c4659619e99f7047246a8927cc316a9d666c4ce319ff180142b35
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役Java parity test旧blob依赖，当前external contract gate继续覆盖
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: de78f6aa565b9d25f989cc14ac80942cc47451a1eadd349b8b36f4386923c198
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役public API inventory旧blob依赖，当前inventory继续作为Rust drift权威
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 9a4ace001799d8abdba9b6fe59e589eecf854113e798a8a1921b1e46dc8ec8ba
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役Rust facade inventory test旧blob依赖，当前scope和alias断言继续覆盖
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: fc527e766a893371f7a475b9b758b24bf4360864ec20b29b3920b95777f8f209
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役Python catalog旧blob依赖，当前external contract gate继续覆盖
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 70222e79fb94edebe0ab21eab6af204e7648503ed4c3779bbe3e2daf6e382871
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役Eval runner旧blob依赖，当前workspace、settlement和trace修复由独立Finding和Evidence拥有
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 73da072b588e2499b70f8d36fcded4ffac38fc41b2d582baa8b86122d0b94d51
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役React capability旧blob依赖，当前stream validation修复由独立Finding和Evidence拥有
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
  "evidence.sdk-contracts#source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe":
    disposition: retired
    predecessor_fingerprints:
      b21aba01b34e74c93d783a89db895282ba831c3c: 70c93a4afaf71e925b853fc86cbd6df3b9987981daa466268835ec43c4e9c8f1
    evidence_refs: []
    decision_authorities: *continuity_authority
    compatibility_impact: 仅退役parity manifest旧blob依赖，当前schema v2 manifest继续由唯一generator维护
    rollback_ref: 按ADR 0041恢复该旧blob和证据source ref后，移除此resolution并重跑continuity
---

# 从 b21aba01 到最终候选结果的语义连续性

## 支持的结论

以`b21aba01b34e74c93d783a89db895282ba831c3c`同时作为共同基准和唯一前置版本时，累计治理分支中所有语义义务要么保持原指纹，要么由本Evidence的20项resolution显式处置。四个变化对象经行为等价证据和ADR 0041收敛到SDK contract-scope场景；三个旧discovery unknown和十三个旧源码blob依赖按证据边界退役。

退役的是旧问题表述或旧内容指纹，不是Rust public API、运行路径、SDK artifact、测试或尚未完成的deferred capability。各高风险repair仍由自己的Finding、task-scoped preflight、repair/verification Evidence和独立rereview负责。

## 来源与范围

义务及predecessor fingerprint来自semantic continuity verifier对远程main基线和候选结果的可重算比较。决策权威是ADR 0041；四个冲突的等价依据是`evidence.sdk-governance-scope-equivalence`。候选结果以排除`.echo-semantic`后的source digest绑定，避免Evidence自引用commit SHA。

## 已知缺口

本Evidence不关闭当前54个open Finding，不代表远程CI、PR/merge、发布或docs.rs完成，也不把1443个deferred identity解释为已交付或逐项待办。
