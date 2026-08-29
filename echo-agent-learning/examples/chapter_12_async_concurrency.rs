use echo_agent_learning::async_concurrency::{complete_within, run_subagents, wait_or_cancel};
use echo_agent_learning::errors::LearningError;
use echo_agent_learning::smart_pointers::synchronization::SharedProgress;
use tokio::sync::watch;
use tokio::time::Duration;

#[tokio::main]
async fn main() -> Result<(), LearningError> {
    let progress = SharedProgress::new();
    let mirrored = progress.clone();
    mirrored.set("compile", 50).await;

    let results = run_subagents(vec!["researcher".to_string(), "reviewer".to_string()]).await?;
    progress.set("compile", 100).await;

    println!("Subagent 结果: {results:?}");
    println!("共享进度: {:?}", progress.snapshot().await);
    println!(
        "按时完成: {}",
        complete_within(Duration::from_millis(1), Duration::from_secs(1)).await?
    );

    let (cancel_sender, cancel_receiver) = watch::channel(false);
    cancel_sender
        .send(true)
        .map_err(|_| LearningError::ChannelClosed)?;
    println!(
        "取消结果: {:?}",
        wait_or_cancel(Duration::from_secs(1), cancel_receiver).await
    );
    Ok(())
}
