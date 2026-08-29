//! Tokio tasks, bounded channels, and structured result collection.

use crate::errors::LearningError;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::time::{Duration, timeout};

fn duration_millis_saturated(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub async fn run_subagents(names: Vec<String>) -> Result<Vec<String>, LearningError> {
    let capacity = names.len().max(1);
    let (sender, mut receiver) = mpsc::channel(capacity);
    let mut handles = Vec::with_capacity(names.len());

    for name in names {
        let sender = sender.clone();
        handles.push(tokio::spawn(async move {
            sender
                .send(format!("{name}: completed"))
                .await
                .map_err(|_| LearningError::ChannelClosed)
        }));
    }
    drop(sender);

    for handle in handles {
        handle
            .await
            .map_err(|error| LearningError::SubagentJoin(error.to_string()))??;
    }

    let mut results = Vec::new();
    while let Some(result) = receiver.recv().await {
        results.push(result);
    }
    results.sort();
    Ok(results)
}

pub async fn complete_within(
    delay: Duration,
    deadline: Duration,
) -> Result<&'static str, LearningError> {
    let deadline_ms = duration_millis_saturated(deadline);
    timeout(deadline, tokio::time::sleep(delay))
        .await
        .map_err(|_| LearningError::TimedOut(deadline_ms))?;
    Ok("completed")
}

pub async fn wait_or_cancel(
    delay: Duration,
    mut cancelled: watch::Receiver<bool>,
) -> Result<&'static str, LearningError> {
    if *cancelled.borrow() {
        return Err(LearningError::Cancelled);
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => Ok("completed"),
        changed = cancelled.changed() => {
            changed.map_err(|_| LearningError::ChannelClosed)?;
            if *cancelled.borrow() {
                Err(LearningError::Cancelled)
            } else {
                Ok("cancel signal cleared")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn collects_each_subagent_result() -> Result<(), LearningError> {
        let results = run_subagents(vec!["writer".to_string(), "reviewer".to_string()]).await?;
        assert_eq!(results.len(), 2);
        assert_eq!(
            results.first().map(String::as_str),
            Some("reviewer: completed")
        );
        Ok(())
    }

    #[tokio::test]
    async fn timeout_and_cancellation_have_distinct_errors() {
        assert!(matches!(
            complete_within(Duration::from_millis(20), Duration::from_millis(1)).await,
            Err(LearningError::TimedOut(_))
        ));

        let (sender, receiver) = watch::channel(false);
        let task = tokio::spawn(wait_or_cancel(Duration::from_secs(1), receiver));
        assert!(sender.send(true).is_ok());
        let result = task
            .await
            .map_err(|error| LearningError::SubagentJoin(error.to_string()));
        assert!(matches!(result, Ok(Err(LearningError::Cancelled))));
    }

    #[test]
    fn duration_conversion_saturates_instead_of_truncating() {
        assert_eq!(duration_millis_saturated(Duration::MAX), u64::MAX);
    }
}
