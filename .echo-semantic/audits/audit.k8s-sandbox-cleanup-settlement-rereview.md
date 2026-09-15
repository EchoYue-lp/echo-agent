---
schema_version: 1
id: audit.k8s-sandbox-cleanup-settlement-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: result_side_effect
freshness: examined
revision: source:6c19670f1c60cd514abba3d30f6293cd385a7df18f8c355fe6fced3e4e6ab8d9
finding_refs: [finding.k8s-sandbox-cleanup-settlement]
challenges:
  pod-owner-and-drain-settlement:
    revision: source:6c19670f1c60cd514abba3d30f6293cd385a7df18f8c355fe6fced3e4e6ab8d9
    source_refs: [echo-execution/src/sandbox/k8s.rs, docs/adr/0002-sandbox-cancellation-cleanup.md]
    evidence_refs: [evidence.k8s-sandbox-cleanup-settlement-repair, evidence.k8s-sandbox-cleanup-settlement-verification]
  cleanup-debt-and-join-recovery:
    revision: source:6c19670f1c60cd514abba3d30f6293cd385a7df18f8c355fe6fced3e4e6ab8d9
    source_refs: [echo-execution/src/sandbox/k8s.rs]
    evidence_refs: [evidence.k8s-sandbox-cleanup-settlement-repair, evidence.k8s-sandbox-cleanup-settlement-verification]
  ambiguous-create-delete-commit:
    revision: source:6c19670f1c60cd514abba3d30f6293cd385a7df18f8c355fe6fced3e4e6ab8d9
    source_refs: [echo-execution/src/sandbox/k8s.rs, docs/adr/0002-sandbox-cancellation-cleanup.md]
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
JoinError补偿再次被caller drop中断，以及create延迟提交是否会越过首次NotFound删除。

## 实际实现路径与证据

Pod身份与全部执行资源由单一detached owner持有；Completed路径先结算进程组，再按剩余
deadline/caller-abandonment有界drain。所有primary terminal进入同一删除路径，删除等待API对象
与finalizer消失。Cleanup debt在owner交付结果前记录，JoinError补偿由第二个detached cleanup
task持有。修复后定向测试16项通过，两档Clippy、crate check与fmt均通过。
Pod删除还必须产生具名receipt并确认缺失；空NotFound在共享deadline内继续probe，延迟出现会重删，
持续歧义成为typed debt。最终定向测试18项、两档Clippy、crate check与fmt均通过。

## 问题记录

候选首轮复审的Important 2项与Minor 1项均已修复；集成复审又发现1项Important ambiguous
create/delete竞态。红测和单点修复完成后第二轮集成复审Critical、Important、Minor均为0，
实现复审结论PASS。SDK合同、两档workspace Clippy、完整workspace/all-target/all-feature测试、
no-default-features检查、17-feature矩阵与语义strict/change-evidence均通过，Finding可标记resolved；
Issue #62等待MR进入远端main后关闭。

## 残余风险

完整Tokio runtime或宿主进程崩溃需要集群侧reconciler或另行设计Job TTL；本修复只保证
进程内caller drop后的owner settlement。

## 未检查项

未连接真实Kubernetes集群，未运行远端CI、不可达node或自定义finalizer controller故障注入。
