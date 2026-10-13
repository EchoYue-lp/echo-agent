---
schema_version: 1
id: audit.channel-manager-borrowed-await-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: failure_concurrency
freshness: examined
revision: 4eb005c15b9c0036b73e409026c668c02f0a9575
finding_refs:
- finding.channel-manager-borrowed-await
challenges:
  owned-slice-and-counterexamples:
    revision: 4eb005c15b9c0036b73e409026c668c02f0a9575
    source_refs:
    - echo-integration/src/channels/manager.rs
    - echo-integration/src/channels/mod.rs
    - docs/adr/0084-channel-owned-operations.md
    - docs/en/15-im-channels.md
    - docs/zh/15-im-channels.md
    - docs/supreme/specs/2026-10-13T1430-channel-owned-operations/design.md
    - docs/supreme/specs/2026-10-13T1430-channel-owned-operations/plans/plan_01_闭合ChannelOwned操作.md
    evidence_refs:
    - evidence.channel-manager-borrowed-await-repair
    - evidence.channel-manager-borrowed-await-verification
    - evidence.framework-guard-pivot-channel-focused-archive
---


# ChannelManager 借用注册表跨await阻止原插件重入：独立局部复审

## 审查范围

独立framework_final_review在本次source摘要上检查新增真实切片、全部10修改/3新增、直接消费者、对应双语文档与ADR84/design/plan；两个focused manifest的owned源码及日志SHA一致。原28 frozen清单/tmp/framework-shared-validation-final-gates/frozen-implementation-docs.json保持一致。

## 已检查故障假设

registry guard不得跨owned await；原plugin/handler/stop责任只能存在于同entry，取消失败不能抹去close责任，批量操作不得吞兄弟结果，metadata不会等待entry锁。

## 实际实现路径与证据

复用ChannelManager唯一map/entry；ChannelHandle直接持原plugin、handler清理责任与transport-stop epoch。get_owned和owned批量future不借用manager，SDK须短锁取owned后释放再await。entry独立read/write锁；start先登记handler，stop先记录transport成功再await close，失败/取消保留责任；get_mut使epoch失效防旧stop覆盖。旧get/get_mut保留，registration metadata固定声明的兼容取舍见ADR0084，不是第二registry/生命周期。

test-manager-repaired.log为11 passed/0 failed/0 ignored；初次AtomicU64警告尝试保留、修复后同11测试及双lint零警告退出0。clippy-alltargets、clippy-production、doc-channels(2 passed/0 failed)、example-demo38均真实receipt exitCode0，所有log/source SHA核对。覆盖五类callback、lazy batch快照、独立entry、rollback/取消/close retry/get_mut epoch/metadata声明。

## 问题记录

局部source-level最终pass，Critical/Important/Minor均0；focused验证证据如上。修复仅关闭本Finding列出的故障，不关闭旧Guard方向、Channel generation/turn或其它开放义务。

## 残余风险

完整最后23尚无fresh receipt，不能给出完整最终工程pass；未证明公开pin、SDK三语言、CLI/官网与exact main。

## 未检查项

不扩大为框架全部风险视角已verified；独立后续完整验收只补工程receipt，不把源码局部复审重写成全量结论。


历史定位补充：上述34d5完整工程/独立审查绑定真实提交 4eb005c15b9c0036b73e409026c668c02f0a9575（唯一source摘要重算一致），后续依赖策略修复使当前source变化，不借此证明新版依赖通过。原命令、路径、时间、原结论与限制保持。
