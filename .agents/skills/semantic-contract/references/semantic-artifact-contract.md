# 语义材料合同

- 合同标识：`echo-semantic-artifacts`
- 合同版本：`1`
- 载体：带 YAML 前置元数据的 Markdown

该合同只规定确定性结构。源码与机器契约说明当前行为；人确认的期望说明应该如何；审查只说明在指定源码版本上
检查过哪些故障假设。三者不得互相替代。

## 目录

```text
semantic/
├── README.md
├── baseline.md
├── maps/
├── behaviors/
├── rules/
├── evidence/
├── findings/
├── audits/
└── discovery/
```

除 `README.md` 与 `baseline.md` 外，每个 Markdown 文件只保存一个对象，文件名必须等于对象 `id`。

## 通用字段

所有对象必须包含：

```yaml
schema_version: 1
id: <稳定标识>
kind: <对象类型>
```

稳定标识使用小写字母、数字、点和连字符。引用字段使用对象标识，不复制被引用对象正文。

源码版本使用以下任一形式：

- 40 位 Git 提交；
- `source:<64 位 sha256>`，表示包含未提交变化的内容快照。

## 基线

`semantic/baseline.md`：

```yaml
schema_version: 1
id: baseline.repository
kind: baseline
source_snapshot:
  base_revision: <40 位 Git 提交>
  content_digest: <64 位 sha256>
inventory_closure: open
behavior_model_closure: open
map_refs: []
regions: []
boundaries: []
coverage: []
```

正文必须包含：`源码快照`、`仓库区域`、`能力图与边界`、`覆盖网格`、`未知与缺口`、`闭合结论`。

区域条目必须有 `path`、`status`。状态为 `in_scope`、`supporting`、`generated_or_vendor` 或 `excluded`；
`excluded` 还必须有 `reason`、`risk`、`recheck_when`。

边界条目必须有 `id`、`map_ref`、`risk`。覆盖条目必须有 `region`、`lens`、`status`；状态为 `covered`、
`not_applicable`、`needs_review` 或 `excluded`，并分别提供对象引用、理由或未知说明。

## 能力图

`semantic/maps/<id>.md`：

```yaml
schema_version: 1
id: map.example
kind: capability_map
title: 示例能力
risk: high
observed_at: <源码版本>
boundary_refs: []
behavior_refs: []
rule_refs: []
evidence_refs: []
finding_refs: []
audit_refs: []
related_map_refs: []
scenarios: {}
```

正文必须包含：`能力范围`、`入口与输出`、`行为关系`、`状态与数据流`、`策略来源与优先级`、
`生命周期与失败路径`、`权限与敏感信息`、`用户侧投影`、`场景处置清单`、`未展开项`。

场景状态只能是 `mapped`、`needs_review`、`excluded`。每个场景都必须有非空 `source_refs`：

- `mapped` 至少引用一个行为承诺、规则、证据或问题记录；
- `needs_review` 必须有 `unknown` 和 `next_step`；
- `excluded` 必须有 `reason`、`risk` 和 `recheck_when`。

## 行为承诺

`semantic/behaviors/<id>.md`：

```yaml
schema_version: 1
id: behavior.example
kind: behavior
status: needs_review
expectation: inferred
risk: high
primary_focus: lifecycle
focus: []
boundary: boundary.example
observed_at: <源码版本>
code_refs: []
rule_refs: []
evidence_refs: []
finding_refs: []
```

正文必须包含：`重要承诺`、`当前行为`、`期望行为`、`触发、结果与副作用`、`失败、重试与恢复`、
`证据`、`裁决记录`。

`status` 使用 `needs_review`、`verified`、`stale`；`expectation` 使用 `unknown`、`inferred`、
`human_confirmed`。

## 规则

`semantic/rules/<id>.md`：

```yaml
schema_version: 1
id: rule.example
kind: rule
status: needs_review
expectation: inferred
risk: high
primary_focus: state_authority
focus: []
observed_at: <源码版本>
behavior_refs: []
code_refs: []
evidence_refs: []
finding_refs: []
```

