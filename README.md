# echo-agent

一个rust实现的agent框架

## 该框架将实现如下核心 Agent 流程

* tools
* todo task
* human in loop
* subagent
* context compact
* mcp
* skills

## 该框架将支持如下功能

* 支持多模型配置
* 用户自由选择是否启用上述核心 agent 流程
* 支持多种使用方式，计划支持：命令行、HTTP
* 支持异步执行，让工具支持异步执行
* 友好的日志处理与错误处理
* 流式支持
* 持久化存储
* 并行工具执行
* 中间件系统，在工具执行前后增加钩子，方便做日志、监控、限流等

## 快速开始

1、模型配置，格式：固定开头(AGENT_MODEL)_模型名称(xxx)_模型参数(xxx)。
可将参数放在环境变量中，或者放在配置文件里面。

样例如下：

```shell
AGENT_MODEL_QWEN3_MODEL=qwen3-max
AGENT_MODEL_QWEN3_BASEURL=https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions
AGENT_MODEL_QWEN3_APIKEY=sk-xxxxxxx

AGENT_MODEL_DS_MODEL=deepseek-chat
AGENT_MODEL_DS_BASEURL=https://api.deepseek.com/chat/completions
AGENT_MODEL_DS_APIKEY=sk-xxxxxxx
```

2、实例化 ReactAgent，指定模型名称、agent 名称、系统提示语

```rust
let system_prompt = r#"系统提示词"#;
let model = "qwen3-max";
let agent_name = "my_math_agent";
```

3、运行

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🧠 ReAct 智能体完整演示\n");

    let system_prompt = r#"你是一个使用 ReAct 框架的智能助手。

**核心规则：在调用任何操作工具之前，必须先调用 think 工具！**

可用工具：
- think: 记录你的推理过程（必须首先调用）
- add/subtract/multiply/divide: 执行计算

标准流程：
1. 调用 think(reasoning="我的分析...") 记录思考
2. 调用实际的操作工具
3. 得到结果后，再次调用 think 分析结果
4. 重复直到问题解决

"#;
    let model = "qwen3-max";
    let agent_name = "my_math_agent";

    let config = ReactConfig::new(model, agent_name, system_prompt).verbose(true);

    let mut agent = ReactAgent::new(config);

    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(DivideTool));
    agent.add_tool(Box::new(MultiplyTool));
    agent.add_tool(Box::new(SubtractTool));

    let result = agent
        .execute("计算 12 除以 3 + 2 +2 * 8 + 2 + 6 乘以 4 等于多少？")
        .await;
    println!("\n📋 最终结果:\n{:?}", result);

    Ok(())
}
```