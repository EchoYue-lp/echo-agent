use echo_agent_learning::errors::{LearningError, parse_positive_limit, required_text};

fn load_settings(name: Option<&str>, limit: &str) -> Result<(String, usize), LearningError> {
    let name = required_text("name", name)?;
    let limit = parse_positive_limit(limit)?;
    Ok((name, limit))
}

fn main() -> Result<(), LearningError> {
    let (name, limit) = load_settings(Some(" assistant "), "8")?;
    println!("名称: {name}, 最大轮数: {limit}");

    match load_settings(None, "not-a-number") {
        Ok(settings) => println!("意外成功: {settings:?}"),
        Err(error) => println!("预期错误: {error}"),
    }
    Ok(())
}
