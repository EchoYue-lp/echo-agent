---
schema_version: 1
id: behavior.native-tool-value-fidelity
kind: behavior
status: verified
expectation: human_confirmed
risk: medium
primary_focus: contract_evidence
focus:
- trigger_input
- result_side_effect
- state_authority
boundary: boundary.tool-permission-sandbox
observed_at: source:1d52da61582a363ec3a3468772f2b789e76b4e043e7797e85f897411361a22fd
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
rule_refs:
- rule.native-tool-value-authority
evidence_refs:
- evidence.database-runtime-cell-repair
- evidence.database-runtime-cell-verification
- evidence.rag-preview-unicode-repair
- evidence.rag-preview-unicode-verification
- evidence.clinical-trials-total-repair
- evidence.clinical-trials-total-verification
- evidence.native-tool-value-full-gates
finding_refs:
- finding.database-runtime-cell-fidelity
- finding.rag-preview-unicode-count
- finding.clinical-trials-total-fidelity
---

# 原生数据库值与 RAG 字符投影

## 重要承诺

现有已接受的 database 和 RAG Tool 应返回真实原生值，不能以列声明、字节长度或默认 null 替代实际值；这不是扩展 Rust identity 的跨语言接受范围。

## 当前行为

数据库统一由 format_db_rows/decode_db_cell 读取原 SQLx AnyRow 单元格实际类型和空值状态；RagChunkDocumentTool 的 char_count 与 truncated 读取既有 scalar chunk/preview 的相同字符定义。

## 期望行为

PRAGMA 的未知声明不吞掉整数和文本；同列跨行类型变化保留；SQL NULL 与缺失单元格明确区分；非有限数失败可见。Unicode metadata 与实际 200 标量预览一致。

## 触发、结果与副作用

原注册的 describe_table/sql_query/list_tables 及 rag_chunk_document 入口；不新增路由。真实 describe Tool 的 SQLite 文件调用前后 bytes 相同；RAG preview 不写索引。此证据不扩大为所有 SQL 查询绝无副作用的形式化保证。

## 失败、重试与恢复

缺值/解码失败/非有限数由原 ToolError::ExecutionFailed 传播，不填成功 null。RAG 空文本与非法分块大小原行为不变；无新增恢复/取消/重试权威。

## 证据

原实现六项反例失败，新实现数据库六项（含原两项）和 RAG 两项通过；focused formatter/lint 和独立源码复审通过。各自归档与三链详见引用。

## 裁决记录

主协调者已明确授权两项既有行为修复；独立源码 PASS 不代替完整门禁、公开 revision、新 Host 三语言和精确 main 验收。因此本能力仍 needs_review。


当前ClinicalTrials增量：total_results读取provider已存在totalCount，原GET明确请求countTotal=true；缺失/非整数不能默认0。当前第三项独立复审未齐，Finding保持open，原DB/RAG源码审查和focused证据仍定位可恢复52eb旧source，不扩为三项审查。


## 限定当前验证范围

本对象只对数据库actualcell/null/error转换、RAGscalar metadata和Clinical既有total字段三个列出承诺verified；23完整门禁及独立最终源码/工程复审已通过。前文needs_review/open记载属于focused阶段，当前三个具体Finding已local三链resolved。未验证PG/MySQL真实服务、全部Tools或Research providers；公开main、新Host三语言及官网依旧独立待验。DB/RAG原52eb审阅保持历史，不刷新为三项。
