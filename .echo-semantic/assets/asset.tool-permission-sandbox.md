---
schema_version: 1
id: asset.tool-permission-sandbox
kind: asset
title: Tool、Permission 与 Sandbox Effect Pipeline
asset_type: state_authority
status: needs_review
risk: high
observed_at: 046674d4775d31ddc2daad7107a8425edb648c8f
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



# Tool、Permission 与 Sandbox Effect Pipeline

## 资产身份

Tool registry/schema/execution primitive 与 ReactAgent permission/approval/rewrite/guard、artifact、resource cleanup 的组合边界；不把 programmatic caller 或 trusted Hook 误并为同一 policy path。

## 来源与消费者

ReactAgent、Skills/Hooks、Subagents 和直接 callers 消费；echo-tools 提供具体 effect。

## 生命周期

Register/visibility、owned generation lookup、shared validation、context-scoped cache/epoch、authorize/rewrite、execute/stream、persist artifact、cancel/cleanup/terminal。Artifact writer、Sandbox backend 与 worktree creator 分别持有资源身份直至结算；未完成清理保留显式 debt。

## 候选关系

PermissionPolicy、PermissionService 与 Hook decision 是组合层，不应各自宣称最终 effect authority。

## 未知与限制

Hook Allow protected-path、readonly custom Tool 与 #47 精确资源 cleanup 反例已闭合；其它 Hook permission precedence 和部分工具 cleanup 仍等待专项审计。

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
