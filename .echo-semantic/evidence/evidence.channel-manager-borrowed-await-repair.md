---
schema_version: 1
id: evidence.channel-manager-borrowed-await-repair
kind: evidence
observed_at: 4eb005c15b9c0036b73e409026c668c02f0a9575
source_refs:
- echo-integration/src/channels/manager.rs
- echo-integration/src/channels/mod.rs
- docs/adr/0084-channel-owned-operations.md
- docs/en/15-im-channels.md
- docs/zh/15-im-channels.md
- docs/supreme/specs/2026-10-13T1430-channel-owned-operations/design.md
- docs/supreme/specs/2026-10-13T1430-channel-owned-operations/plans/plan_01_闭合ChannelOwned操作.md
supports:
- behavior.extension-publication
- rule.extension-generation-authority
limitations:
- Exact local source slice and focused tests only; fresh complete23, published pin, SDK/CLI/website and exact
  main require separate receipts.
- Independent incremental source review pass has zero actionable items; it does not certify every framework
  lens.
---


# ChannelManager 借用注册表跨await阻止原插件重入：修复

## 支持的结论

复用ChannelManager唯一map/entry；ChannelHandle直接持原plugin、handler清理责任与transport-stop epoch。get_owned和owned批量future不借用manager，SDK须短锁取owned后释放再await。entry独立read/write锁；start先登记handler，stop先记录transport成功再await close，失败/取消保留责任；get_mut使epoch失效防旧stop覆盖。旧get/get_mut保留，registration metadata固定声明的兼容取舍见ADR0084，不是第二registry/生命周期。

## 来源与范围

SDK原health持manager mutex跨await，插件回查同manager IDs无法获取锁；start/send/stop/handler close同类借用边界。只替换RwLock无法移除嵌套等待。产品不需要新增权限门控。

逐文件SHA、原始日志/receipt/manifest与before/after字节见 evidence.framework-guard-pivot-channel-focused-archive。独立framework_final_review已读历史24e3到当前全部10 modified+3 added及直接消费者，局部sourcepass/action0。原28 frozen文件保持与历史清单相同，不借用其旧完整23作为本增量证明。

## 已知缺口

新版完整工程门禁尚待执行；公开依赖、真实SDK各语言消费与精确main独立验收。Plan lifecycle保持ready，只有最终交付验收可推进。


历史定位补充：上述34d5完整工程/独立审查绑定真实提交 4eb005c15b9c0036b73e409026c668c02f0a9575（唯一source摘要重算一致），后续依赖策略修复使当前source变化，不借此证明新版依赖通过。原命令、路径、时间、原结论与限制保持。
