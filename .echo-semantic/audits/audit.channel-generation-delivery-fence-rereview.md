---
schema_version: 1
id: audit.channel-generation-delivery-fence-rereview
kind: audit
boundary_ref: boundary.protocol-surfaces
lens: time_lifecycle
freshness: examined
revision: source:3a1ceacf4f7e1214698abf2ea09cc3217e426b87fadb0988c97926eb9dcd2bfa
finding_refs: [finding.channel-reset-stale-generation-delivery]
challenges:
  transport-admission-barrier:
    revision: source:3a1ceacf4f7e1214698abf2ea09cc3217e426b87fadb0988c97926eb9dcd2bfa
    source_refs: [echo-integration/src/channels/channels/mod.rs, echo-integration/src/channels/channels/qq/channel.rs, echo-integration/src/channels/channels/feishu/channel.rs]
    evidence_refs: [evidence.channel-generation-delivery-fence-repair, evidence.channel-generation-delivery-fence-verification]
  setup-and-stream-cancellation:
    revision: source:3a1ceacf4f7e1214698abf2ea09cc3217e426b87fadb0988c97926eb9dcd2bfa
    source_refs: [echo-integration/src/channels/session.rs]
    evidence_refs: [evidence.channel-generation-delivery-fence-verification]
  rotation-shared-authority:
    revision: source:3a1ceacf4f7e1214698abf2ea09cc3217e426b87fadb0988c97926eb9dcd2bfa
    source_refs: [echo-integration/src/channels/types.rs, echo-integration/src/channels/session.rs]
    evidence_refs: [evidence.channel-generation-delivery-fence-repair, evidence.channel-generation-delivery-fence-verification]
---

# Channel generation delivery fence 独立复审

## 审查范围

独立reviewer `review_issue41_channel_fence`只读审查完整未提交diff、ADR 0001/0057、
SessionHandler与QQ/飞书直接消费者、red/green日志和focused Clippy。最终结论pass，0未决项。

## 已检查故障假设

reset是否先于已接纳network delivery返回；旧stream setup/poll是否可逃逸取消；未poll stream是否
泄漏receipt；application rotate是否让旧permit从后续framework reset计数中消失；timeout/prune
是否误替换active generation；queue/direct send是否绕过typed stale；外部代码是否可清空opaque fence。

## 实际实现路径与证据

reviewer初审发现setup await未select cancellation；补red/green后消除。增量复审又发现rotate
替换独立fence state会遗失旧permit，以及public message field可被外部清空；对应反例先0/1，
修复为共享generation-level lifecycle/active count并收回模块私有后1/1。最后补QQ/飞书direct send
2/2，证明typed stale校验先于channel started/network分支。

## 问题记录

三项Important实现/合同问题与一项direct-send覆盖缺口均已在本轮直接修复；最终复审未发现剩余阻断。
主代理随后执行17项独立feature矩阵和完整本地合并门禁，全部exit 0；全量测试汇总为
2819 passed、0 failed、3 ignored。门禁之后仅回填本Evidence与Finding状态，生产快照未变化。

## 残余风险

本地transport admission之前的旧输出被拒绝；已经被远端provider接纳的side effect不可撤回，
framework通过等待该permit结算保证reset acknowledgement不会先于它出现。自定义transport绕过
framework delivery API时不自动继承此保证。

## 未检查项

reviewer未运行真实QQ/飞书网络请求；17-feature矩阵与完整workspace门禁由主代理完成，
远端Linux/Windows CI仍由PR交付核实。
