//! Support types for updating the pool.

use crate::{
    identifier::TransactionId, pool::state::SubPool, PoolTransaction, ValidPoolTransaction,
};
use alloy_primitives::TxHash;
use std::sync::Arc;

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

impl From<SubPool> for Destination {
    fn from(sub_pool: SubPool) -> Self {
        Self::Pool(sub_pool)
    }
}

/// Tracks the result after updating the pool
#[derive(Debug)]
pub(crate) struct UpdateOutcome<T: PoolTransaction> {
    /// transactions promoted to the pending pool
    pub(crate) promoted: Vec<Arc<ValidPoolTransaction<T>>>,
    /// transaction that failed and were discarded
    pub(crate) discarded: Vec<Arc<ValidPoolTransaction<T>>>,
}

impl<T: PoolTransaction> Default for UpdateOutcome<T> {
    fn default() -> Self {
        Self { promoted: vec![], discarded: vec![] }
    }
}
