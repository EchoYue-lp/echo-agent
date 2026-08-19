//! `Pin<Box<Future>>` gives dynamically dispatched futures a stable location.

use futures::future::BoxFuture;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub fn boxed_message(message: String) -> BoxFuture<'static, String> {
    Box::pin(async move {
        tokio::time::sleep(Duration::from_millis(1)).await;
        message
    })
}

pub fn pinned_message(message: String) -> Pin<Box<dyn Future<Output = String> + Send + 'static>> {
    Box::pin(async move { message })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn boxed_future_can_be_awaited() {
        assert_eq!(boxed_message("done".to_string()).await, "done");
        assert_eq!(pinned_message("pinned".to_string()).await, "pinned");
    }
}
