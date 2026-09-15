---
schema_version: 1
id: audit.k8s-sandbox-cleanup-settlement-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: result_side_effect
freshness: examined
revision: source:ebb9b2db3cc55a1e63eeea959d10359c8da2d72c9e0e6e2ea27026186bc87c48
finding_refs: [finding.k8s-sandbox-cleanup-settlement]
challenges:
  pod-owner-and-drain-settlement:
    revision: source:ebb9b2db3cc55a1e63eeea959d10359c8da2d72c9e0e6e2ea27026186bc87c48
    source_refs: [echo-execution/src/sandbox/k8s.rs, docs/adr/0002-sandbox-cancellation-cleanup.md]
    evidence_refs: [evidence.k8s-sandbox-cleanup-settlement-repair, evidence.k8s-sandbox-cleanup-settlement-verification]
  cleanup-debt-and-join-recovery:
    revision: source:ebb9b2db3cc55a1e63eeea959d10359c8da2d72c9e0e6e2ea27026186bc87c48
    source_refs: [echo-execution/src/sandbox/k8s.rs]
    evidence_refs: [evidence.k8s-sandbox-cleanup-settlement-repair, evidence.k8s-sandbox-cleanup-settlement-verification]
---

# K8s Sandbox cleanup settlement独立复审

## 审查范围

Reviewer检查detached Pod owner、caller-abandonment guard、kubectl child/process group、stdin与
pipe drain、执行deadline、Pod delete settlement、cleanup debt、JoinError补偿、ADR和
deterministic fake-kubectl测试；SandboxManager owner与其它Sandbox Finding排除。

## 已检查故障假设

检查caller drop跳过delete、leader退出但helper持有pipe导致owner永久阻塞、stdin写入阻塞或
失败、delete spawn/nonzero/timeout伪装成功、cleanup error在owner-result/caller-ack间丢失，
以及JoinError补偿再次被caller drop中断。

## 实际实现路径与证据

Pod身份与全部执行资源由单一detached owner持有；Completed路径先结算进程组，再按剩余
deadline/caller-abandonment有界drain。所有primary terminal进入同一删除路径，删除等待API对象
与finalizer消失。Cleanup debt在owner交付结果前记录，JoinError补偿由第二个detached cleanup
task持有。修复后定向测试16项通过，两档Clippy、crate check与fmt均通过。

## 问题记录

首轮复审的Important 2项与Minor 1项均已修复；第二轮Critical、Important、Minor均为0，
实现复审结论PASS。Finding在integration统一刷新共享snapshot并执行final gate前保持open。

## 残余风险

完整Tokio runtime或宿主进程崩溃需要集群侧reconciler或另行设计Job TTL；本修复只保证
进程内caller drop后的owner settlement。

## 未检查项

未连接真实Kubernetes集群，未运行full workspace门禁、远端CI、不可达node或自定义finalizer
controller故障注入。
