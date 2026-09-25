//! Helpers for waiting on conditions of test nodes.

use eyre::eyre;
use std::{fmt::Display, future::Future, time::Duration};

/// Maximum time the wait helpers wait for a condition, e.g. for a node to sync to or commit a
/// block.
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Interval at which [`poll_until`] polls.
pub const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Calls `poll` every [`POLL_INTERVAL`] until it returns a value.
///
/// Returns an error describing `what` was awaited if the condition is not met within
/// [`WAIT_TIMEOUT`], or the first error returned by `poll`.
pub async fn poll_until<T, F, Fut>(what: impl Display, mut poll: F) -> eyre::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = eyre::Result<Option<T>>>,
{
    let wait = async {
        loop {
            if let Some(value) = poll().await? {
                return Ok(value)
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    };
    tokio::time::timeout(WAIT_TIMEOUT, wait)
        .await
        .map_err(|_| eyre!("timed out waiting for {what}"))?
}
