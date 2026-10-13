//! Local payload-building recovery for unwinding transaction execution panics.

use alloy_primitives::B256;
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_transaction_pool::error::PoolTransactionError;
use std::{
    collections::HashSet,
    fmt,
    panic::{catch_unwind, AssertUnwindSafe},
};
use tracing::error;

/// Bound replay work when several independent transactions panic.
const MAX_REBUILDS: usize = 8;

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

impl PoolTransactionError for TransactionPanic {
    fn is_bad_transaction(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub(super) fn catch_transaction_panic<T>(
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
        PayloadBuilderError::other(TransactionPanic { hash, message })
    })
}

pub(super) fn rebuild_after_panic<T>(
    mut attempt: impl FnMut(&HashSet<B256>, bool) -> Result<T, PayloadBuilderError>,
) -> Result<T, PayloadBuilderError> {
    let mut excluded = HashSet::new();
    loop {
        let result = attempt(&excluded, !excluded.is_empty());
        let Err(PayloadBuilderError::Other(error)) = &result else { return result };
        let Some(panic) = error.downcast_ref::<TransactionPanic>() else { return result };
        error!(
            target: "payload_builder",
            tx_hash = %panic.hash,
            panic_message = %panic.message,
            rebuilds = excluded.len(),
            "transaction execution panicked; discarding build attempt and excluding transaction"
        );
        if excluded.len() == MAX_REBUILDS || !excluded.insert(panic.hash) {
            return result
        }
    }
}

pub(super) fn excluded_transaction(hash: B256) -> TransactionPanic {
    TransactionPanic { hash, message: "excluded after execution panic".to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, Log, TxKind, U256};
    use reth_evm::{eth::EthEvmFactory, precompiles::DynPrecompile, Evm, EvmEnv, EvmFactory};
    use revm::{
        context::{BlockEnv, CfgEnv, TxEnv},
        database::InMemoryDB,
        precompile::PrecompileId,
        primitives::hardfork::SpecId,
        state::AccountInfo,
    };
    use std::{io, panic::panic_any};

    #[test]
    fn rebuild_discards_panicking_precompile_writes_and_logs() {
        let bad = B256::repeat_byte(1);
        let good = B256::repeat_byte(2);
        let caller = Address::repeat_byte(3);
        let recipient = Address::repeat_byte(4);
        let precompile = Address::repeat_byte(5);
        let mut parent_db = InMemoryDB::default();
        parent_db.insert_account_info(
            caller,
            AccountInfo { balance: U256::from(1_000_000), ..Default::default() },
        );
        let env = EvmEnv::new(CfgEnv::new_with_spec(SpecId::BERLIN), BlockEnv::default());
        let mut attempts = 0;
        let (db, results) = rebuild_after_panic(|excluded, retry| {
            attempts += 1;
            assert_eq!(retry, attempts > 1);
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
                if excluded.contains(&hash) {
                    continue
                }
                let result = catch_transaction_panic(hash, || {
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
            Ok((evm.into_db(), results))
        })
        .unwrap();
        assert_eq!(attempts, 2);
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
    fn ordinary_errors_are_not_retried() {
        let mut attempts = 0;
        let result = rebuild_after_panic::<()>(|_, _| {
            attempts += 1;
            Err(PayloadBuilderError::MissingPayload)
        });
        assert!(matches!(result, Err(PayloadBuilderError::MissingPayload)));
        assert_eq!(attempts, 1);
    }

    #[test]
    fn unrelated_internal_errors_are_not_retried() {
        let mut attempts = 0;
        let result = rebuild_after_panic::<()>(|_, _| {
            attempts += 1;
            Err(PayloadBuilderError::other(io::Error::other("provider failure")))
        });
        assert_eq!(result.unwrap_err().to_string(), "provider failure");
        assert_eq!(attempts, 1);
    }

    #[test]
    fn successful_execution_errors_are_preserved() {
        let result = catch_transaction_panic(B256::ZERO, || Err::<(), _>("invalid nonce"));
        assert_eq!(result.unwrap(), Err("invalid nonce"));
    }

    #[test]
    fn multiple_panics_are_bounded() {
        let mut attempts = 0;
        let result = rebuild_after_panic::<()>(|excluded, _| {
            attempts += 1;
            let hash = B256::repeat_byte(excluded.len() as u8);
            catch_transaction_panic(hash, || panic!("injected panic"))
        });
        assert!(result.is_err());
        assert_eq!(attempts, MAX_REBUILDS + 1);
    }

    #[test]
    fn repeated_panicking_hash_is_not_retried_forever() {
        let mut attempts = 0;
        let result = rebuild_after_panic::<()>(|_, _| {
            attempts += 1;
            catch_transaction_panic(B256::ZERO, || panic!("injected panic"))
        });
        assert!(result.is_err());
        assert_eq!(attempts, 2);
    }

    #[test]
    fn panic_payload_destructor_is_not_run() {
        struct PanickingDrop;
        impl Drop for PanickingDrop {
            fn drop(&mut self) {
                panic!("panic payload destructor")
            }
        }
        let result = catch_transaction_panic(B256::ZERO, || panic_any(PanickingDrop));
        let Err(PayloadBuilderError::Other(error)) = result else { panic!("expected panic error") };
        assert_eq!(
            error.to_string(),
            format!("transaction {} panicked: non-string panic payload", B256::ZERO)
        );
    }

    #[test]
    fn exclusions_do_not_blame_peers() {
        assert!(!excluded_transaction(B256::ZERO).is_bad_transaction());
    }
}
