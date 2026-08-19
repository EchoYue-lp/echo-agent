use echo_rust_learning::errors::{LearningError, parse_positive_limit};
use echo_rust_learning::traits::{
    LearningTaskBuilder, PlainFormatter, StatusFormatter, TaskFormatter, format_dynamic,
};

fn main() -> Result<(), LearningError> {
    let limit = parse_positive_limit("8")?;
    let task = LearningTaskBuilder::new()
        .title("理解 trait object")
        .running(true)
        .build()?;
    let formatters: Vec<Box<dyn TaskFormatter>> =
        vec![Box::new(PlainFormatter), Box::new(StatusFormatter)];

    println!("限制: {limit}");
    for line in format_dynamic(&formatters, &task) {
        println!("{line}");
    }
    Ok(())
}
