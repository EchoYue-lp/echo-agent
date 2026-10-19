---
schema_version: 1
id: baseline.repository
kind: baseline
source_snapshot:
  base_revision: f3fa0705f62fe813a5a86bc41cd673f82f915a61
  content_digest: 1828e264adb8119b8b19c84ccc0e18b35067b6333f89c775e51856563290659e
inventory_closure: closed
behavior_model_closure: closed
map_refs:
- map.workspace-architecture
- map.agent-session-turn
- map.context-memory
- map.task-subagent-workflow
- map.observation-persistence-delivery
- map.tool-permission-sandbox
- map.extension-lifecycle
- map.llm-provider-runtime
- map.protocol-surfaces
- map.eval-evolution
- map.sdk-facade-parity
regions:
- path: .agents
  status: supporting
- path: .cargo
  status: supporting
- path: .example.env
  status: supporting
- path: .github
  status: supporting
- path: .gitattributes
  status: supporting
- path: .gitignore
  status: supporting
- path: AGENTS.md
  status: supporting
- path: AUDIT_REPORT.md
  status: supporting
- path: CHANGELOG.md
  status: supporting
- path: CONTRIBUTING.md
  status: supporting
- path: Cargo.lock
  status: supporting
- path: Cargo.toml
  status: in_scope
- path: LICENSE
  status: supporting
- path: README.md
  status: supporting
- path: README.zh.md
  status: supporting
- path: benches
  status: supporting
- path: deny.toml
  status: supporting
- path: docs
  status: supporting
- path: echo-agent-learning
  status: supporting
- path: echo-core
  status: in_scope
- path: echo-execution
  status: in_scope
- path: echo-integration
  status: in_scope
- path: echo-macros
  status: in_scope
- path: echo-orchestration
  status: in_scope
- path: echo-state
  status: in_scope
- path: echo-tools
  status: in_scope
- path: mcp.json.example
  status: supporting
- path: rust-toolchain.toml
  status: supporting
- path: scripts
  status: supporting
- path: src
  status: in_scope
- path: tests
  status: supporting
- path: vendor
  status: generated_or_vendor
boundaries:
- id: boundary.workspace-architecture
  map_ref: map.workspace-architecture
  risk: high
- id: boundary.agent-session-turn
  map_ref: map.agent-session-turn
  risk: high
- id: boundary.context-memory
  map_ref: map.context-memory
  risk: high
- id: boundary.task-subagent-workflow
  map_ref: map.task-subagent-workflow
  risk: high
- id: boundary.observation-persistence-delivery
  map_ref: map.observation-persistence-delivery
  risk: high
- id: boundary.tool-permission-sandbox
  map_ref: map.tool-permission-sandbox
  risk: high
- id: boundary.extension-lifecycle
  map_ref: map.extension-lifecycle
  risk: high
- id: boundary.llm-provider-runtime
  map_ref: map.llm-provider-runtime
  risk: high
- id: boundary.protocol-surfaces
  map_ref: map.protocol-surfaces
  risk: high
- id: boundary.eval-evolution
  map_ref: map.eval-evolution
  risk: high
- id: boundary.sdk-facade-parity
  map_ref: map.sdk-facade-parity
  risk: high
coverage:
- region: Cargo.toml
  lens: trigger_input
  status: covered
  refs: &id001
  - map.workspace-architecture
- region: Cargo.toml
  lens: result_side_effect
  status: covered
  refs: *id001
- region: Cargo.toml
  lens: state_authority
  status: covered
  refs: *id001
- region: Cargo.toml
  lens: data_durability
  status: covered
  refs: *id001
- region: Cargo.toml
  lens: time_lifecycle
  status: covered
  refs: *id001
- region: Cargo.toml
  lens: failure_concurrency
  status: covered
  refs: *id001
- region: Cargo.toml
  lens: permission_external
  status: covered
  refs: *id001
- region: Cargo.toml
  lens: contract_evidence
  status: covered
  refs: *id001
- region: echo-core
  lens: trigger_input
  status: covered
  refs: &id002
  - map.workspace-architecture
  - map.agent-session-turn
  - map.context-memory
  - map.observation-persistence-delivery
  - map.tool-permission-sandbox
  - map.extension-lifecycle
  - map.llm-provider-runtime
