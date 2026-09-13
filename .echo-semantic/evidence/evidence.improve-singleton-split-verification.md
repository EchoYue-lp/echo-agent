---
schema_version: 1
id: evidence.improve-singleton-split-verification
kind: evidence
observed_at: source:a2317ccf488e81ce737d93a5c7b13369d67228da5e54baf56c14210a47794342
source_refs:
  - src/improve/loop.rs
supports: [behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - 完整workspace合并门禁与远端CI尚未执行
  - 未执行真实LLM improvement loop或并发workspace故障注入
---

# Improve singleton split 验证证据

## 支持的结论

旧实现上的合法单例测试在`src/improve/loop.rs`以`min = 1, max = 0` panic并exit 101。修复后的4个Improve loop tests全部通过：单例只在train、混合criteria不把单例放入holdout、同criteria多例在ratio -1/0/1/2下两侧非空且每个case恰好出现一次、默认配置不回归。

## 来源与范围

Improve feature Clippy以`-D warnings`通过，`--no-default-features --features improve` check通过；`contracts/sdk`与`sdks/shared`相对基准没有diff。实现未改变public签名、SDK identity、持久化、workspace或副作用路径。

## 已知缺口

本证据只覆盖私有split算法与输入边界；`run_async`真实Agent执行、workspace cleanup和跨进程行为不在本切片内。
