use crate::skills::Skill;
use crate::tools::Tool;
use crate::tools::others::weather::WeatherTool;

/// 天气查询技能
///
/// 为 Agent 提供城市天气查询能力。
///
/// # 使用方式
/// ```rust
/// agent.add_skill(Box::new(WeatherSkill));
/// ```
pub struct WeatherSkill;

impl Skill for WeatherSkill {
    fn name(&self) -> &str {
        "weather"
    }

    fn description(&self) -> &str {
        "城市天气查询能力，可查询指定城市在特定日期的天气状况"
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![Box::new(WeatherTool)]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        Some(
            "\n\n## 天气查询能力（Weather Skill）\n\
             你可以使用 `query_weather(city, date)` 工具查询天气信息：\n\
             - `city`：城市名称（如 \"北京\"、\"上海\"）\n\
             - `date`：查询日期（如 \"今天\"、\"2024-01-15\"）\n\
             当用户询问天气相关问题时，直接调用此工具获取准确信息，不要凭空捏造天气数据。"
                .to_string(),
        )
    }
}