- region: echo-core
  lens: result_side_effect
  status: covered
  refs: *id002
- region: echo-core
  lens: state_authority
  status: covered
  refs: *id002
- region: echo-core
  lens: data_durability
  status: covered
  refs: *id002
- region: echo-core
  lens: time_lifecycle
  status: covered
  refs: *id002
- region: echo-core
  lens: failure_concurrency
  status: covered
  refs: *id002
- region: echo-core
  lens: permission_external
  status: covered
  refs: *id002
- region: echo-core
  lens: contract_evidence
  status: covered
  refs: *id002
- region: echo-execution
  lens: trigger_input
  status: covered
  refs: &id003
  - map.tool-permission-sandbox
  - map.extension-lifecycle
- region: echo-execution
  lens: result_side_effect
  status: covered
  refs: *id003
- region: echo-execution
  lens: state_authority
  status: covered
  refs: *id003
- region: echo-execution
  lens: data_durability
  status: covered
  refs: *id003
- region: echo-execution
  lens: time_lifecycle
  status: covered
  refs: *id003
- region: echo-execution
  lens: failure_concurrency
  status: covered
  refs: *id003
- region: echo-execution
  lens: permission_external
  status: covered
  refs: *id003
- region: echo-execution
  lens: contract_evidence
  status: covered
  refs: *id003
- region: echo-integration
  lens: trigger_input
  status: covered
  refs: &id004
  - map.extension-lifecycle
  - map.llm-provider-runtime
  - map.protocol-surfaces
- region: echo-integration
  lens: result_side_effect
  status: covered
  refs: *id004
- region: echo-integration
  lens: state_authority
  status: covered
  refs: *id004
- region: echo-integration
  lens: data_durability
  status: covered
  refs: *id004
- region: echo-integration
  lens: time_lifecycle
  status: covered
  refs: *id004
- region: echo-integration
  lens: failure_concurrency
  status: covered
  refs: *id004
- region: echo-integration
  lens: permission_external
  status: covered
  refs: *id004
- region: echo-integration
  lens: contract_evidence
  status: covered
  refs: *id004
- region: echo-macros
  lens: trigger_input
  status: covered
  refs: &id005
  - map.workspace-architecture
  - map.tool-permission-sandbox
  - map.extension-lifecycle
- region: echo-macros
  lens: result_side_effect
  status: covered
  refs: *id005
- region: echo-macros
  lens: state_authority
  status: covered
  refs: *id005
- region: echo-macros
  lens: data_durability
  status: covered
  refs: *id005
- region: echo-macros
  lens: time_lifecycle
  status: covered
  refs: *id005
- region: echo-macros
  lens: failure_concurrency
  status: covered
  refs: *id005
- region: echo-macros
  lens: permission_external
  status: covered
  refs: *id005
- region: echo-macros
  lens: contract_evidence
  status: covered
  refs: *id005
- region: echo-orchestration
  lens: trigger_input
  status: covered
  refs: &id006
  - map.agent-session-turn
  - map.task-subagent-workflow
  - map.observation-persistence-delivery
  - map.tool-permission-sandbox
- region: echo-orchestration
  lens: result_side_effect
  status: covered
  refs: *id006
- region: echo-orchestration
  lens: state_authority
  status: covered
  refs: *id006
- region: echo-orchestration
  lens: data_durability
  status: covered
  refs: *id006
- region: echo-orchestration
  lens: time_lifecycle
  status: covered
  refs: *id006
- region: echo-orchestration
  lens: failure_concurrency
  status: covered
  refs: *id006
- region: echo-orchestration
  lens: permission_external
  status: covered
  refs: *id006
- region: echo-orchestration
  lens: contract_evidence
  status: covered
  refs: *id006
- region: echo-state
  lens: trigger_input
  status: covered
  refs: &id007
  - map.context-memory
  - map.observation-persistence-delivery
  - map.eval-evolution
- region: echo-state
  lens: result_side_effect
  status: covered
  refs: *id007
- region: echo-state
  lens: state_authority
  status: covered
  refs: *id007
- region: echo-state
  lens: data_durability
  status: covered
  refs: *id007
- region: echo-state
  lens: time_lifecycle
  status: covered
  refs: *id007
