---
schema_version: 1
id: evidence.database-runtime-cell-repair
kind: evidence
observed_at: 52eb905fe03711151508d1f439f87a863edaa839
source_refs:
- echo-tools/src/database.rs
- docs/en/42-database-tools.md
- docs/zh/42-database-tools.md
- docs/evidence/database-describe-fidelity/receipt.json
- docs/evidence/database-describe-fidelity/SHA256SUMS.json
- docs/evidence/database-describe-fidelity/README.md
- docs/evidence/database-describe-fidelity/independent-review-forwarded.txt
- docs/evidence/database-describe-fidelity/before.log
- docs/evidence/database-describe-fidelity/after.log
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

# 数据库单元格声明与实际类型混淆：修复

## 支持的结论

旧 PRAGMA 六列声明 NULL，但实际值有 BIGINT/TEXT；四个回归确认丢值、同列动态值丢失、缺失 cell 默 Null、非有限数默 Null。修复读取 raw runtime type/is_null，保原 typed decoder并明确 ExecutionFailed。

## 来源与范围

数据库同一组四项旧反例失败，新六项全部通过，含原分页/readonly两项。
exact命令与原始输出在 docs/evidence/database-describe-fidelity/receipt.json 和对应日志。两个失败fixture尝试分别标为编译失败及三行为失败/一URL错误，不充当最终四行为红证据。before/after源码可逆base64和decoded SHA保留，原20-file清单不改写。

## 已知缺口

package warnings lint、四项生产panic lint与fmt均已通过；独立源码复审0问题。公开revision、完整门禁、新Host三语言和精确main尚未证明。此 Evidence 不提升 broad Tool/permission 审查状态。

历史观察定位：原 source:782042b6c310f1e2cde73ccc5adf5039a8aa50e95bd1e264900d25f9ca4b19da 的930个非语义文件已精确保存为内部可恢复 Git 52eb905fe03711151508d1f439f87a863edaa839，源码摘要重算相同；真实 HEAD/index/branch 未变化。此 Git 对象只保存原DB/RAG观察，不证明后续ClinicalTrials增量或公开交付。
