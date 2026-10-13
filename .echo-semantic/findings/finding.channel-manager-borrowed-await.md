---
schema_version: 1
id: finding.channel-manager-borrowed-await
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus:
- state_authority
- time_lifecycle
- result_side_effect
- contract_evidence
boundary_ref: boundary.extension-lifecycle
behavior_refs:
- behavior.extension-publication
rule_refs:
- rule.extension-generation-authority
evidence_refs:
- evidence.channel-manager-borrowed-await-repair
- evidence.channel-manager-borrowed-await-verification
- evidence.framework-guard-pivot-channel-focused-archive
- evidence.framework-final-34d5-full-verification
- evidence.framework-final-34d5-gate-archive
audit_refs:
- audit.channel-manager-borrowed-await-rereview
- audit.framework-final-34d5-gate-rereview
decision_refs: []
repair_evidence_refs:
- evidence.channel-manager-borrowed-await-repair
verification_evidence_refs:
- evidence.channel-manager-borrowed-await-verification
- evidence.framework-final-34d5-full-verification
rereview_audit_refs:
- audit.channel-manager-borrowed-await-rereview
- audit.framework-final-34d5-gate-rereview
discovered_at: 95ec66c6083a8feb17a1c28de0d01e1a24ca8a28
---



# ChannelManager 借用注册表跨await阻止原插件重入

## 问题

SDK原health持manager mutex跨await，插件回查同manager IDs无法获取锁；start/send/stop/handler close同类借用边界。只替换RwLock无法移除嵌套等待。产品不需要新增权限门控。

## 触发条件与影响

registry guard不得跨owned await；原plugin/handler/stop责任只能存在于同entry，取消失败不能抹去close责任，批量操作不得吞兄弟结果，metadata不会等待entry锁。

## 唯一权威与边界

复用ChannelManager唯一map/entry；ChannelHandle直接持原plugin、handler清理责任与transport-stop epoch。get_owned和owned批量future不借用manager，SDK须短锁取owned后释放再await。entry独立read/write锁；start先登记handler，stop先记录transport成功再await close，失败/取消保留责任；get_mut使epoch失效防旧stop覆盖。旧get/get_mut保留，registration metadata固定声明的兼容取舍见ADR0084，不是第二registry/生命周期。

## 证据

test-manager-repaired.log为11 passed/0 failed/0 ignored；初次AtomicU64警告尝试保留、修复后同11测试及双lint零警告退出0。clippy-alltargets、clippy-production、doc-channels(2 passed/0 failed)、example-demo38均真实receipt exitCode0，所有log/source SHA核对。覆盖五类callback、lazy batch快照、独立entry、rollback/取消/close retry/get_mut epoch/metadata声明。 独立局部复审pass/action0，原始字节归档可核验。

## 处理记录

本地此故障已具修复、真实focused验证及独立复审，故resolved；fresh完整23、正式发布pin、SDK/CLI/官网与精确main交付仍分别待验。此状态不声明外部Issue关闭，也不替代其它Finding。


## 当前完整工程验收

同一source34d5的新版23完整门禁已真实通过（evidence.framework-final-34d5-full-verification）：3471/0/3外部optin ignored、86targets、59examples，全部warnings0/source/index不变。上文fresh23待验是此前focused阶段记录；完整工程缺口由此补齐，公开pin、SDK/CLI/官网及exact main仍独立待验。原独立局部Audit范围不扩大。


## 当前候选最终复审

audit.framework-final-34d5-gate-rereview 将本地此故障的精确源码/既有红绿证据与同source34d5新完整23门禁绑定；独立implementation review PASS/action0。当前可以进入授权framework里程碑交付，SDK/remoteCI/公开pin及精确main仍另验，其它Finding与全risk lenses不随此关闭。