- region: echo-state
  lens: failure_concurrency
  status: covered
  refs: *id007
- region: echo-state
  lens: permission_external
  status: covered
  refs: *id007
- region: echo-state
  lens: contract_evidence
  status: covered
  refs: *id007
- region: echo-tools
  lens: trigger_input
  status: covered
  refs: &id008
  - map.tool-permission-sandbox
- region: echo-tools
  lens: result_side_effect
  status: covered
  refs: *id008
- region: echo-tools
  lens: state_authority
  status: covered
  refs: *id008
- region: echo-tools
  lens: data_durability
  status: covered
  refs: *id008
- region: echo-tools
  lens: time_lifecycle
  status: covered
  refs: *id008
- region: echo-tools
  lens: failure_concurrency
  status: covered
  refs: *id008
- region: echo-tools
  lens: permission_external
  status: covered
  refs: *id008
- region: echo-tools
  lens: contract_evidence
  status: covered
  refs: *id008
- region: src
  lens: trigger_input
  status: covered
  refs: &id009
  - map.agent-session-turn
  - map.context-memory
  - map.task-subagent-workflow
  - map.observation-persistence-delivery
  - map.tool-permission-sandbox
  - map.extension-lifecycle
  - map.llm-provider-runtime
  - map.protocol-surfaces
  - map.eval-evolution
- region: src
  lens: result_side_effect
  status: covered
  refs: *id009
- region: src
  lens: state_authority
  status: covered
  refs: *id009
- region: src
  lens: data_durability
  status: covered
  refs: *id009
- region: src
  lens: time_lifecycle
  status: covered
  refs: *id009
- region: src
  lens: failure_concurrency
  status: covered
  refs: *id009
- region: src
  lens: permission_external
  status: covered
  refs: *id009
- region: src
  lens: contract_evidence
  status: covered
  refs: *id009
---






# echo-agent 全 Workspace 语义基线

## 源码快照

本候选基线以本地任务启动 `95ec66c6083a8feb17a1c28de0d01e1a24ca8a28` 为可恢复祖先，绑定当前非语义源码摘要；任务分支的 merge commit 不作为目标主线祖先。`.echo-semantic` 自身不参与摘要。后续业务、合同、测试或正式文档变化必须通过 semantic-diff 刷新。

## 仓库区域

9 个 framework production 顶层区域进入 `in_scope`；tests、examples、docs、CI、scripts、
learning 和 repository metadata 是 supporting consumers。已迁出的 SDK 产品由独立仓库
承接，不再属于当前 framework workspace 区域。

## 能力图与边界

十张顶层 Capability Map 覆盖架构、Agent/Turn、Context、Task/Subagent/Workflow、Observation/Persistence、Tool/Permission、Extensions、LLM、Protocols 与 Eval/Evolution；现有 SDK facade map 作为 protocol 子图保留。

## 覆盖网格

每个 in-scope 区域的八个风险视角都有对应 map；`mapped` 包括正常路径和已记录 Finding，证据不足或需裁决的场景保留 `needs_review`/`unresolved`。

## 未知与缺口

当前 open Findings 与 Discovery unknown 是后续 high-risk audit frontier。闭合不表示缺陷已解决，也不表示所有动态 consumer 或外部环境已经运行验证。

## 闭合结论

路径库存和行为模型在当前快照上闭合：所有能力、已知问题和未知区均有可追踪处置。后续工作按 Finding 与风险边界推进，不按 SDK identity 数量推进。

当前 Task runtime/ACP lease 修复已映射并具有完整本地工程门禁。库存/模型的 closed 只表示场景有明确处置，不表示所有 Behavior verified；其它 needs_review/Open Finding、消费者、远端 main 验收仍保持各自证据边界。既有修复Finding的本地resolved与其历史工程/独立复审链保持；公开pin、消费者和main继续独立验收。


本轮新增 Guard 重叠区间、Pivot typed/null/error 与 Channel owned 操作已归原能力图，各自有修复、focused 验证和独立局部复审证据。当前模型观察绑定 source:34d5f5574c9cf5a645970cd8b58367c39bce268630488b68d3ea54e996728e17；历史 ecac/b0d162/8896 Evidence 保持可恢复 Git 和原限制。新版完整工程门禁、发布后的消费者与精确 main 仍独立待验，不以模型 closed 代替。


