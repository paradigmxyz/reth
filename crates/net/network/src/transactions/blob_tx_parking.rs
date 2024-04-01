//! Park blob transactions that failed import to pool due to nonce gap.

use std::collections::HashMap;

use reth_primitives::Address;
use reth_transaction_pool::{PoolTransaction, ValidPoolTransaction};

/// Parking for blob transactions that were denied import to pool due to a nonce gap.
#[derive(Debug, Default)]
pub struct BlobTxNonceParking<T>
where
    T: PoolTransaction,
{
    // todo: bound, possibly eviction by system time of oldest txn by sender so that some senders
    // aren't able to evict other senders by frequent txns with nonce gaps
    inner: HashMap<Address, ParkedBlobTxns<T>>,
}

impl<T> BlobTxNonceParking<T>
where
    T: PoolTransaction,
{
    /// Parks a valid blob transaction.
    pub fn park(&mut self, valid_tx: ValidPoolTransaction<T>) {
        // log warning or metric for how many are already pending. when to discard? lru doesn't
        // make sense here, rather opposite
        let sender = valid_tx.sender();
        let Some(parked_txns) = self.inner.get_mut(&sender) else {
            _ = self.inner.insert(sender, ParkedBlobTxns::new(valid_tx));
            return
        };

        parked_txns.insert(valid_tx)
    }

    /// Unparks any blob transactions that are now canonical due to the successful import.
    pub fn on_successful_import(
        &mut self,
        sender: &Address,
        nonce: u64,
    ) -> Option<Vec<ValidPoolTransaction<T>>> {
        let parked_txns = self.inner.get_mut(sender)?;

        let txns_to_reimport = parked_txns.unpark_transactions(nonce)?;

        if parked_txns.txns.is_empty() {
            _ = self.inner.remove(sender);
        }

        Some(txns_to_reimport)
    }
}

/// Blob transactions from some sender, that are parked.
#[derive(Debug)]
pub struct ParkedBlobTxns<T>
where
    T: PoolTransaction,
{
    /// Lowest nonce being awaited by parked transactions (lowest key in parked transactions map).
    lowest_pending_nonce: u64,
    /// Maps a nonce to the transaction that comes after it.
    txns: HashMap<u64, ValidPoolTransaction<T>>,
}

impl<T> ParkedBlobTxns<T>
where
    T: PoolTransaction,
{
    /// Returns a new instance, inserting the give transaction.
    pub fn new(valid_tx: ValidPoolTransaction<T>) -> Self {
        let prev_nonce = valid_tx.nonce();
        // hash map grows in powers of 2, avoid first two reorgs
        let mut txns = HashMap::with_capacity(4);
        txns.insert(prev_nonce, valid_tx);

        Self { lowest_pending_nonce: prev_nonce, txns }
    }

    /// Inserts a valid transaction.
    pub fn insert(&mut self, valid_tx: ValidPoolTransaction<T>) {
        let prev_nonce = valid_tx.nonce();
        if prev_nonce < self.lowest_pending_nonce {
            self.lowest_pending_nonce = prev_nonce;
        }
        _ = self.txns.insert(prev_nonce, valid_tx)
    }

    /// Unparks any transactions that are canonical w.r.t. the given nonce.
    pub fn unpark_transactions(
        &mut self,
        canonical_nonce: u64,
    ) -> Option<Vec<ValidPoolTransaction<T>>> {
        if self.lowest_pending_nonce != canonical_nonce {
            return None
        }
        let mut canonical_txns = Vec::with_capacity(self.txns.len());
        while let Some(tx) = self.txns.remove(&(self.lowest_pending_nonce + 1)) {
            self.lowest_pending_nonce += 1;
            canonical_txns.push(tx)
        }
        canonical_txns.shrink_to_fit();

        Some(canonical_txns)
    }
}
