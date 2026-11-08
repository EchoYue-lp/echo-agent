---
schema_version: 1
id: asset.tool-permission-sandbox
kind: asset
title: Tool、Permission 与 Sandbox Effect Pipeline
asset_type: state_authority
status: active
risk: high
observed_at: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
boundary_refs:
- boundary.tool-permission-sandbox
code_refs:
- echo-core/src/tools/mod.rs
- echo-tools/src/security.rs
- echo-core/src/project_rules.rs
- echo-tools/src/files/code_search.rs
- echo-tools/src/files/diff.rs
- echo-tools/src/files/glob.rs
- echo-tools/src/files/grep.rs
- echo-tools/src/files/mod.rs
- echo-tools/src/files/repo_map.rs
- echo-tools/src/git_checkpoint.rs
- echo-core/src/tools/permission.rs
- echo-core/src/tools/artifact.rs
- echo-execution/src/tools.rs
- echo-execution/src/sandbox/resource_owner.rs
- echo-execution/src/sandbox/manager.rs
- echo-execution/src/sandbox/docker.rs
- echo-execution/src/sandbox/k8s.rs
- echo-orchestration/src/human_loop/service.rs
- src/agent/react/run/pipeline.rs
- src/agent/react/mod.rs
- echo-execution/src/skills/hooks.rs
- echo-execution/src/sandbox/local.rs
- echo-tools/src/git_worktree.rs
- echo-tools/src/git.rs
- echo-tools/src/http_body.rs
- echo-core/src/tools/filesystem.rs
- echo-tools/src/filesystem.rs
- echo-tools/src/files/files.rs
- echo-tools/src/files/apply_patch.rs
- src/agent/snapshot.rs
- docs/adr/0098-project-file-system-adapter.md
consumer_refs:
- src/agent/react/subsystems/tool_exec.rs
- echo-tools/src/lib.rs
- echo-tools/src/registry.rs
behavior_refs:
- behavior.effect-permission-execution
- behavior.git-branch-switch-fidelity
rule_refs:
- rule.permission-effect-order
- rule.git-branch-switch-authority
evidence_refs:
- evidence.effects-extensions
- evidence.streaming-tool-validation-repair
- evidence.streaming-tool-validation-verification
- evidence.tool-read-cache-authority-repair
- evidence.tool-read-cache-authority-verification
- evidence.tool-registry-owned-handle-repair
- evidence.tool-registry-owned-handle-verification
- evidence.hook-protected-path-repair
- evidence.hook-protected-path-verification
- evidence.readonly-tool-capability-repair
- evidence.readonly-tool-capability-verification
- evidence.effect-cleanup-owner-repair
- evidence.effect-cleanup-owner-verification
- evidence.git-branch-switch-fidelity
- evidence.source8-tools-current-source-review
finding_refs:
- finding.tool-read-cache-scope
- finding.tool-registry-mutation-active-call-deadlock
- finding.streaming-tool-validation
- finding.plan-mode-write-surface
- finding.approval-authority
- finding.hook-protected-path
- finding.sandbox-minimum-isolation
- finding.guard-direction-contract
- finding.trace-audit-secret-boundary
- finding.effect-cleanup-owner
- finding.git-branch-switch-dirty-path
candidate_refs: []
---

# 当前来源与有限父层接管

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

构造时显式FileRoot由原provider-aware resolver统一校验原实际symlink/..和递归项；runtime cwd仅解析base，未设根继续unconfined。原API FileRoot20 before7通过13失败，当前同20及原filesystem13通过。完整门禁发现原合法相对provider/default-dot回归，原full05真实失败保留；仅Glob入口base_dir.is_some时启原scope validator后，旧default-path四pattern断言在focused及当前whole实际通过。共享resolver/RelativeFiles/断言不改，不新增权限门。Hook5与本父nested owner/MCP局限仍分别说明。

原父风险、完整code/consumer/behavior/rule/finding关系不缩减；以下准备段的needs_review/未取得等字样仅保原时点，当前可消费声明与上述具名修复按本节和机器observed_at。相邻Behavior/Rule、开放Finding、Discovery与真实未知不自动升级。

# 已冻结的r04准备记录原文

# Tool、Permission 与 Sandbox Effect Pipeline

## 资产身份

Tool registry/schema/execution primitive与ReactAgent permission/approval/rewrite/guard、artifact、provider文件目标及resource cleanup的组合边界；保原25条code_refs的完整责任，不把trusted Hook/programmatic caller并成同一产品permission路径。

## 来源与消费者

ReactAgent、Skills/Hooks、Subagent及直接caller继续消费；echo-tools提供具体effect。补录现有FileSystem、实际读/改工具、snapshot和provider consumer边，所有原风险/引用/历史Finding保持。新修复和两cache窗口证据见evidence.source8-tools-current-source-review，不能以旧11resolved替代。