当前完整工程门禁补齐：evidence.framework-final-34d5-full-verification 绑定当前34d5源码、23exit0/warnings0、3471/0/3ignored/86targets/59examples；evidence.framework-final-34d5-gate-archive 保存实际47原文件。库存与模型closed含明确未知处置，依然不等于全部Behavior verified。SDK/remote main等独立验收保持。


当前候选最终独立实现复审 audit.framework-final-34d5-gate-rereview 已PASS/action0，完整源码+新23receipt归档闭合，允许进入用户授权的framework里程碑交付；交付尚须实际PR/CI/main证据。Planready与其它开放义务仍保持。


本次依赖策略增量：旧34d5门禁/复审已保留为真实4eb Git历史；新源码含6个Cargo文件和依赖修复原始证据，绑定77de6ca90ad842eb4b7b2921a4baa954f41cccfe。依赖Finding与新增场景归原workspace map，focused459及审计red/green不替代新完整门禁/独立审查/远端dependency-policy CI。


本轮完整工程结果：依赖修复新23最终全部通过，第一次baselineancestor错误导致的真实test101及修复同命令attempt2通过均保留 evidence.framework-dependency-final-gate-archive。semantic写完后需针对baseline3再验；旧34d5 Gate/Audit只历史定位，新依赖源码局部review/工程绿不替代远端main或SDK消费者。


Channel handler forward当前模型已归原extension/session边界与开放Finding；源码摘要包含本轮正式docs和原始evidencearchive，新retire/open23/80及full23/SDK/官网/main仍待，不借前23或历史79声明closed。


Channel invocation本地三链现已闭合：same source d056完整23/current23/80回归及独立源码/receipt审查通过。仅新有限Behavior/Rule verified、具体Finding resolved，SDK/官网/目标main仍独立待交付，旧宽对象不由此verified。


## Database、RAG与Clinical本轮范围

当前D候选绑定source:1d52da61582a363ec3a3468772f2b789e76b4e043e7797e85f897411361a22fd，可恢复主线祖先仍95ec66c6083a8feb17a1c28de0d01e1a24ca8a28。DB/RAG原focused与独立复审保持真实52eb905fe03711151508d1f439f87a863edaa839（source782）历史，不扩大至Clinical。Clinical新增URL countTotal及响应totalCount错误合同有独立原始old/new证据，当前Finding仍open等新复审。新3场景归原Tool map及限定native value Behavior/Rule；原宽对象未提升状态，未新增公共API/状态权威或接受新语言identity。

原N11对象原source:d056精确等已发布554f，全部引用逐blob同，已改为历史Git定位保留原审查范围/时间/状态。614项任务范围校验错误来自installed0.5对历史SDK引用直接读当前树的限制，原始命令及stderr见evidence.native-value-semantic-history-reader-limit；不删引用或恢复迁出的SDK。全项目strict snapshot/change-evidence按真实历史Git继续执行；该限制不代替新完整23、baseline native3、独立复审、SDK消费者或精确main验收。

## 本次Git修复的精确源码快照维护

原基线 source_snapshot {"base_revision": "b9099558620f5f9693c736c85c214db14f62a8cb", "content_digest": "436a5153c5b7a29a7ea6b81fe972ae5ed368d9dfb6a8f6a0e0687458b2b96d00"} 属原框架已发布阶段。此次只在本GitTool根缺陷及原依赖锁安全补丁/正式归档全部真实提交后计算官方源码摘要；当前观察Git 2c9f1e41fcf1350337157a8bd80c304b1b0242a4 的source_digest_at_revision与工作树source_digest严格相同 399c6b119b30d0b29a8996c97bc074a8de36509adaa67fb3141e6eae1884fa28。历史Evidence/Audit各自observed与原执行时间不批量换成当前；新Git具体对象/三链及原a193六门禁收据分别保原范围，后继security锁/门禁有自己的原始证据。

本次没有新增公共API、第二Git权威、运行时模型或强force行为。原区域分类与两个closure值原样保持；GitBranch四场景specific verified不扩大原DB/RAG或全部平台/SDK消费者，remote/main交付仍由对应实际结果验收。机器任务基准按真实fresh preflight当前HEAD验证；a634起点的完整已提交差异另在有界semantic-diff及连续性报告保全，不把最终任务机器base伪说成原a634全检查通过。

