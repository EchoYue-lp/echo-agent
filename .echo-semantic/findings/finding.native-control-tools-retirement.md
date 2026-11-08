---
schema_version: 1
id: finding.native-control-tools-retirement
kind: finding
type: consolidation_candidate
status: resolved
severity: high
primary_focus: contract_evidence
focus:
- state_authority
- time_lifecycle
- failure_concurrency
- data_durability
boundary_ref: boundary.agent-session-turn
behavior_refs:
- behavior.agent-turn-lifecycle
rule_refs:
- rule.turn-terminal-authority
evidence_refs:
- evidence.native-control-retirement-observations
- evidence.native-control-retirement-before-source
- evidence.native-control-retained-contracts-equivalence
audit_refs:
- audit.native-control-tools-retirement-rereview
decision_refs: []
repair_evidence_refs:
- evidence.native-control-retirement-observations
verification_evidence_refs:
- evidence.native-control-retained-contracts-equivalence
rereview_audit_refs:
- audit.native-control-tools-retirement-rereview
discovered_at: 0e404185c8f252da3e335c35547e52249c74dc65
candidate_asset_refs:
- asset.legacy-native-control-tools
- asset.agent-execution-contract
- asset.approved-a2a-lsp-source-retirement
canonical_asset_ref: asset.agent-execution-contract
replacement_refs:
- asset.agent-execution-contract
delete_paths:
- echo-agent-learning/examples/demo23_a2a.rs
- echo-agent-learning/tests/example_contracts/demo55_lsp_tools.rs
- echo-core/src/lsp/client.rs
- echo-core/src/lsp/mod.rs
- echo-core/src/lsp/types.rs
- echo-integration/src/lsp/client.rs
- echo-integration/src/lsp/config.rs
- echo-integration/src/lsp/jsonrpc.rs
- echo-integration/src/lsp/manager.rs
- echo-integration/src/lsp/mod.rs
- src/a2a/auth.rs
- src/a2a/client.rs
- src/a2a/mod.rs
- src/a2a/serve.rs
- src/a2a/server.rs
- src/a2a/types.rs
- src/lsp.rs
- src/tools/builtin/answer.rs
- src/tools/builtin/think.rs
- src/tools/lsp.rs
rollback_ref: 0e404185c8f252da3e335c35547e52249c74dc65
decision: retire
---

# 两内置控制工具退役的项目删除证明

## 问题

原完整native任务缺少真实删除身份与repairRefs，两个已批准退出的控制包装未获得正式
语义关联。现有consolidation_candidate/decision:retire支持该范围，不重复请求删除批准。

## 触发条件与影响

源树移除src/tools/builtin/think.rs与answer.rs，原默认只有FinalAnswer注册，Think从未
默认注册，answer原为pub(crate)。旧名称终态、critic耗尽接受、one-shot Stop及新native/
恢复/冷重载增强均是有意退出或修正；把它们误写全等价会掩盖真实合同变化。

## 证据

before普通Evidence记录Git `0e404185c8f252da3e335c35547e52249c74dc65` 的17源/OID/SHA和行锚，无before执行票。
after普通观察保43native/407、17custom、22pending、30File、104Provider和默认7原票。
限定Eq只比较六个共同不变量，14观察条目/11去重源逐SHA等各原运行input；原profile/
9或17个完整输入差异均保留。实际专项Audit记录独立内容pass与已纠正的计数Minor。
完整归档见`/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/.echo-semantic/local/parallel-executors-20261027/framework/native-control-retirement-proof-candidate-20261028`，不是空等价占位。

## 处理记录

ADR0101和Root本轮metadata-only准入承接既有具名退役授权；canonical继续是
asset.agent-execution-contract，不新建执行权威。六对象按A/B正常record落位，原完整
owner/task/base/refs/evidence/保护与99+4路径保留。原open/null阶段保真归档；本轮已用实际官方task值绑定，并取得六合同与正式正文
专项独审C0/I0/M0，仅本Finding在Root准入内resolved并走同一完整任务normal repair-ref。
第10项外部聚合限制和旧402不在此关闭，不改其它Asset状态、unknown或风险。


## 2026-10-28 当前保存切点的源码删除准入补充

本Finding在当前task-scope校验中只承接既有审批的20个具名源码删除准入：原2控制工具
与已正式采用的A2A/LSP专用18源。decision仍为retire，不把A2A/LSP视作与ReAct功能
等价，不扩删其它框架API。新增历史asset逐路径绑定0e4 Git blob/SHA；原2工具asset、
canonical执行合同与其它unknown/needs_review状态保持。

Root既有完整30源码采用/Frame15文档准入、ADR0090及ADR0101提供审批来源；原
whole-source当前测试/Clippy、受控metadata1442、保留feature与退役feature拒绝原件
均按各自真实输入留存。verification Evidence仍只核六个共同保留不变量，不把20项
退出面或新NoCompat取消的旧兼容义务写为matched。

扩展delete_paths与candidate_asset_refs只是修复本task在多个具名删除面合流后的
源码准入映射；不授权未完成的SDK pin/native/MainCI，不宣runtime全等或Source通过。
人类维护优先级暂停产品迭代，旧compat-only后继实际删除仍未实施。
