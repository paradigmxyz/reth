//! Containment and pool eviction for unwinding transaction execution panics.

use alloy_primitives::B256;
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_transaction_pool::TransactionPool;
use std::{
    fmt,
    panic::{catch_unwind, AssertUnwindSafe},
};
use tracing::error;

/// An execution panic is local builder policy, not a consensus validation error.
#[derive(Debug)]
pub(super) struct TransactionPanic {
    hash: B256,
    message: String,
}

impl fmt::Display for TransactionPanic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "transaction {} panicked: {}", self.hash, self.message)
    }
}

impl std::error::Error for TransactionPanic {}

pub(super) fn catch_transaction_panic<T>(
    pool: &impl TransactionPool,
    hash: B256,
    execute: impl FnOnce() -> T,
) -> Result<T, PayloadBuilderError> {
    // AssertUnwindSafe permits containment only: the caller must discard the executor and DB
    // on error. It does not establish that any of their invariants survived the unwind.
    catch_unwind(AssertUnwindSafe(execute)).map_err(|panic| {
        let message = match panic.downcast::<String>() {
            Ok(message) => *message,
            Err(panic) => match panic.downcast::<&str>() {
                Ok(message) => (*message).to_owned(),
                Err(panic) => {
                    // An opaque payload may have a panicking destructor. Avoid another unwind
                    // during recovery; only non-string payloads leak on this exceptional path.
                    std::mem::forget(panic);
                    "non-string panic payload".to_owned()
                }
            },
        };
        let removed = pool.remove_transactions_and_descendants(vec![hash]);
        error!(
            target: "payload_builder",
            tx_hash = %hash,
            panic_message = %message,
            removed_transactions = removed.len(),
            "transaction execution panicked; evicting transaction and descendants and failing build"
        );
        PayloadBuilderError::other(TransactionPanic { hash, message })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Signed, TxEip1559};
    use alloy_primitives::{Address, Log, Signature, TxKind, U256};
    use reth_ethereum_primitives::TransactionSigned;
    use reth_evm::{eth::EthEvmFactory, precompiles::DynPrecompile, Evm, EvmEnv, EvmFactory};
    use reth_primitives_traits::Recovered;
    use reth_transaction_pool::{
        blobstore::InMemoryBlobStore,
        noop::{MockTransactionValidator, NoopTransactionPool},
        CoinbaseTipOrdering, EthPooledTransaction, Pool, PoolTransaction, TransactionOrigin,
    };
    use revm::{
        context::{BlockEnv, CfgEnv, TxEnv},
        database::InMemoryDB,
        precompile::PrecompileId,
        primitives::hardfork::SpecId,
        state::AccountInfo,
    };
    use std::{
        future::Future,
        panic::panic_any,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    #[test]
    fn failed_build_discards_precompile_writes_and_logs() {
        let bad = B256::repeat_byte(1);
        let good = B256::repeat_byte(2);
        let caller = Address::repeat_byte(3);
        let recipient = Address::repeat_byte(4);
        let precompile = Address::repeat_byte(5);
        let pool = NoopTransactionPool::default();
        let mut parent_db = InMemoryDB::default();
        parent_db.insert_account_info(
            caller,
            AccountInfo { balance: U256::from(1_000_000), ..Default::default() },
        );
        let env = EvmEnv::new(CfgEnv::new_with_spec(SpecId::BERLIN), BlockEnv::default());
        let build = |include_bad| {
            let mut evm = EthEvmFactory::default().create_evm(parent_db.clone(), env.clone());
            evm.precompiles_mut().apply_precompile(&precompile, |_| {
                Some(DynPrecompile::new_stateful(
                    PrecompileId::Custom("panic-test".into()),
                    move |mut input| {
                        input.internals.sstore(precompile, U256::ZERO, U256::from(42)).unwrap();
                        input.internals.log(Log::new_unchecked(
                            precompile,
                            vec![],
                            Default::default(),
                        ));
                        panic!("injected panic after storage write and log")
                    },
                ))
            });
            let mut results = vec![];
            for (hash, nonce, target) in [(good, 0, recipient), (bad, 1, precompile)] {
                if hash == bad && !include_bad {
                    continue
                }
                let result = catch_transaction_panic(&pool, hash, || {
                    evm.transact_commit(TxEnv {
                        caller,
                        nonce,
                        kind: TxKind::Call(target),
                        value: U256::from(1),
                        gas_limit: 100_000,
                        ..Default::default()
                    })
                })?
                .unwrap();
                results.push(result);
            }
            Ok::<_, PayloadBuilderError>((evm.into_db(), results))
        };

        let error = build(true).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("transaction {bad} panicked: injected panic after storage write and log")
        );
        assert_eq!(parent_db.cache.accounts[&caller].info.nonce, 0);
        assert_eq!(parent_db.cache.accounts[&caller].info.balance, U256::from(1_000_000));
        assert!(!parent_db.cache.accounts.contains_key(&recipient));
        assert!(!parent_db.cache.accounts.contains_key(&precompile));

        // A separate build starts from the parent after the scheduler observes the error.
        let (db, results) = build(false).unwrap();
        assert_eq!(db.cache.accounts[&caller].info.nonce, 1);
        assert_eq!(db.cache.accounts[&caller].info.balance, U256::from(999_999));
        assert_eq!(db.cache.accounts[&recipient].info.balance, U256::from(1));
        assert!(!db.cache.accounts.contains_key(&precompile));
        assert_eq!(results.len(), 1);
        assert!(results[0].is_success());
        assert_eq!(results[0].tx_gas_used(), 21_000);
        assert!(results[0].logs().is_empty());
    }

    #[test]
    fn successful_execution_errors_are_preserved_without_eviction() {
        let pool = test_pool();
        let hash = add_transaction(&pool, Address::repeat_byte(1), 0);
        let result = catch_transaction_panic(&pool, hash, || Err::<(), _>("invalid nonce"));
        assert_eq!(result.unwrap(), Err("invalid nonce"));
        assert!(pool.get(&hash).is_some());
    }

    #[test]
    fn string_panic_payload_is_preserved() {
        let result = catch_transaction_panic(&NoopTransactionPool::default(), B256::ZERO, || {
            panic_any(String::from("owned panic message"))
        });
        assert_eq!(
            result.unwrap_err().to_string(),
            format!("transaction {} panicked: owned panic message", B256::ZERO)
        );
    }

    #[test]
    fn panic_payload_destructor_is_not_run() {
        struct PanickingDrop;
        impl Drop for PanickingDrop {
            fn drop(&mut self) {
                panic!("panic payload destructor")
            }
        }
        let result = catch_transaction_panic(&NoopTransactionPool::default(), B256::ZERO, || {
            panic_any(PanickingDrop)
        });
        assert_eq!(
            result.unwrap_err().to_string(),
            format!("transaction {} panicked: non-string panic payload", B256::ZERO)
        );
    }

    #[test]
    fn panic_fails_once_and_evicts_transaction_and_descendants() {
        let pool = test_pool();
        let sender = Address::repeat_byte(1);
        let bad = add_transaction(&pool, sender, 0);
        let descendant = add_transaction(&pool, sender, 1);
        let good = add_transaction(&pool, Address::repeat_byte(2), 0);
        let mut attempts = 0;
        let result = catch_transaction_panic(&pool, bad, || {
            attempts += 1;
            panic!("injected panic")
        });
        assert_eq!(
            result.unwrap_err().to_string(),
            format!("transaction {bad} panicked: injected panic")
        );
        assert_eq!(attempts, 1);
        assert!(pool.get(&bad).is_none());
        assert!(pool.get(&descendant).is_none());
        // A fresh iterator in a later build cannot select either evicted transaction.
        assert_eq!(pool.best_transactions().map(|tx| *tx.hash()).collect::<Vec<_>>(), vec![good]);
    }

    fn test_pool() -> Pool<
        MockTransactionValidator<EthPooledTransaction>,
        CoinbaseTipOrdering<EthPooledTransaction>,
        InMemoryBlobStore,
    > {
        Pool::new(
            MockTransactionValidator::default(),
            CoinbaseTipOrdering::default(),
            InMemoryBlobStore::default(),
            Default::default(),
        )
    }

    fn add_transaction(
        pool: &impl TransactionPool<Transaction = EthPooledTransaction>,
        sender: Address,
        nonce: u64,
    ) -> B256 {
        let tx = TransactionSigned::from(Signed::new_unhashed(
            TxEip1559 {
                nonce,
                to: TxKind::Call(sender),
                gas_limit: 21_000,
                max_fee_per_gas: 1_000,
                max_priority_fee_per_gas: 1,
                ..Default::default()
            },
            Signature::new(U256::from(1), U256::from(1), false),
        ));
        let tx =
            EthPooledTransaction::try_from_consensus(Recovered::new_unchecked(tx, sender)).unwrap();
        // The mock validator completes synchronously and requires no runtime or I/O.
        let future = pool.add_transaction(TransactionOrigin::External, tx);
        let Poll::Ready(result) = pin!(future).poll(&mut Context::from_waker(Waker::noop())) else {
            panic!("mock validation unexpectedly yielded")
        };
        result.unwrap().hash
    }
}
