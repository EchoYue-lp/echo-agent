use echo_rust_learning::fundamentals::{
    AttemptDecision, countdown, decide_attempt, safe_item, summarize_scores, swap_coordinates,
};

fn main() {
    let language = "Rust";
    let language = format!("{language} 2024 edition"); // shadowing creates a new binding
    let scores = [72_u32, 88, 95];
    let summary = summarize_scores(&scores);

    println!("语言: {language}");
    println!("分数摘要: {summary:?}");
    println!("安全访问: {:?}", safe_item(&scores, 99));
    println!("坐标交换: {:?}", swap_coordinates((10, 20)));
    println!("倒计时: {:?}", countdown(3));

    match decide_attempt(1, 3) {
        AttemptDecision::Start => println!("开始第一次尝试"),
        AttemptDecision::Retry { remaining } => println!("重试，还剩 {remaining} 次"),
        AttemptDecision::Stop => println!("停止执行"),
    }
}
