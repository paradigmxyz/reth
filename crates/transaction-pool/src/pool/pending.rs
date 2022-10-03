use crate::traits::PoolTransaction;

/// A pool of validated transactions that are ready on the current state and are waiting to be
/// included in a block.
///
/// Each transaction in this pool is valid on its own, i.e. they are not dependent on transaction
/// that must be executed first. Each of these transaction can be executed independently on the
/// current state
pub struct PendingTransactions<T: PoolTransaction> {
    i: T,
}