正文必须包含：`不变量或唯一权威`、`适用行为`、`当前实现`、`期望行为`、`证据`、`裁决记录`。

## 证据

`semantic/evidence/<id>.md`：

```yaml
schema_version: 1
id: evidence.example
kind: evidence
observed_at: <源码版本>
source_refs: []
supports: []
limitations: []
```

正文必须包含：`支持的结论`、`来源与范围`、`已知缺口`。简单且不复用的证据应内联在其所属对象中，
不创建独立文件。

## 问题记录

`semantic/findings/<id>.md`：

```yaml
schema_version: 1
id: finding.example
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: failure_concurrency
focus: []
boundary_ref: boundary.example
behavior_refs: []
rule_refs: []
evidence_refs: []
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: <源码版本>
```

正文必须包含：`问题`、`触发条件与影响`、`证据`、`处理记录`。

`type` 使用 `implementation_bug`、`intent_gap`、`evidence_gap`、`authority_conflict`；`status` 使用
`open`、`resolved`、`risk_accepted`、`false_positive`。

- `implementation_bug` 必须有触发条件、被破坏的不变量或行为、影响和具体证据；
- `resolved` 必须有修复证据、验证证据和复审引用；
- `risk_accepted` 必须有人的裁决引用；
- `false_positive` 必须在正文处理记录中说明原判断为何不成立。

## 审查

`semantic/audits/<id>.md`：

```yaml
schema_version: 1
id: audit.boundary-example.failure-concurrency
kind: audit
boundary_ref: boundary.example
lens: failure_concurrency
freshness: examined
revision: <源码版本>
challenges: {}
finding_refs: []
```

正文必须包含：`审查范围`、`已检查故障假设`、`实际实现路径与证据`、`问题记录`、`残余风险`、`未检查项`。

每个 `challenges` 条目必须有 `revision`、非空 `source_refs` 和 `evidence_refs`。`freshness` 只能是
`examined` 或 `stale`；没有实际审查时不创建空文件。

## 发现过程证据

`semantic/discovery/<id>.md` 使用 `kind: discovery`，并包含 `source_snapshot`、`scope`、`inspected_paths`、
`candidate_refs`、`unresolved`。正文必须包含：`扫描范围`、`候选事实`、`归并结果`、`未决项`。
它只保存指定源码快照的发现证据，不反向定义长期语义。

## 八个风险视角

`lens` 只能使用：

1. `trigger_input`：触发与输入；
2. `result_side_effect`：结果与副作用；
3. `state_authority`：状态与权威；
4. `data_durability`：数据与持久性；
5. `time_lifecycle`：时间与生命周期；
6. `failure_concurrency`：失败与并发；
7. `permission_external`：权限与外部边界；
8. `contract_evidence`：契约与证据。

风险使用 `low`、`medium`、`high`。每个对象只有一个 `primary_focus`，`focus` 可以有多个值。

## 引用与闭合

- `map_refs` 必须与 `maps/` 中有效对象集合一致；
- 一个边界只能归属一个主要能力图；
- 每个行为承诺必须被其边界所属能力图引用；
- `*_refs` 指向的语义对象必须存在且类型正确；
- `inventory_closure: closed` 要求区域分类与八视角覆盖网格没有空白；
- `behavior_model_closure: closed` 要求中高风险能力图没有 `needs_review` 场景、悬空关系或未处置场景。

## 源码快照

源码内容摘要按仓库相对路径排序。每个普通文件依次输入“路径、零字节、`file:`、文件内容 SHA-256、换行”；
每个符号链接依次输入“路径、零字节、`symlink:`、链接目标、换行”，最后计算整体 SHA-256。文件集合是 Git
已跟踪文件与未忽略的未跟踪文件之和，排除 `semantic/`。两个仓库分别计算快照，不能共用一个摘要。

## 禁止结论

任何对象状态都不能使用 `clean`、`safe` 或其它绝对安全词。结构校验通过只证明材料结构，不证明行为正确、
审查充分、测试通过或系统没有缺陷。