## 生命周期

Register/visibility→PermissionStage绑定授权owned Arc与有效输入→readproof/guard→START→Manager在原registry短锁内同步选择执行Arc+epoch并核授权Arc→原execute/stream→artifact/effect/trace→cancel/cleanup/terminal。START后同名替换不能把新工具挂在旧批准上，已准入旧代可完成而不换源/重审批。

read-before-edit从实际Tool同步source hook取得确认FileRead对应provider+原path，base_dir与snapshot cwd按原target resolver处理，不用host canonicalize解释non-native路径；无source的custom effect保留但不能授写，flag=false不调用新hook，原provider error不回退host。读记录由原Agent持有共享Arc<Mutex<FileReadHistory>>，snapshot引用同一读map，每次prepare清空并保原TTL；不引第二map、validator或store。

缓存lookup在cache锁内核expected epoch，tuple选择持原registration lock覆盖发布/失效窗口；旧A被选中后不得命中B缓存并给B FileRead签A来源，新B发布但旧epoch未失效时也不得读A旧缓存。正常同代B cache仍有效；stream/nonstream、write invalidation、TTL/capacity、resource owner/started-effect原路保留。Glob沿真实遍历和Path components组合**与wildcard，绝对/相对/CurDir/平台prefix分开，不再把*.rs当字面后缀。

Artifact、sandbox、worktree各保原owner到实际结算，未完成cleanup留debt；权限用于Agent自动行为，不给用户直接终端/MCP套full-auto门控。

## 候选关系

PermissionPolicy/PermissionService/Hook是组合层，非平行effect authority；provider adapter只无损传目标身份，不造registry/executor/rollback或自动持久化。

## 未知与限制

原observed_at046674d4775d31ddc2daad7107a8425edb648c8f及旧正文阶段性结论保历史。TP-I2/I3原5同断言RED→r08 GREEN、额外U7/正负控制、glob独立r09十项与R14两cache窗口2pipeline＋2Manager及旧8cache控制分别真实通过；不是四条完整pipeline，更不能相加装完整门禁。R15 Full05自然0仍只保其工程范围，R15仅改旧Permission trace fixture两行身份，生产相对R14不变。

本候选继续needs_review，新增明确阻断是最终父报告I1/I2。已读动态分支须按原条件接管：同一Hook事件的reducer是deny>ask>require>allow；PreToolUse Ask保交互意图，后PermissionRequest Allow不能抹掉；PreToolUse Allow在protected-path后可以签receipt并return，该分支不再发后PermissionRequest，不能概括成全事件Deny恒先于Allow。若要不同产品语义，归原policy owner裁决，不由本材料重排政策。

Hook Subagent timeout会离开观察future，builtin HookSubagentCancelGuard与dispatch_owned继续保同一owner/token/permits/lifecycle至收束，不把drop称physical completion；真实nested owner已追读。Hook MCP timeout只有调用者诊断，stdio pending撤销本地等待、HTTP guard只管本地in-flight/close，均不能证明远端tools/call副作用已取消或完成。远端effect终态、任意custom callback及第三方backend履约保具体限制，不再笼统说整个nested调用链未审。Windows/真实Linux故障、SSH/two-machine、Docker/K8s现场、真实远端、SDK accepted/pin/Host/native、性能仍未验；glob portable/source-only平台判断不能充Windows执行。没有借本提案关闭任何开放Finding或Discovery。

## 原阶段增量记录（历史原文保全，不代表本R15接管范围）

## 现有GitTool分支切换具体闭合

只新增GitBranchTool switch原根缺陷的源码/四场景消费，nativeGit维持唯一repo事实。实际原CLI2/Tool3fail→same4pass与最后保护oracle/源码独审支持具体localresolved。此模型当前observed定位046674保存的a193原source文件/新场景，其他旧正文与对象状态全部保留；不把原DB/RAG verified平移为全部Git，也不扩大原permission/cleanup闭合。最终snapshot/remoteCI及SDKpublishedpin消费者独立。

## Web远端失败的本次定向映射

`echo-tools/src/http_body.rs` 是已有HTTP适配与共享body读取边界；本次在原typed reqwest/HTTP状态与header仍存在时保留远端失败事实，不通过错误文本猜测。Web provider继续不拥有失败重试；既有ToolFailure承载恢复事实，原ToolManager保持唯一有限重试、等待取消及物理收束权威。DDG原策略回退、分页位置和各reader原大小/编码范围保持；不据此扩展research等其它消费者的验收。

