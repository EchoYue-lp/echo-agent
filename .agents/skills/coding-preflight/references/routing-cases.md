# 路由走查样例

修改本组 Skill 的 `description` 后，用以下近似请求检查触发边界。只判断应加载哪个入口 Skill，不把该表当成
自动授权或业务验收。

| 用户意图 | 应触发 | 不应触发 |
| --- | --- | --- |
| 新增一个框架功能，先看看有没有现成能力 | `coding-preflight` | `semantic-discover`、`semantic-audit` |
| 修一个局部解析缺陷，已有明确复现 | `coding-preflight` | `semantic-decide` |
| 建立整个仓库的语义行为地图 | `semantic-discover` | `coding-preflight`、`semantic-diff` |
| 已有代码差异，分析哪些行为和审查会失效 | `semantic-diff` | `semantic-discover` |
| 审查指定边界的并发风险，已有源码版本 | `semantic-audit` | `semantic-diff`、普通代码审查 |
| 这个行为应该报错还是降级，需要人决定 | `semantic-decide` | `semantic-audit` |
| 检查语义材料引用以及问题是否满足关闭条件 | `semantic-verify` | 普通完成验证 |
| 只运行 Cargo 测试并确认是否通过 | 普通仓库验证 | 所有 semantic Skill |
| 只调整 Markdown 排版，没有语义材料 | `coding-preflight` 的快速路径 | `semantic-audit` |
| 合并重复状态机并删除旧路径 | `coding-preflight` 的架构收敛分支 | `semantic-discover` |
