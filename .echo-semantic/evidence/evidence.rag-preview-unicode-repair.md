---
schema_version: 1
id: evidence.rag-preview-unicode-repair
kind: evidence
observed_at: 52eb905fe03711151508d1f439f87a863edaa839
source_refs:
- echo-tools/src/rag.rs
- docs/en/02-tools.md
- docs/zh/02-tools.md
- docs/evidence/database-describe-fidelity/receipt.json
- docs/evidence/database-describe-fidelity/SHA256SUMS.json
- docs/evidence/database-describe-fidelity/README.md
- docs/evidence/database-describe-fidelity/independent-review-forwarded.txt
- docs/evidence/database-describe-fidelity/rag-before.log
- docs/evidence/database-describe-fidelity/rag-after.log
supports:
- behavior.native-tool-value-fidelity
- rule.native-tool-value-authority
limitations:
- Focused exact source and original counterexamples only; no PostgreSQL/MySQL native
  server proof.
- Full workspace gates after latest published Channel successor, CI, precise main
  and new SDK Host three-language evidence remain outstanding.
- Independent review is a forwarded conversation transcript, not tool stdout; its
  scope is six frozen source/docs files and twenty original archive files.
---

# RAG 字符数和截断状态误用字节长度：修复

## 支持的结论

旧 中文😀甲 char_count 为13而非4，70中文 scalars 完整预览仍 truncated=true。修复只让两个 metadata 共享 chars().count()，原 chunker/preview/index不变。

## 来源与范围

RAG同两项旧反例失败，新两项通过，覆盖4字符与70/200/201中文及ASCII边界。
exact命令与原始输出在 docs/evidence/database-describe-fidelity/receipt.json 和对应日志。两个失败fixture尝试分别标为编译失败及三行为失败/一URL错误，不充当最终四行为红证据。before/after源码可逆base64和decoded SHA保留，原20-file清单不改写。

## 已知缺口

package warnings lint、四项生产panic lint与fmt均已通过；独立源码复审0问题。公开revision、完整门禁、新Host三语言和精确main尚未证明。此 Evidence 不提升 broad Tool/permission 审查状态。

历史观察定位：原 source:782042b6c310f1e2cde73ccc5adf5039a8aa50e95bd1e264900d25f9ca4b19da 的930个非语义文件已精确保存为内部可恢复 Git 52eb905fe03711151508d1f439f87a863edaa839，源码摘要重算相同；真实 HEAD/index/branch 未变化。此 Git 对象只保存原DB/RAG观察，不证明后续ClinicalTrials增量或公开交付。
