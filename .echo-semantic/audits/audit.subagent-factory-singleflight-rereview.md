---
schema_version: 1
id: audit.subagent-factory-singleflight-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: failure_concurrency
freshness: examined
revision: 6d66479fd520da9cbbb66723faa35ce69a8963a8
finding_refs: [finding.subagent-factory-cancellation, finding.subagent-factory-publication-race]
challenges:
  cancellation-and-failure-ownership:
    revision: 6d66479fd520da9cbbb66723faa35ce69a8963a8
    source_refs: [src/agent/subagent/registry.rs, docs/adr/0033-subagent-factory-singleflight-publication.md]
    evidence_refs: [evidence.subagent-factory-singleflight-repair, evidence.subagent-factory-singleflight-verification]
  same-revision-publication:
    revision: 6d66479fd520da9cbbb66723faa35ce69a8963a8
    source_refs: [src/agent/subagent/registry.rs]
    evidence_refs: [evidence.subagent-factory-singleflight-repair, evidence.subagent-factory-singleflight-verification]
  registration-generation-fence:
    revision: 6d66479fd520da9cbbb66723faa35ce69a8963a8
    source_refs: [src/agent/subagent/registry.rs]
    evidence_refs: [evidence.subagent-factory-singleflight-verification]
---

# Subagent factory single-flight 独立复审

## 审查范围

复审`RegistryEntry`的revision-scoped OnceCell、`get_agent`初始化与fast path、取消/错误重试、同revision并发发布、remove/re-register generation fence、catalog revision、caller deadline及确定性交错测试。

## 已检查故障假设

验证cancel、error或initializer panic是否遗留ownership，同revision是否仍可创建或发布两个Agent，旧cell结果是否能进入新registration，fast path与revision/cell复核是否线性一致，以及publication测试是否仅因负向timeout或未调度第二caller而假通过。

## 实际实现路径与证据

每个entry持有独立OnceCell和revision；成功初始化先在cell内发布，随后用revision与cell identity双重fence复核当前registration。取消或错误保持cell为空，旧代结果只能返回当前entry已发布实例或None。首轮review指出publication测试缺少第二caller正信号；修复后第二resolver先确认entered，再在第一resolver仍暂停时必须实际join并返回cached Arc，旧实现则明确产生duplicate factory start。最终同时断言Arc identity与factory attempt为1。15个registry tests、3个fresh-factory executor tests、Clippy、feature check与SDK inventory drift check通过。

## 问题记录

首轮Important测试假阳性风险已修复并通过同一reviewer定向复审；`finding.subagent-factory-cancellation`与`finding.subagent-factory-publication-race`具备repair、verification和rereview证据，可标记resolved。未关闭TaskClaim/Attempt、definition catalog或其它Finding。

## 残余风险

Factory返回前产生的外部副作用不由Registry补偿；持续失败时等待者可串行重试；同名递归依赖调用方遵守rustdoc并设置适当deadline。

## 未检查项

未执行loom、第三方factory故障注入、完整workspace合并门禁、远端CI、CLI或website验收。
