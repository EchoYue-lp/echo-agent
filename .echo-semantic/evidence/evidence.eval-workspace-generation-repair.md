---
schema_version: 1
id: evidence.eval-workspace-generation-repair
kind: evidence
observed_at: source:c205eb521ef63e2d37d921693a1a0703b253144b575642baa3ec94c3ba2d75b3
source_refs:
  - src/eval/runner.rs
  - src/eval/comparator.rs
  - src/improve/loop.rs
  - docs/en/24-eval-system.md
  - docs/zh/24-eval-system.md
  - docs/adr/0036-eval-workspace-generation-lifecycle.md
supports: [behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - timeout与caller-drop只保留隔离generation，不证明Agent stream producer已settled
  - retained timeout目录的最终回收继续由finding.eval-timeout-settlement负责
---

# Eval workspace generation 修复证据

## 支持的结论

基准`e59fe773d92bca409fe0606b608c4812f87f7ac2`把fixture复制到`workspace_root/case.id`并先递归删除目标，无fixture直接使用共享root；Improve固定使用`temp/improve_i`且early-stop绕过手工cleanup。两个真实red分别以同ID并发共享cwd、无fixture共享parent退出101。

当前每个`EvalRunner::run`在配置parent下创建随机唯一`eval-` generation；fixture只复制到空generation，无fixture也使用该目录。私有guard禁用隐式Drop清理：settled路径显式close并把错误投影到EvalResult，timeout显式keep并记录路径，caller-drop warning后保留。Improve与Comparator只选择系统temp parent，不再拥有目录命名或删除逻辑。

## 来源与范围

`src/eval/runner.rs`是唯一generation、cwd与cleanup disposition owner；Improve/Comparator是薄调用方。ADR 0036与双语Eval文档记录OpenAI Evals、Inspect AI和tempfile依据、兼容与回滚。

## 已知缺口

Issue #48仍需让Eval消费bounded Turn settlement后再决定timeout generation回收；本修复不增加后台reaper、worktree或应用Workspace策略。
