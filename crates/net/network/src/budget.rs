/// Default budget to try and drain streams.
pub const DEFAULT_BUDGET_TRY_DRAIN_STREAM: u32 = 1024;

/// Budget for polling stream once.
pub const BUDGET_POLL_ONCE: u32 = 1;

/// Default budget to try and drain pending messages from [`NetworkHandle`](crate::NetworkHandle)
/// channel.
pub const DEFAULT_BUDGET_TRY_DRAIN_NETWORK_HANDLE_CHANNEL: u32 = 4 * 1024;

/// Default budget to try and flush pending pool imports to pool. This number reflects the number
/// of transactions that can be queued for import to pool in each iteration of the loop in the
/// [`TransactionsManager`](crate::TransactionsManager) future. from the potential number of
/// pooled imports  Default is 23,04 million ceiled, so 24 million pool imports.
pub const DEFAULT_BUDGET_TRY_DRAIN_PENDING_POOL_IMPORTS: u32 = 24 * 1000000;

/// Default budget to try and stream hashes of successfully imported transactions from the pool.
/// Default is naturally same as the number of transactions to attempt importing,
/// [`DEFAULT_BUDGET_TRY_DRAIN_PENDING_POOL_IMPORTS`], so 24 million pool imports.
pub const DEFAULT_BUDGET_TRY_DRAIN_POOL_IMPORTS: u32 = 24 * 1000000;

/// Polls the given stream. Breaks with `true` if there maybe is more work. Note: this does not
/// register wake up, caller's scope is responsible for doing so.
#[macro_export]
macro_rules! poll_nested_stream_with_yield_points {
    ($target:literal, $label:literal, $budget:ident, $poll_stream:expr, $on_ready_some:expr $(, $on_ready_none:expr;)? $(,)?) => {{
        let mut budget: u32 = $budget;

        loop {
            match $poll_stream {
                Poll::Ready(Some(item)) => {
                    let mut f = $on_ready_some;
                    f(item);

                    budget = budget.saturating_sub(1);
                    if budget == 0 {
                        break true
                    }
                }
                Poll::Ready(None) => {
                    $($on_ready_none;)?
                    break false
                }
                Poll::Pending => break false,
            }
        }
    }};
}
