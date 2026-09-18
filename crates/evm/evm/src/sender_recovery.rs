use alloc::{sync::Arc, vec::Vec};
use alloy_primitives::{map::FbBuildHasher, Address, B256};
use reth_primitives_traits::{
    transaction::{recover::recover_signers, signed::RecoveryError},
    SignedTransaction,
};

/// Number of entries retained in the default sender recovery cache.
const SENDER_RECOVERY_CACHE_CAPACITY: usize = 1 << 17;

/// Shared cache of recovered transaction senders.
///
/// Sender recovery is performed when a transaction enters the pool and again when the same
/// transaction is received in an execution payload or a builder block submission. Sharing this
/// bounded, lock-free cache lets payload prewarming and builder block validation reuse the result
/// produced by transaction-pool ingress.
///
/// Cached senders are trusted without re-checking the signature, so only senders recovered from
/// the transaction with the keyed hash are ever inserted.
#[derive(Clone, Debug)]
pub struct SenderRecoveryCache {
    cache: Arc<fixed_cache::Cache<B256, Address, FbBuildHasher<32>, SenderRecoveryCacheConfig>>,
}

impl SenderRecoveryCache {
    /// Creates a sender recovery cache with the given capacity.
    ///
    /// The capacity must satisfy [`fixed_cache::Cache`] capacity requirements.
    pub fn new(capacity: usize) -> Self {
        Self { cache: Arc::new(fixed_cache::Cache::new(capacity, FbBuildHasher::<32>::default())) }
    }

    /// Returns a cached sender for the transaction hash.
    #[inline]
    pub fn get(&self, tx_hash: &B256) -> Option<Address> {
        self.cache.get(tx_hash)
    }

    /// Returns the cached sender or recovers and caches it on a miss.
    ///
    /// Failed recoveries are not cached.
    #[inline]
    pub fn recover<T: SignedTransaction>(&self, transaction: &T) -> Result<Address, RecoveryError> {
        self.cache.get_or_try_insert_with_ref(
            transaction.tx_hash(),
            |_| transaction.try_recover(),
            |hash| *hash,
        )
    }

    /// Recovers the senders of the given transactions, in transaction order.
    ///
    /// Cached senders are reused. The remaining transactions are recovered together, in parallel
    /// when the `rayon` feature of `reth-primitives-traits` is enabled, and their senders are
    /// cached. Nothing is cached if any recovery fails.
    pub fn recover_signers<T: SignedTransaction>(
        &self,
        transactions: &[T],
    ) -> Result<Vec<Address>, RecoveryError> {
        let mut senders = Vec::with_capacity(transactions.len());
        let mut misses = Vec::new();
        for (index, transaction) in transactions.iter().enumerate() {
            match self.get(transaction.tx_hash()) {
                Some(sender) => senders.push(sender),
                None => {
                    // Placeholder that is overwritten once the miss has been recovered below.
                    senders.push(Address::ZERO);
                    misses.push(index);
                }
            }
        }

        if misses.is_empty() {
            return Ok(senders)
        }

        let recovered =
            recover_signers(misses.iter().map(|&index| &transactions[index]).collect::<Vec<_>>())?;
        for (index, sender) in misses.into_iter().zip(recovered) {
            self.cache.insert(*transactions[index].tx_hash(), sender);
            senders[index] = sender;
        }

        Ok(senders)
    }
}

impl Default for SenderRecoveryCache {
    fn default() -> Self {
        Self::new(SENDER_RECOVERY_CACHE_CAPACITY)
    }
}

struct SenderRecoveryCacheConfig;

impl fixed_cache::CacheConfig for SenderRecoveryCacheConfig {
    const STATS: bool = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::TxLegacy;
    use alloy_primitives::{Signature, U256};
    use reth_ethereum_primitives::{Transaction, TransactionSigned};

    #[test]
    fn recover_populates_cache() {
        let transaction = TransactionSigned::new_unhashed(
            Transaction::Legacy(TxLegacy::default()),
            Signature::test_signature(),
        );
        let cache = SenderRecoveryCache::new(4);
        let shared_cache = cache.clone();

        let sender = cache.recover(&transaction).unwrap();

        assert_eq!(shared_cache.get(transaction.tx_hash()), Some(sender));
        assert_eq!(shared_cache.recover(&transaction).unwrap(), sender);
    }

    #[test]
    fn failed_recovery_is_not_cached() {
        let transaction = TransactionSigned::new_unhashed(
            Transaction::Legacy(TxLegacy::default()),
            Signature::new(U256::ZERO, U256::ZERO, false),
        );
        let cache = SenderRecoveryCache::new(4);

        assert!(cache.recover(&transaction).is_err());
        assert_eq!(cache.get(transaction.tx_hash()), None);
    }

    /// Returns a validly signed transaction whose sender depends on the nonce.
    fn signed_transaction(nonce: u64) -> TransactionSigned {
        TransactionSigned::new_unhashed(
            Transaction::Legacy(TxLegacy { nonce, ..Default::default() }),
            Signature::test_signature(),
        )
    }

    #[test]
    fn recover_signers_reuses_cached_senders_and_caches_misses() {
        let transactions: Vec<_> = (0..4).map(signed_transaction).collect();
        let recovered: Vec<_> =
            transactions.iter().map(|transaction| transaction.try_recover().unwrap()).collect();
        let cache = SenderRecoveryCache::default();

        // Stands in for a sender recovered by another component: it can only be returned if
        // the hit skipped signature recovery.
        let cached_sender = Address::repeat_byte(0xaa);
        cache.cache.insert(*transactions[1].tx_hash(), cached_sender);

        let senders = cache.recover_signers(&transactions).unwrap();

        assert_eq!(senders, [recovered[0], cached_sender, recovered[2], recovered[3]]);
        for (transaction, sender) in transactions.iter().zip(&senders) {
            assert_eq!(cache.get(transaction.tx_hash()), Some(*sender));
        }
        assert!(cache.recover_signers::<TransactionSigned>(&[]).unwrap().is_empty());
    }

    #[test]
    fn recover_signers_rejects_invalid_signature_without_caching() {
        let valid = signed_transaction(0);
        let invalid = TransactionSigned::new_unhashed(
            Transaction::Legacy(TxLegacy::default()),
            Signature::new(U256::ZERO, U256::ZERO, false),
        );
        let cache = SenderRecoveryCache::default();

        assert!(cache.recover_signers(&[valid.clone(), invalid.clone()]).is_err());
        assert_eq!(cache.get(valid.tx_hash()), None);
        assert_eq!(cache.get(invalid.tx_hash()), None);
    }
}
