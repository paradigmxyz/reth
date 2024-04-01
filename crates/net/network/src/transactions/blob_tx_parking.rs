//! Park blob transactions that failed import to pool due to nonce gap.

use std::{collections::HashMap, fmt::Debug, sync::Arc};

use itertools::Itertools;
use reth_primitives::Address;
use reth_transaction_pool::{PoolTransaction, ValidPoolTransaction};
use tracing::trace;

/// Parking for blob transactions that were denied import to pool due to a nonce gap.
#[derive(Debug)]
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
    pub fn park(&mut self, valid_tx: Arc<ValidPoolTransaction<T>>) {
        // log warning or metric for how many are already pending. when to discard? lru doesn't
        // make sense here, rather opposite
        let sender = valid_tx.sender();
        let Some(parked_txns) = self.inner.get_mut(&sender) else {
            _ = self.inner.insert(sender, ParkedBlobTxns::new(valid_tx));
            return
        };

        parked_txns.park(valid_tx)
    }

    /// Unparks any blob transactions that are now canonical due to the successful import.
    pub fn on_successful_import(
        &mut self,
        sender: &Address,
        nonce: u64,
    ) -> Option<Vec<Arc<ValidPoolTransaction<T>>>> {
        let parked_txns = self.inner.get_mut(sender)?;

        let txns_to_reimport = parked_txns.unpark_transactions(nonce)?;

        if parked_txns.txns.is_empty() {
            _ = self.inner.remove(sender);
        }

        trace!(target: "net::tx::blob_tx_parking",
            %sender,
            tx_pool_highest_nonce=nonce,
            unparked_nonce_range=?txns_to_reimport.first().unwrap().nonce()
                ..=txns_to_reimport.last().unwrap().nonce(),
            txns=format!("[{}]", txns_to_reimport.iter().map(|tx| tx.hash()).format(",")),
            "unparked blob transactions for sender"
        );

        Some(txns_to_reimport)
    }
}

impl<T> Default for BlobTxNonceParking<T>
where
    T: PoolTransaction,
{
    fn default() -> Self {
        Self { inner: HashMap::new() }
    }
}

/// Blob transactions from some sender, that are parked.
#[derive(Debug)]
pub struct ParkedBlobTxns<T>
where
    T: PoolTransaction,
{
    /// Lowest parked nonce. Note: this is not exact, lowest nonce will not be update upon
    /// unparking transactions.
    lowest_nonce: u64,
    /// Maps a nonce to the transaction that comes after it.
    txns: HashMap<u64, Arc<ValidPoolTransaction<T>>>,
}

impl<T> ParkedBlobTxns<T>
where
    T: PoolTransaction,
{
    /// Returns a new instance, inserting the give transaction.
    pub fn new(valid_tx: Arc<ValidPoolTransaction<T>>) -> Self {
        let nonce = valid_tx.nonce();
        // hash map grows in powers of 2, avoid first two reorgs
        let mut txns = HashMap::with_capacity(4);
        txns.insert(nonce, valid_tx);

        Self { lowest_nonce: nonce, txns }
    }

    /// Inserts a valid transaction.
    pub fn park(&mut self, valid_tx: Arc<ValidPoolTransaction<T>>) {
        let nonce = valid_tx.nonce();
        if nonce < self.lowest_nonce {
            self.lowest_nonce = nonce;
        }
        _ = self.txns.insert(nonce, valid_tx)
    }

    /// Unparks any transactions that are canonical w.r.t. the given nonce.
    pub fn unpark_transactions(
        &mut self,
        canonical_nonce: u64,
    ) -> Option<Vec<Arc<ValidPoolTransaction<T>>>> {
        let canonical_nonce = canonical_nonce + 1;
        if self.lowest_nonce > canonical_nonce {
            return None
        }
        let mut canonical_txns = Vec::with_capacity(self.txns.len());
        for nonce in self.lowest_nonce..canonical_nonce {
            if let Some(tx) = self.txns.remove(&nonce) {
                canonical_txns.push(tx)
            }
        }
        self.lowest_nonce = canonical_nonce;
        while let Some(tx) = self.txns.remove(&(self.lowest_nonce)) {
            self.lowest_nonce += 1;
            canonical_txns.push(tx)
        }
        canonical_txns.shrink_to_fit();

        Some(canonical_txns)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use reth_tracing::init_test_tracing;
    use reth_transaction_pool::{
        test_utils::{MockTransaction, MockTransactionFactory},
        TransactionOrigin,
    };

    use super::*;

    #[derive(Default)]
    struct MockValidTransactionFactory(MockTransactionFactory);

    impl MockValidTransactionFactory {
        fn new_tx(
            &mut self,
            sender: Address,
            nonce: u64,
        ) -> Arc<ValidPoolTransaction<MockTransaction>> {
            let mut transaction = MockTransaction::eip4844();
            transaction.set_nonce(nonce);
            transaction.set_sender(sender);
            let transaction_id = self.0.tx_id(&transaction);

            Arc::new(ValidPoolTransaction {
                transaction,
                transaction_id,
                propagate: false,
                timestamp: Instant::now(),
                origin: TransactionOrigin::External,
            })
        }
    }

    #[test]
    fn test_park_and_unpark_transactions() {
        init_test_tracing();

        let mut tx_factory = MockValidTransactionFactory::default();

        // rig

        let mut highest_nonce_in_pool = 0;
        let mut parking: BlobTxNonceParking<MockTransaction> = BlobTxNonceParking::default();

        let sender = Address::random();

        // note: only txns with nonce 3, 5, 6 and 7 are used, but makes easier if index == nonce
        let mut txns = Vec::with_capacity(7);
        for i in 0..=7 {
            txns.push(tx_factory.new_tx(sender, i as u64));
        }
        // park a transaction with nonce 3. tx is waiting for txns with nonce 1 and 2 to be
        // imported again.
        parking.park(txns[3].clone());

        // park three consecutive transactions.
        for i in 5..=7 {
            parking.park(txns[i].clone());
        }

        // test

        // tx with nonce 1 was imported
        highest_nonce_in_pool += 1;
        let unparked_txns = parking.on_successful_import(&sender, highest_nonce_in_pool);
        assert!(unparked_txns.is_none());

        // next tx was imported, nonce 2, this unparks tx with nonce 3
        highest_nonce_in_pool += 1;
        let unparked_txns = parking.on_successful_import(&sender, highest_nonce_in_pool);
        assert_eq!(vec!(txns[3].clone()), unparked_txns.unwrap());

        // next tx was imported, nonce 4, this unparks txns with nonce 5, 6 and 7
        highest_nonce_in_pool += 2;
        let unparked_txns = parking.on_successful_import(&sender, highest_nonce_in_pool);
        assert_eq!(vec!(txns[5].clone(), txns[6].clone(), txns[7].clone()), unparked_txns.unwrap());
    }
}
