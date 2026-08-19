use echo_rust_learning::smart_pointers::pinning::{boxed_message, pinned_message};

#[tokio::main]
async fn main() {
    let first = boxed_message("BoxFuture 完成".to_string()).await;
    let second = pinned_message("Pin<Box<Future>> 完成".to_string()).await;
    println!("{first}");
    println!("{second}");
}
