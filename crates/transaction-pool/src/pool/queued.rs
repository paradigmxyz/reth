use crate::traits::PoolTransaction;

/// A pool of transactions that are not ready on the current state and are waiting for state changes
/// that turn them valid.
///
/// This could include:
///     - transactions that are waiting until a pending or queued transactions are mined
///     - state changes that turns them valid (e.g. basefee)
pub struct QueuedTransactions<T: PoolTransaction> {
    i: T,
}
