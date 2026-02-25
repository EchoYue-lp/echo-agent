# Examples 说明

`examples` 目录按能力拆分，保持“一例一能力”：

- `demo01_tools.rs`: 工具调用（Tools）
- `demo02_tasks.rs`: 任务规划（Task Planning）
- `demo03_approval.rs`: 人机协同确认（Human-in-Loop）
- `demo04_subagent.rs`: 子 Agent 编排（SubAgent / Orchestrator）

## 运行方式

```bash
cargo run --example demo01_tools
cargo run --example demo02_tasks
cargo run --example demo03_approval
cargo run --example demo04_subagent
```

## 设计原则

- 各 demo 只突出一个核心能力，避免多个机制互相干扰。
- 先验证能力边界，再组合成完整 Agent 框架场景。
- 示例优先可读性和稳定复现，不追求业务复杂度。
