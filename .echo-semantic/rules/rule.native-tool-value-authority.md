---
schema_version: 1
id: rule.native-tool-value-authority
kind: rule
status: needs_review
expectation: human_confirmed
risk: medium
primary_focus: state_authority
focus:
- contract_evidence
- result_side_effect
observed_at: source:07caadeefd3db109af0778911e6c3fe16539c3eca9ca95baddc2ed1a59a489d8
behavior_refs:
- behavior.native-tool-value-fidelity
code_refs:
- echo-tools/src/database.rs
- echo-tools/src/rag.rs
- echo-tools/src/registry.rs
- docs/en/02-tools.md
- docs/zh/02-tools.md
- docs/en/42-database-tools.md
- docs/zh/42-database-tools.md
- echo-tools/src/research/clinical_trials.rs
- docs/en/22-research-tools.md
- docs/zh/22-research-tools.md
evidence_refs:
- evidence.database-runtime-cell-repair
- evidence.database-runtime-cell-verification
- evidence.rag-preview-unicode-repair
- evidence.rag-preview-unicode-verification
- evidence.clinical-trials-total-repair
- evidence.clinical-trials-total-verification
- evidence.native-tool-value-full-gates
- evidence.chart-dimension-fidelity
finding_refs:
- finding.database-runtime-cell-fidelity
- finding.rag-preview-unicode-count
- finding.clinical-trials-total-fidelity
- finding.chart-dimension-fidelity
---

# 原生 Tool 值的唯一权威

## 不变量或唯一权威

数据库每个单元格的值、类型和 SQL NULL 状态由原 SQLx runtime value 决定，column declaration 仅为 metadata。RAG 文本 chunk 与预览采用既有 Unicode 标量迭代；char_count/truncated 不另用 UTF-8 byte 定义。

## 适用行为

现有数据库 Tool 共享 decoder，以及已有 rag_chunk_document 预览字段。SDK 仅调用框架，不能复制 SQL parser 或 RAG chunk 算法。

## 当前实现

try_get_raw 失败明确报错，实际 SQL NULL 才允许 JSON null；actual type 决定原 fallible typed decoder。FLOAT 无法表示为 JSON Number 时失败。原 chunk_text 与 chars().take(200) 保持，metadata 共享 chars().count()。

## 期望行为

保持整数边界、UTF-8 文本、finite float 和原 BLOB base64对象；不因缺失值填默认。这里只记录已存在的 authority 约束，不新增 state/store/API 或架构权威。

## 证据

精确旧红/新绿、原始 SQLx metadata/actual type 诊断、文件 bytes 比较、源码与双语文档摘要、独立复审已提供。PG/MySQL native server、最终新 Host 和主线交付另验。

## 裁决记录

使用原 framework converter 和 chunker，未引入通用 cache、新 backend、协议或新的跨语言接受字段。完整候选门禁尚待，故不把规则提升为全面 verified。


当前ClinicalTrials增量：total_results读取provider已存在totalCount，原GET明确请求countTotal=true；缺失/非整数不能默认0。当前第三项独立复审未齐，Finding保持open，原DB/RAG源码审查和focused证据仍定位可恢复52eb旧source，不扩为三项审查。


## 限定当前验证范围

本对象只对数据库actualcell/null/error转换、RAGscalar metadata和Clinical既有total字段三个列出承诺verified；23完整门禁及独立最终源码/工程复审已通过。前文needs_review/open记载属于focused阶段，当前三个具体Finding已local三链resolved。未验证PG/MySQL真实服务、全部Tools或Research providers；公开main、新Host三语言及官网依旧独立待验。DB/RAG原52eb审阅保持历史，不刷新为三项。

Chart范围的当前源码观察和局部候选只覆盖上述新场景；旧宽范围证据不冒本轮验证。
