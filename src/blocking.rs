//! Bounded synchronous work. Permits live on the blocking thread, including
//! when the request awaiting it is cancelled.

use anyhow::Result;
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;

static WORK: Semaphore = Semaphore::const_new(16);

/// Short synchronous sections in long-lived async tasks. Also works in tests
/// using a current-thread runtime, where block_in_place is unavailable.
pub fn in_place<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(work)
        }
        _ => work(),
    }
}

pub async fn run<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    let permit = WORK.acquire().await?;
    Ok(tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await?)
}

pub async fn response<T: IntoResponse + Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Response {
    match run(work).await {
        Ok(value) => value.into_response(),
        Err(error) => crate::api::fail(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_work_does_not_stop_runtime_timers() {
        let work = tokio::spawn(run(|| std::thread::sleep(Duration::from_millis(150))));
        tokio::time::timeout(Duration::from_millis(100), tokio::time::sleep(Duration::from_millis(10)))
            .await
            .expect("runtime timers must keep running");
        work.await.unwrap().unwrap();
    }
}