当前Tool＋Domain679包候选的976项选定Rust测试、格式、focused Clippy、featureless/web与demo41/demo56编译已实际通过；三个旧行为对照及首轮新测试返回分支误判的真实失败原件保留。本地原件与精确source epoch见`.echo-semantic/local/framework-tool-domain/batch-return.json`、`source-epoch3.json`及独立增量审查；64预算组合属于一条回归，不另计64测试。该映射不改变本资产原needs_review、observed_at或旧未知项，不表示全资产、完整feature矩阵、SDK Host、新accepted、native或main交付完成。


## 当前项目文件适配的增量映射

本次项目文件适配将原工具的实际 I/O 指向一个不可变 provider，保留 per-call cwd、
原 ToolManager/权限、patch preimage/atomic/rollback 与资源 owner；类型化未知写
不自动重放或猜测回滚。`echo-core/src/tools/filesystem.rs` 定义该通用文件 I/O
接缝，`echo-tools/src/filesystem.rs` 复用原本机文件、CAS 与目录身份机制提供默认
实现。这两个文件是当前候选新增源，不存在于本资产原 observed_at 的历史提交；
因此在这里记录候选职责，不把它们误列为该历史 revision 的 code_refs。

`echo-core/src/project_rules.rs` 保留原规则链并经 provider 读取；
`echo-tools/src/files/mod.rs` 的原路径解析使用同 provider 的命名空间和每次调用 cwd。
`echo-tools/src/files/code_search.rs`、`echo-tools/src/files/grep.rs` 和
`echo-tools/src/files/glob.rs` 将原遍历与内容读取接到该来源，原匹配算法保持。
`echo-tools/src/files/diff.rs` 通过同来源读取待比较内容；
`echo-tools/src/files/repo_map.rs` 通过同来源读取目录与符号输入。上述七个原路径
均可在原 observed_at revision 恢复，其 code_refs 补齐该既有资产的职责覆盖；
本段明确描述的是当前候选的实际改动，不追溯宣称历史版本已经支持 provider。

当前候选实际通过了限定 feature 的库测试、相关 Clippy、四个 feature 检查和
格式检查，随后两个原离线示例也实际运行并自然退出 0；两批准确 clean 均为 0。
本地原件见 `.echo-semantic/local/project-filesystem-adapter/actual-focused-v3/batch-return.json`
与 `.echo-semantic/local/project-filesystem-adapter/offline-examples-actual-v1/lease-return.json`。
这只是列明范围的执行证据；原 needs_review、observed_at、历史未知和全资产验证
结论均不改变。完整 feature/platform、消费者、真实远端、SDK Host 及 main 资格
仍未由本次检查建立。

## R15父层postfix新增阻断（当前义务）

最终Tools父审查C0/I2/M1，报告SHA `8cf5d91ecea8f1f619df2420f81ce7d0d4e6864de0a0be646021e4c344d27340`。I1是现有明确目录合同的源码反例：Glob/RepoMap construction base的词法`../`检查可放行外部路径；四工具递归child symlink未重复验证原根。targetFS namespace正确不等于目录上界正确。Grep已存在多允许根/无根合同，修复不得把普通cwd或用户交互变新门控。Gov file-root r02已获独立source pass，20条原public API回归保持before/after相同测试字节，并已固定进入统一after d702；本稿准备时仍无实际NoRun/RED/GREEN，本材料不写正式源码。

I2是无sandbox Command Hook在计时前完整写stdin，合法大HookContext和不读stdin的child能让已声明timeout失效；IM指定作者的同deadline及fail-fast IO r03已获source-only pass并进入after d702，物理结算与同断言运行仍待实际证据。它同时阻断Extension的Hook合同，不能改称平台unknown或只列外部环境未验。M1只纠正上述permission/tuple先后及Agent共享读map文字，没有重排runtime或改变既有cache修复。父Asset保持needs_review/high，原Finding不闭合。

## 本稿固定后继的资格分列

本稿初始取件基底为R15 `197cd657…`，当前四组修复固定组合为after manifest `d702396280f50ecc8138ccfbd53ae251061a6593451e91af90d337de2b98525f`，assembly `734021fa…`、执行计划 `1fba9fb4…`及其独立静态组合复核只证明准确准备与限定source范围。它们都是普通取件身份，不能填observed_at或冒task/source身份。

本稿准备时，原c01-before-01已进入编译的状态来自Root受控交接；本线不轮询/重启原operation，未取得可用于本后继绑定的实际NoRun自然0、实际test list或新反例RED/GREEN。函数清单是prepared selectors，编译/setup失败不能算行为RED；R15原24与fresh限定工程资格保原来源，不追给after d702。后续真实观察、稳定源工程与正常source/final绑定均交F sole，父status、风险、真实unknown、旧Eq/Finding/Discovery不由本材料改判。
