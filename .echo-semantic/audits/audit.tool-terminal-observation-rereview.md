---
schema_version: 1
id: audit.tool-terminal-observation-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: result_side_effect
freshness: examined
revision: b71f03ba16fdbefa0a595b92fe82feee39f8e09e
finding_refs: [finding.tool-terminal-observation-divergence]
challenges:
  public-audit-terminal:
    revision: b71f03ba16fdbefa0a595b92fe82feee39f8e09e
    source_refs: [src/agent/react/run/pipeline.rs, src/agent/snapshot.rs, src/agent/react/builder.rs]
    evidence_refs: [evidence.tool-terminal-observation-repair, evidence.tool-terminal-observation-verification]
  guarded-observation:
    revision: b71f03ba16fdbefa0a595b92fe82feee39f8e09e
    source_refs: [src/agent/react/run/pipeline.rs, src/agent/snapshot.rs, echo-state/src/audit/memory.rs]
    evidence_refs: [evidence.tool-terminal-observation-verification]
---

# 工具终态观察独立复审

## 审查范围

独立 reviewer `review_pr130_tool_terminal` 审查 PR #130 完整 diff 与本轮未提交修复；
最终结论 pass 绑定 HEAD 90c9925、pipeline blob bcdd7161、snapshot blob c1c88637 及中英文 security 文档。
随后测试执行行为增量经过定向复审：stdin-drain fixture 的 pipeline blob 为 9dfacf0b，
最终源码摘要为本 Audit 的 revision。reviewer 以 blob 对比确认生产语义未变、断言未变，
但将 fixture 归类为测试语义增量；公开 hook 仍返回 exit 2，all-feature pipeline 23/23 通过。
既有 37 份历史材料除快照标记外与 main@44b2ed68 逐字相同，原主线摘要 eff0290e
已从该 Git tree 重算核实；历史事实绑定该 main revision，而非 squash 后不保证可取回的 PR commit。
本轮 repair、verification 与 rereview 则单独绑定最终当前摘要，不重新宣称全仓审查。
随后 demo64 的注释与固定展示文字经过增量复审，执行流程与断言未变；reviewer pass，
指出的末尾旧 ParseValidateStage 文案已改为 ToolVisibilityStage 并补执行后观察说明。

## 已检查故障假设

公开 audit_logger 是否提前报告成功、执行异常是否重复审计、post-effect block 是否跳过观察、
执行前 block 是否伪造结果、成功/失败输出与 error-only 诊断是否绕过输出护栏或恢复清空内容。

## 实际实现路径与证据

AuditStage 移至输出处理后，记录一次最终 ToolResult；post-effect block 的观察阶段继续执行。
OutputGuardStage 覆盖全部执行结果，无输出失败的 audit_error_output 为独立安全投影而非新状态权威。
caller 结果采用处理后输出，错误诊断契约不变。四个反例红测均在修复后被相应回归覆盖，pipeline 20/20。
完整门禁发现的 hook fixture stdin 竞态经过独立复现、脱敏诊断与同样本 red/green，
最终命令先消费 context 再 exit 2，临时诊断全部删除；增量复审确认未修改生产 sandbox 策略。

## 问题记录

初审及两轮复审发现公开审计提前成功、空串回退、失败输出绕过护栏、error-only 审计绕过护栏；
全部在本轮修复并通过最终只读复审，未发现剩余 findings。

## 残余风险

显式注册 AuditCallback 与 audit_logger 是两个可选消费者；同一 logger 被用户重复注册不承诺去重。
全仓 secret retention 与 backend error 脱敏由独立 Finding 处理，不在此复审中宣称闭合。

## 未检查项

reviewer 未运行 Cargo；全量合并门禁、严格语义快照与远端 CI 由主代理完成。
未扩展外部 SDK 或真实 shell/MCP 的每种恢复副作用。
