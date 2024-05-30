/// Default budget to try and drain streams.
///
/// Default is 10 iterations.
pub const DEFAULT_BUDGET_TRY_DRAIN_STREAM: u32 = 10;

/// Default budget to try and drain [`Swarm`](crate::swarm::Swarm).
///
/// Default is 10 [`SwarmEvent`](crate::swarm::SwarmEvent)s.
pub const DEFAULT_BUDGET_TRY_DRAIN_SWARM: u32 = 10;

/// Default budget to try and drain pending messages from [`NetworkHandle`](crate::NetworkHandle)
/// channel. Polling the [`TransactionsManager`](crate::transactions::TransactionsManager) future
/// sends these types of messages.
//
// Default is 40 outgoing transaction messages.
pub const DEFAULT_BUDGET_TRY_DRAIN_NETWORK_HANDLE_CHANNEL: u32 =
    4 * DEFAULT_BUDGET_TRY_DRAIN_STREAM;

/// Default budget to try and drain stream of
/// [`NetworkTransactionEvent`](crate::transactions::NetworkTransactionEvent)s from
/// [`NetworkManager`](crate::NetworkManager).
///
/// Default is 10 incoming transaction messages.
pub const DEFAULT_BUDGET_TRY_DRAIN_NETWORK_TRANSACTION_EVENTS: u32 = DEFAULT_BUDGET_TRY_DRAIN_SWARM;

/// Default budget to try and flush pending pool imports to pool. This number reflects the number
/// of transactions that can be queued for import to pool in each iteration of the loop in the
/// [`TransactionsManager`](crate::transactions::TransactionsManager) future.
//
// Default is 40 pending pool imports.
pub const DEFAULT_BUDGET_TRY_DRAIN_PENDING_POOL_IMPORTS: u32 = 4 * DEFAULT_BUDGET_TRY_DRAIN_STREAM;

/// Default budget to try and stream hashes of successfully imported transactions from the pool.
///
/// Default is naturally same as the number of transactions to attempt importing,
/// [`DEFAULT_BUDGET_TRY_DRAIN_PENDING_POOL_IMPORTS`], so 40 pool imports.
pub const DEFAULT_BUDGET_TRY_DRAIN_POOL_IMPORTS: u32 =
    DEFAULT_BUDGET_TRY_DRAIN_PENDING_POOL_IMPORTS;

/// Polls the given stream. Breaks with `true` if there maybe is more work.
#[macro_export]
macro_rules! poll_nested_stream_with_budget {
    ($target:literal, $label:literal, $budget:ident, $poll_stream:expr, $on_ready_some:expr $(, $on_ready_none:expr;)? $(,)?) => {{
        let mut budget: u32 = $budget;

            loop {
                match $poll_stream {
                    Poll::Ready(Some(item)) => {
                        #[allow(unused_mut)]
                        let mut f = $on_ready_some;
                        f(item);

                        budget = budget.saturating_sub(1);
                        if budget == 0 {
                            break true
                        }
                    }
                    Poll::Ready(None) => {
                        $($on_ready_none;)? // todo: handle error case with $target and $label
                        break false
                    }
                    Poll::Pending => break false,
                }
            }
    }};
}

/// Metered poll of the given stream. Breaks with `true` if there maybe is more work.
#[macro_export]
macro_rules! metered_poll_nested_stream_with_budget {
    ($acc:ident, $target:literal, $label:literal, $budget:ident, $poll_stream:expr, $on_ready_some:expr $(, $on_ready_none:expr;)? $(,)?) => {{
        duration_metered_exec!(
            {
                $crate::poll_nested_stream_with_budget!($target, $label, $budget, $poll_stream, $on_ready_some $(, $on_ready_none;)?)
            },
            $acc
        )
    }};
}
