---
schema_version: 1
id: evidence.eval-workspace-generation-verification
kind: evidence
observed_at: 29a00f66843263f27503f83247ee8a770b89e913
source_refs:
  - src/eval/runner.rs
  - src/eval/comparator.rs
  - src/improve/loop.rs
  - docs/en/24-eval-system.md
  - docs/zh/24-eval-system.md
  - docs/adr/0036-eval-workspace-generation-lifecycle.md
supports: [behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - 完整workspace合并门禁与远端CI尚未执行
  - timeout测试使用可控pending Agent，不证明真实detached producer已终止
---

# Eval workspace generation 验证证据

## 支持的结论

同ID fixture与无fixture red均在旧实现exit 101并报告共享cwd。修复后9个EvalRunner tests通过：两个并发fixture副本独立且settled后删除；无fixture run不使用parent且cwd不同；timeout返回保留路径且目录存在；真实active run被caller abort后保留其invocation cwd；typed Agent error显式删除settled generation；fixture setup失败不启动Agent并清理；注入cleanup error使EvalResult失败且保留既有criteria score。

独立复审后的最终门禁中，完整Eval 21 tests、Improve 17 tests、documentation contract 5 tests、`eval,improve` all-target Clippy以及eval/improve两个独立no-default feature check均exit 0。源码不再包含`improve_i`、`ab_compare_`、case-ID join/delete或手工删除runner root；contracts/sdk与sdks/shared相对基准零diff。

## 来源与范围

red/green与工程日志位于`.supreme/logs/plan14-*`；测试通过channels和可控Agent记录真实invocation cwd，不依赖sleep或全局TMPDIR修改。

## 已知缺口

独立review与最终feature/docs门禁已通过；本Evidence须与最后一次semantic/Issue门禁共同使用。未执行长期并发stress、真实detached Tool timeout或跨平台文件删除故障，Issue #48保持open。
