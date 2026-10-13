---
schema_version: 1
id: evidence.clinical-trials-total-repair
kind: evidence
observed_at: 848fa138e1edc077e017b399d891918ca1d3f949
source_refs:
- echo-tools/src/research/clinical_trials.rs
- docs/en/22-research-tools.md
- docs/zh/22-research-tools.md
- docs/evidence/clinical-trials-total-fidelity/default.json
- docs/evidence/clinical-trials-total-fidelity/requested.json
- docs/evidence/clinical-trials-total-fidelity/receipt.json
- docs/evidence/clinical-trials-total-fidelity/SHA256SUMS.json
- docs/evidence/clinical-trials-total-fidelity/before.log
- docs/evidence/clinical-trials-total-fidelity/after.log
supports:
- behavior.native-tool-value-fidelity
- rule.native-tool-value-authority
limitations:
- Exact ClinicalTrials third-file offline and captured upstream scope; independent
  source review and current full23 gates now pass.
- Public main, remote CI and new SDK Host three-language proof remain required; captured386
  is historical and no live server was rerun in this lane.
---

# ClinicalTrials 总量保真：修复与验证

## 支持的结论

原URL未请求countTotal，官方响应study1但无totalCount；旧normalizer默0成功。现原Tool显式countTotal=true，缺失或非u64整数返回原ExecutionFailed；合法0/u64MAX及真实386/page1保留，不新增nullableAPI或SDK字段。

## 来源与范围

原normalization完整body机械private抽取，dedent逐字相同再测；旧4回归3行为失败/1合法通过/0ignored，新同4通过/0ignored。原始HTTP2×200、源码前/中/后可逆base64、SHA和actual日志在docs/evidence/clinical-trials-total-fidelity。此旧反例不冒充whole old-main binary。

## 已知缺口

fmt、alltarget/allfeature echo_tools warningslint和四paniclint0；第三源码独立复审、完整新门禁、public main和SDK消费者待验。旧DB/RAG已审2项只历史保留，不为此项背书。


当前补齐：第三源码独立sdk_capability_acceptance_audit PASS/action0，随后framework_final_review整候选源码与23receipt PASS/action0；见两个当前Audit和evidence.native-tool-value-full-gates。原focused阶段待review记录保留为历史进展，不作为当前未闭合事实。
