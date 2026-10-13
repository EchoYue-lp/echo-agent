---
schema_version: 1
id: audit.native-tool-value-fidelity-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: contract_evidence
freshness: examined
revision: 52eb905fe03711151508d1f439f87a863edaa839
finding_refs:
- finding.database-runtime-cell-fidelity
- finding.rag-preview-unicode-count
challenges:
  runtime-cell-authority:
    revision: 52eb905fe03711151508d1f439f87a863edaa839
    source_refs:
    - echo-tools/src/database.rs
    - docs/evidence/database-describe-fidelity/before.log
    - docs/evidence/database-describe-fidelity/after.log
    evidence_refs:
    - evidence.database-runtime-cell-repair
    - evidence.database-runtime-cell-verification
  unicode-preview-consistency:
    revision: 52eb905fe03711151508d1f439f87a863edaa839
    source_refs:
    - echo-tools/src/rag.rs
    - docs/evidence/database-describe-fidelity/rag-before.log
    - docs/evidence/database-describe-fidelity/rag-after.log
    evidence_refs:
    - evidence.rag-preview-unicode-repair
    - evidence.rag-preview-unicode-verification
  reversible-evidence-and-bounded-claims:
    revision: 52eb905fe03711151508d1f439f87a863edaa839
    source_refs:
    - echo-tools/src/database.rs
    - echo-tools/src/rag.rs
    - echo-tools/src/registry.rs
    - docs/en/02-tools.md
    - docs/zh/02-tools.md
    - docs/en/42-database-tools.md
    - docs/zh/42-database-tools.md
    - docs/evidence/database-describe-fidelity/receipt.json
    - docs/evidence/database-describe-fidelity/SHA256SUMS.json
    - docs/evidence/database-describe-fidelity/independent-review-forwarded.txt
    evidence_refs:
    - evidence.database-runtime-cell-repair
    - evidence.database-runtime-cell-verification
    - evidence.rag-preview-unicode-repair
    - evidence.rag-preview-unicode-verification
---

# 原生 Tool 值保真：独立源码复审

## 审查范围

审查者 sdk_capability_acceptance_audit，独立只读六个冻结源码/文档文件、20份原始归档、可恢复before/after与SQLx实际解码路径。主协调者在会话中逐字转发结论；原文见 docs/evidence/database-describe-fidelity/independent-review-forwarded.txt。这是 conversation review transcript，不是脚本stdout或独立重新运行测试的收据。

## 已检查故障假设

列声明不等于实际值类型；实际SQL NULL与缺失值不能合并；float非有限数不能默null；i64边界/Unicode/BLOB保持；RAG metadata必须与200scalar预览相同定义；历史错误fixture不得冒充行为红。

## 实际实现路径与证据

SQLx0.8.6 AnyRow.try_get_raw 经 fallible values.get、ValueRef.is_null/type_info，与 try_get 的 runtime compatibility；原 typed decoder保持。RAG只修原metadata chars.count，chunk/index算法保持。六源码/文档和原20档摘要匹配，生产数据库 a53610a7f1f3250a749eff3306aebe577157b4984bc572e9589da66bffdf3a0a、RAG d7dacc2133e935b982237807cbb13b7b3de7d11cbdb85d4662793f357bd77f6f。

## 问题记录

限定源码修复 PASS；Critical、Important、Minor均0。真实四数据库反例和两RAG反例转绿，原两数据库回归保持通过；focused lint/paniclint/fmt/taskscope通过。

## 残余风险

新完整门禁、N后继精确main、公开SDK依赖与真实新Host三语言均待验；旧0435 byte计数证据只保留历史。

## 未检查项

不证明PG/MySQL真实server、全能力风险视角或生产交付。此Audit只闭合两项具体源码故障的独立复审链。

历史观察定位：原 source:782042b6c310f1e2cde73ccc5adf5039a8aa50e95bd1e264900d25f9ca4b19da 的930个非语义文件已精确保存为内部可恢复 Git 52eb905fe03711151508d1f439f87a863edaa839，源码摘要重算相同；真实 HEAD/index/branch 未变化。此 Git 对象只保存原DB/RAG观察，不证明后续ClinicalTrials增量或公开交付。
