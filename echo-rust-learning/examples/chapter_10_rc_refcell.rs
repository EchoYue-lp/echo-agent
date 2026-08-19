use echo_rust_learning::errors::LearningError;
use echo_rust_learning::smart_pointers::rc_refcell::SharedNotebook;

fn main() -> Result<(), LearningError> {
    let first_view = SharedNotebook::new();
    let second_view = first_view.clone();
    second_view.add("Rc 共享所有权")?;
    second_view.add("RefCell 在运行时检查借用")?;

    println!("强引用数: {}", first_view.strong_count());
    println!("共享内容: {:?}", first_view.entries()?);
    Ok(())
}
