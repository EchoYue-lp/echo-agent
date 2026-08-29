use echo_agent_learning::ownership::{append_label, character_count, first_non_empty, owned_title};

fn main() {
    let title = String::from("学习所有权");
    let count = character_count(&title);
    let copied_for_storage = owned_title(&title);
    let labels = append_label(vec!["rust".to_string()], "ownership");

    println!("{title}: {count} 个字符");
    println!("独立所有权: {copied_for_storage}");
    println!("回退值: {}", first_non_empty("", "untitled"));
    println!("标签: {labels:?}");
}
