//! Helpers for waiting on conditions of test nodes.

use eyre::{ensure, eyre};
use std::{fmt::Display, future::Future, time::Duration};
use tokio::time::Instant;

/// Maximum time the wait helpers wait for a condition, e.g. for a node to sync to or commit a
/// block.
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Interval at which [`poll_until`] polls, [`assert_holds_for`] checks and
/// [`NodeTestContext::advance_while`] advances the chain.
///
/// [`NodeTestContext::advance_while`]: crate::node::NodeTestContext::advance_while
pub const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Calls `poll` every [`POLL_INTERVAL`] until it returns a value.
///
/// Returns an error describing `what` was awaited if the condition is not met within
/// [`WAIT_TIMEOUT`], or the first error returned by `poll`. This is [`poll_until_with`] with the
/// default [`PollOpts`], see there for when `poll` is called.
///
/// `what` is held across awaits, so it must be `Send` for the returned future to be `Send`. This
/// rules out `format_args!`, use `format!` instead.
pub async fn poll_until<T, F, Fut>(what: impl Display + Send, poll: F) -> eyre::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = eyre::Result<Option<T>>>,
{
    poll_until_with(PollOpts::default(), what, poll).await
}

/// Calls `poll` until it returns a value, with the timeout and interval of `opts`.
///
/// The first poll starts immediately, and every further poll `opts.interval` after the previous
/// one returned `None`. The wait ends `opts.timeout` after the call: a poll that is still running
/// is dropped, and there is no final poll at the deadline, so a condition that only becomes true
/// during the last interval is reported as timed out, and an interval at least as long as the
/// timeout polls once.
///
/// Returns the value of the first poll that returns `Some`, the first error returned by `poll`, or
/// an error naming `what` and the timeout, e.g. `timed out after 30s waiting for block 1`.
///
/// `what` is held across awaits, so it must be `Send` for the returned future to be `Send`. This
/// rules out `format_args!`, use `format!` instead.
///
/// ```ignore
/// let receipt = poll_until_with(
///     PollOpts { interval: Duration::from_secs(1), ..Default::default() },
///     format!("receipt of transaction {tx_hash}"),
///     || async { Ok(provider.get_transaction_receipt(tx_hash).await?) },
/// )
/// .await?;
/// ```
pub async fn poll_until_with<T, F, Fut>(
    opts: PollOpts,
    what: impl Display + Send,
    mut poll: F,
) -> eyre::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = eyre::Result<Option<T>>>,
{
    let PollOpts { timeout, interval } = opts;
    let wait = async {
        loop {
            if let Some(value) = poll().await? {
                return Ok(value)
            }
            tokio::time::sleep(interval).await;
        }
    };
    tokio::time::timeout(timeout, wait)
        .await
        .map_err(|_| eyre!("timed out after {timeout:?} waiting for {what}"))?
}

/// Asserts that the condition checked by `check` holds for `duration`.
///
/// Use this instead of a fixed sleep followed by an assertion to verify that something does not
/// happen, e.g. that a transaction stays in the pool. `check` returns whether the condition still
/// holds, as a future like the poll of [`poll_until`], so it can query the node over RPC and
/// return errors with `?`.
///
/// The first check starts immediately, every further check [`POLL_INTERVAL`] after the previous
/// one returned, and the last check starts at `duration` after the call, so the condition is
/// verified at both ends of `duration`. A condition that breaks and recovers between two checks
/// goes unnoticed. Checks are not cancelled, so a successful call takes `duration` plus the time of
/// the last check.
///
/// Returns the first error returned by `check`, or an error as soon as a check returns `false`,
/// naming `what` and the time from the call to the start of that check, e.g. `expected
/// transaction to stay in the pool for 2s, but it did not hold after 1.24s`.
///
/// `what` is held across awaits, so it must be `Send` for the returned future to be `Send`. This
/// rules out `format_args!`, use `format!` instead.
pub async fn assert_holds_for<F, Fut>(
    duration: Duration,
    what: impl Display + Send,
    mut check: F,
) -> eyre::Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = eyre::Result<bool>>,
{
    let start = Instant::now();
    let deadline = start + duration;
    loop {
        let checked_at = Instant::now();
        ensure!(
            check().await?,
            "expected {what} for {duration:?}, but it did not hold after {:?}",
            checked_at - start
        );
        if checked_at >= deadline {
            return Ok(())
        }
        tokio::time::sleep_until(deadline.min(Instant::now() + POLL_INTERVAL)).await;
    }
}

/// Timeout and interval of a single [`poll_until_with`] call.
///
/// The default is [`WAIT_TIMEOUT`] and [`POLL_INTERVAL`], which [`poll_until`] uses, so set only
/// what differs, e.g. `PollOpts { interval: Duration::from_secs(1), ..Default::default() }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollOpts {
    /// Maximum time to wait for the condition, from the start of the wait.
    pub timeout: Duration,
    /// Time between a poll returning `None` and the start of the next poll.
    pub interval: Duration,
}

impl Default for PollOpts {
    fn default() -> Self {
        Self { timeout: WAIT_TIMEOUT, interval: POLL_INTERVAL }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn poll_until_with_times_out_without_final_poll() {
        let mut polls = 0;
        let opts = PollOpts { timeout: Duration::from_millis(50), interval: WAIT_TIMEOUT };
        let err = poll_until_with(opts, "condition", || {
            polls += 1;
            async { Ok(None::<()>) }
        })
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "timed out after 50ms waiting for condition");
        assert_eq!(polls, 1);
    }

    #[tokio::test]
    async fn poll_until_with_returns_first_value() {
        let mut polls = 0;
        let opts = PollOpts { interval: Duration::from_millis(1), ..Default::default() };
        let value = poll_until_with(opts, "third poll", || {
            polls += 1;
            let value = (polls == 3).then_some(polls);
            async move { Ok(value) }
        })
        .await
        .unwrap();
        assert_eq!(value, 3);
    }

    #[tokio::test]
    async fn assert_holds_for_returns_after_duration() {
        let duration = Duration::from_millis(50);
        let start = Instant::now();
        assert_holds_for(duration, "condition", || async { Ok(true) }).await.unwrap();
        assert!(start.elapsed() >= duration);
    }

    #[tokio::test]
    async fn assert_holds_for_fails_when_condition_breaks() {
        let mut checks = 0;
        let err = assert_holds_for(WAIT_TIMEOUT, "condition", || {
            checks += 1;
            let holds = checks < 2;
            async move { Ok(holds) }
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(
            err.starts_with("expected condition for 60s, but it did not hold after "),
            "unexpected error: {err}"
        );
        assert_eq!(checks, 2);
    }

    #[tokio::test]
    async fn assert_holds_for_checks_at_deadline() {
        let duration = Duration::from_millis(50);
        let deadline = Instant::now() + duration;
        let result = assert_holds_for(duration, "condition", || {
            let holds = Instant::now() < deadline;
            async move { Ok(holds) }
        })
        .await;
        assert!(result.is_err(), "condition broken at the deadline was not checked");
    }
}
