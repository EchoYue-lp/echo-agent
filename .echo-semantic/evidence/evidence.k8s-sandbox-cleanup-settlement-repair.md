---
schema_version: 1
id: evidence.k8s-sandbox-cleanup-settlement-repair
kind: evidence
observed_at: source:eff0290e1aff3c3a56f0ba94f57460e220d08f8eb04d3023efde058982972f03
source_refs:
  - echo-execution/src/sandbox/k8s.rs
  - docs/adr/0002-sandbox-cancellation-cleanup.md
  - docs/en/security.md
  - docs/zh/security.md
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 完整进程或Tokio runtime崩溃仍需要独立的集群侧reconciler或Job TTL设计
  - 本修复不改变SandboxManager owner缺口或其它Sandbox Finding
---

# K8s Sandbox cleanup settlement修复证据

## 支持的结论

`K8sSandbox`在启动kubectl前预分配Pod名称，并把kubectl child、stdin/output、同一执行
deadline、显式取消、caller-abandonment token和Pod删除转交单一detached backend owner。
正常与非零退出、stdin/kubectl失败、超时、取消和caller drop均汇入同一Pod cleanup。

kubectl leader正常退出后仍结算其进程组，避免helper后代持有stdout/stderr使owner卡在EOF；
pipe drain同时受剩余执行deadline与caller-abandonment约束。JoinError补偿删除也由detached
cleanup task持有，丢弃等待它的future不会再次中断删除。

删除使用一秒grace period、显式`--wait=true`和有界kubectl timeout；spawn、timeout或
非零退出不再丢弃。等待中的caller收到保留primary terminal facts与cleanup debt的typed
sandbox I/O错误；detached owner在把结果交给caller前无条件记录cleanup debt，不存在
owner result已完成而caller随后drop导致debt同时丢失的窗口。

创建请求还可能在本地kubectl终止后延迟提交。Pod delete/get/poll现在共用一个绝对cleanup
deadline；`delete --output=name`提供真实删除receipt，空成功只表示当前NotFound而不是terminal。
owner会继续确认，延迟出现则重删；始终没有receipt时到期返回typed cleanup debt，不能猜测
远端create从未提交。

## 来源与范围

实现位于`echo-execution/src/sandbox/k8s.rs`，复用ADR 0002已经接受的Local/Docker
detached owner terminal contract，没有新增公共API、产品权限策略或第二套生命周期状态机。
Kubernetes Pod termination、finalizer、foreground deletion、Job TTL与`kubectl delete`
官方合同记录在更新后的ADR 0002。

## 已知缺口

本修复保证进程内caller drop后的owner继续结算；完整runtime或宿主进程崩溃不受Tokio task
保护。采用Job/TTL作为集群侧兜底需要独立改变create/attach/cancel与Job foreground deletion
协议，不在Finding #62的direct-Pod修复中隐式引入。
