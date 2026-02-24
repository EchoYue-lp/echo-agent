use echo_agent::agent::Agent;
use echo_agent::agent::react_agent::{AgentConfig, AgentRole, ReactAgent};
use echo_agent::tools::Tool;
use echo_agent::tools::math::{AddTool, DivideTool, MultiplyTool, SubtractTool};
use echo_agent::tools::weather::WeatherTool;

/// demo04: SubAgent 编排演示（Orchestrator + Worker）
fn create_all_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(AddTool),
        Box::new(SubtractTool),
        Box::new(MultiplyTool),
        Box::new(DivideTool),
        Box::new(WeatherTool),
    ]
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("🧪 demo04 - SubAgent 编排演示\n");

    // 第一步：定义 Worker sub-agent 配置
    let sub_agent_configs = vec![
        AgentConfig::new(
            "qwen3-max",
            "weather-agent",
            "你是获取天气数据的专家，专注于：

- 使用工具获取天气数据
 - 在获取天气数据时完全根据用户需求进行，不凭空猜测、杜撰、编造一切时间、地点参数",
        )
        .enable_tool(true)
        .enable_task(false)
        .enable_human_in_loop(false)
        .enable_subagent(false)
        .allowed_tools(vec![WeatherTool.name().to_string()]),
        AgentConfig::new(
            "qwen3-max",
            "math-agent",
            "你是擅长数据计算的专家，专注于：

- 进行复杂的数学计算",
        )
        .enable_tool(true)
        .enable_task(false)
        .enable_human_in_loop(false)
        .enable_subagent(false)
        .allowed_tools(vec![
            AddTool.name().to_string(),
            SubtractTool.name().to_string(),
            MultiplyTool.name().to_string(),
            DivideTool.name().to_string(),
        ]),
    ];

    //  第二步：构建 Worker sub-agents
    let sub_agents: Vec<Box<dyn Agent>> = sub_agent_configs
        .into_iter()
        .map(|config| {
            let mut agent = ReactAgent::new(config);
            agent.add_tools(create_all_tools());
            Box::new(agent) as Box<dyn Agent>
        })
        .collect();

    // 第三步：构建 Orchestrator main agent
    let main_config = AgentConfig::new(
        "qwen3-max",
        "main_agent",
        r#"你是一个智能助手，负责协调和分配任务。

你的职责：
1. 理解用户需求
2. 判断任务复杂度
3. 对于复杂或专业任务，使用 agent_tool 分配给专用 SubAgent
4. 汇总 SubAgent 结果并呈现给用户

可用的 SubAgent：
- math-agent: 擅长数学计算
- weather-agent: 获取天气相关信息

当任务涉及天气查询或数学计算时，优先通过 agent_tool 调度对应 SubAgent，不要自己直接计算。"#,
    )
    .role(AgentRole::Orchestrator)
    .enable_tool(false)
    .enable_task(true)
    .enable_human_in_loop(false)
    .enable_subagent(true)
    .verbose(true)
    .max_iterations(50);

    let mut main_agent = ReactAgent::new(main_config);
    main_agent.register_agents(sub_agents);

    // 第四步：执行任务
    let result = main_agent
        .execute_with_planning(
            "我有1000元，我准备去商场购物。\
            我需要买10个15元的本子，16个8元的笔，2个98元的玩具，一个500元的衣服。\
            商场决定单品价格满500打八折，总价满800打9折。\
            如果当天天气下雨，则商场总价会打85折。\
            请问我还剩多少？",
        )
        .await;

    println!("\n✅ 最终结果:\n{:?}", result);
    Ok(())
}
