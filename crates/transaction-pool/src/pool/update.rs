//! Support types for updating the pool.
use crate::{identifier::TransactionId, pool::state::SubPool};
use reth_primitives::TxHash;

/// A change of the transaction's location
///
/// NOTE: this guarantees that `current` and `destination` differ.
#[derive(Debug)]
pub(crate) struct PoolUpdate {
    /// Internal tx id.
    pub(crate) id: TransactionId,
    /// Hash of the transaction.
    pub(crate) hash: TxHash,
    /// Where the transaction is currently held.
    pub(crate) current: SubPool,
    /// Where to move the transaction to.
    pub(crate) destination: Destination,
}

/// Where to move an existing transaction.
#[derive(Debug)]
pub(crate) enum Destination {
    /// Discard the transaction.
    Discard,
    /// Move transaction to pool
    Pool(SubPool),
}