## 发布谱系基准与精确源码观察

source_snapshot.base_revision 使用已经发布的共同祖先 a634，仅承担可恢复谱系与祖先校验；它不声称摘要399c6b119b30d0b29a8996c97bc074a8de36509adaa67fb3141e6eae1884fa28就是a634旧tree。content_digest仍由官方方法对当前所有非semantic文件计算，并与保存完整原始包的真实2c9 Git tree完全相同；仅后继语义元提交的afa不会改变这个源码摘要。机器 --base 当前任务预检基准与这个发布谱系字段职责不同。

前述2c9是实际源码/归档观察切点，原Evidence/Audit各自046/a193/141运行来源不改；Root将保留可远端获取的provenance标签确保squash后来源仍可回放，未在本段宣称标签已经创建。该一字段修正让最终squash主线以a634为祖先，避免把未合入任务commit当main祖先。源码、工程输入、覆盖与两closure状态均未更改。

## Physical settlement B focused source checkpoint

The new content digest is the canonical digest of the frozen B source and maintained documents. The existing published lineage base remains unchanged. Historical Git observations above and their source digests retain their original epoch; no old Evidence/Audit revision is moved to this candidate. Current focused ownership tests and independent review are recorded by evidence.subagent-physical-settlement-repair and audit.subagent-physical-settlement-focused-review. This snapshot maintenance does not claim six merge gates, feature matrix, consumer qualification, main CI, or new global behavioral closure.

The subsequent ACP close increment has canonical source digest 951cf5e91f3d0fb879bda5e1960e21750accb35acdc3246c14fb6a900b53225e. The preceding focused source digest 3fd9e1ed897ce9ee4088530ea89cde93193cea1eb1d5a1afcd612ab083e2deb7 remains recoverable in pushed commit 87223c8163f6b30479fbd44eadd09736db50bd5d. Full gates/consumer/main remain incomplete.

## Compacted-context and canonical-root focused checkpoint

Current canonical source digest is b2639597dfa94b7e16aa312e46db6585d9df7636d6340c004ae978f18711d0f1, computed by the unchanged installed
0.5 source_digest function after the source/docs freeze. Published lineage base
f3fa remains unchanged; actual machine preflight/change-evidence base is f990.
The preceding f990 tree independently hashes to 6e61249f5796e67d8daa2038c750f8452c9c6d41f5e14dcda791405fd2db5d06, so its earlier model
and token-limit evidence retains that historical source identity and scope.

Two new, distinct framework Findings close only the retained-ordinal and
canonical-root occurrence/value local focused invariants, with one bounded Evidence and two
current challenges in the existing Context-memory Audit. Twelve distinct tests
(ten new, two existing controls), the fixture-only tail rerun, focused Clippy and
fmt are actual; no old suite total is used as new proof. Broad Behavior statuses,
inventory/behavior closure values and historical challenges are unchanged.

Raw/copied sources/binaries/PF stay ignored. Old task-mode historical SDK reads
and 512-closure limits retain their existing disposition; a full historical
strict-snapshot/change-evidence run at actual f990 base is separate from native
Stop, cumulative f3fa qualification, full six gates/17 features, new SDK Host
aggregate/schema3 acceptance, exact-main CI and required website mirror sync.
F41/ADR0056 join the already planned F38/ADR0047 official website sync after exact
framework main; examples have no changed source-specific call site and remain
covered by the later required workspace all-target gate.

## Configured-client construction focused extension

The current canonical source digest is 1828e264adb8119b8b19c84ccc0e18b35067b6333f89c775e51856563290659e; actual preflight base is674f2b41.
The previous source digest b263 remains attached to that exact Git checkpoint
for its projection/root qualification. No earlier closure flags, broad Behavior
status or historical test total is promoted. The new bounded Evidence records
10 native tests (three new, seven existing) and one executable public Rustdoc,
plus focused Clippy/fmt; two earlier fixture failures remain separately recorded.
One Rust public constructor is not automatically a new SDK remote contract.
Consumer sharing/Host pin, full6+17, exact main CI, website mirrors and native
Stop remain their original separate obligations.
