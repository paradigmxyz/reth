//! EVM database types used by the RPC state cache.

use alloy_eip7928::{bal::DecodedBal, BlockAccessIndex};
use alloy_primitives::{Address, B256, U256};
use reth_errors::ProviderError;
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::StateProviderBox;
use revm::{
    database::{bal::BalState, State},
    state::{bal::Bal as RevmBal, AccountInfo, Bytecode},
    Database, DatabaseRef,
};
use std::sync::Arc;

/// Helper alias type for the state's [`State`]
pub type StateCacheDb = State<StateProviderDatabase<StateProviderBox>>;

/// Mutable call state backed by cached end-of-block BAL values.
pub type CallStateDb = State<BalPostStateDatabase<StateProviderDatabase<StateProviderBox>>>;

/// Serves cached BAL post-state beneath mutable call state and overrides.
///
/// The underlying database must serve the post-state of the same block as the BAL. This keeps
/// fallback reads, account existence, and state-root computation consistent with the cached values.
#[derive(Debug)]
pub struct BalPostStateDatabase<DB> {
    database: DB,
    bal_state: BalState,
}

impl<DB> BalPostStateDatabase<DB> {
    /// Wraps the block's post-state database with its optional cached BAL.
    pub fn new(database: DB, bal: Option<Arc<RevmBal>>) -> Self {
        Self {
            database,
            bal_state: BalState {
                bal,
                // BAL reads are exclusive. A cursor beyond all valid indices includes the
                // post-execution writes without needing the block's transaction count.
                bal_index: BlockAccessIndex::new(u64::MAX),
                allow_db_fallback: true,
                ..Default::default()
            },
        }
    }
}

impl<DB: DatabaseRef<Error = ProviderError>> DatabaseRef for BalPostStateDatabase<DB> {
    type Error = ProviderError;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let mut account = self.database.basic_ref(address)?;
        // A zero balance write can belong to an account removed by state clearing. Preserve
        // the post-state provider's absence instead of recreating an empty account from the BAL.
        if account.is_some() {
            self.bal_state.basic(address, &mut account).map_err(ProviderError::Bal)?;
        }
        Ok(account)
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.database.code_by_hash_ref(code_hash)
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        if let Some(value) = self.bal_state.storage(&address, index).map_err(ProviderError::Bal)? {
            return Ok(value)
        }
        self.database.storage_ref(address, index)
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.database.block_hash_ref(number)
    }
}

impl<DB: DatabaseRef<Error = ProviderError>> Database for BalPostStateDatabase<DB> {
    type Error = ProviderError;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.basic_ref(address)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.code_by_hash_ref(code_hash)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.storage_ref(address, index)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.block_hash_ref(number)
    }
}

/// Attaches `bal` to the database, positioned at the state right before the transaction at
/// `tx_index`.
///
/// Reads served by the attached BAL reflect all writes prior to the transaction, including the
/// block's pre-execution system calls. Reads not covered by the BAL fall back to the underlying
/// database, which holds the correct values for all state the block does not touch.
///
/// Note: changes must not be committed to the database afterwards, because the attached BAL takes
/// precedence over committed state when serving reads.
#[inline]
pub fn attach_bal_before_tx<DB: Database>(
    db: &mut State<DB>,
    bal: &DecodedBal<Arc<RevmBal>>,
    tx_index: usize,
) {
    db.set_bal(Some(bal.as_bal().clone()));
    db.set_allow_bal_db_fallback(true);
    db.set_bal_index(BlockAccessIndex::from_tx_index(tx_index as u64));
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_evm::{eth::EthEvmFactory, overrides::apply_state_overrides, Evm, EvmFactory};
    use alloy_primitives::{address, Bytes, U256};
    use alloy_rpc_types_eth::state::AccountOverride;
    use revm::{
        context::TxEnv,
        database::{CacheDB, EmptyDB},
        database_interface::EmptyDBTyped,
        state::{
            bal::{AccountBal, Bal, BalWrites, BlockAccessIndex},
            AccountInfo,
        },
    };
    use std::{cell::Cell, iter::once};

    const CONTRACT: Address = Address::repeat_byte(0x11);

    #[test]
    fn bal_post_state_reads_include_post_execution_and_fall_back() {
        let (database, bal) = post_state_fixture();
        let db = BalPostStateDatabase::new(database, Some(bal));

        assert_eq!(db.storage_ref(CONTRACT, U256::ZERO).unwrap(), U256::from(42));
        assert_eq!(db.database.storage_reads.get(), 0);
        // Read-only slots and addresses outside the BAL still use the provider.
        assert_eq!(db.storage_ref(CONTRACT, U256::from(1)).unwrap(), U256::from(99));
        assert_eq!(db.storage_ref(Address::ZERO, U256::ZERO).unwrap(), U256::ZERO);
        assert_eq!(db.database.storage_reads.get(), 2);

        let account = db.basic_ref(CONTRACT).unwrap().unwrap();
        assert_eq!(account.nonce, 1);
        assert_eq!(account.balance, U256::from(7));
        assert!(account.code.is_some());
    }

    #[test]
    fn bal_post_state_preserves_overrides_and_sequential_calls() {
        for cached in [false, true] {
            let (database, bal) = post_state_fixture();
            let mut state = State::builder()
                .with_database(BalPostStateDatabase::new(database, cached.then_some(bal)))
                .with_bundle_update()
                .build();
            apply_state_overrides(
                once((
                    CONTRACT,
                    AccountOverride {
                        balance: Some(U256::from(123)),
                        state_diff: Some(once((B256::ZERO, B256::from(U256::from(100)))).collect()),
                        ..Default::default()
                    },
                ))
                .collect(),
                &mut state,
            )
            .unwrap();

            // Each call increments slot zero and returns its new value. Later calls must read
            // the simulated writes, even if the base BAL contains an older value for the slot.
            for nonce in 0..2 {
                let mut evm = EthEvmFactory::default().create_evm(&mut state, Default::default());
                let result = evm
                    .transact_commit(TxEnv {
                        caller: Address::repeat_byte(0x22),
                        kind: CONTRACT.into(),
                        gas_limit: 100_000,
                        nonce,
                        ..Default::default()
                    })
                    .unwrap();
                assert!(result.is_success());
                assert_eq!(U256::from_be_slice(result.output().unwrap()), U256::from(101 + nonce));
            }
            assert_eq!(state.basic(CONTRACT).unwrap().unwrap().balance, U256::from(123));
            assert_eq!(state.storage(CONTRACT, U256::ZERO).unwrap(), U256::from(102));
            assert_eq!(state.database.storage_ref(CONTRACT, U256::ZERO).unwrap(), U256::from(42));

            // A full storage override also clears slots covered by the BAL and provider.
            apply_state_overrides(
                once((
                    CONTRACT,
                    AccountOverride { state: Some(Default::default()), ..Default::default() },
                ))
                .collect(),
                &mut state,
            )
            .unwrap();
            assert_eq!(state.storage(CONTRACT, U256::ZERO).unwrap(), U256::ZERO);
            assert_eq!(state.storage(CONTRACT, U256::from(1)).unwrap(), U256::ZERO);
        }
    }

    #[test]
    fn bal_post_state_preserves_absent_accounts() {
        let mut account = AccountBal::default();
        account.account_info.balance = BalWrites::new(vec![(BlockAccessIndex::new(1), U256::ZERO)]);
        let bal = Arc::new(once((CONTRACT, account)).collect());
        let db = BalPostStateDatabase::new(EmptyDBTyped::<ProviderError>::default(), Some(bal));
        assert!(db.basic_ref(CONTRACT).unwrap().is_none());
    }

    fn post_state_fixture() -> (CountingDatabase, Arc<RevmBal>) {
        // Increment slot zero and return its new value.
        let code = Bytecode::new_raw(
            alloy_primitives::hex!("6000546001018060005560005260206000f3").into(),
        );
        let mut database = CacheDB::new(EmptyDBTyped::<ProviderError>::default());
        database.insert_account_info(
            CONTRACT,
            AccountInfo {
                nonce: 1,
                balance: U256::from(7),
                code_hash: code.hash_slow(),
                code: Some(code.clone()),
                ..Default::default()
            },
        );
        database.insert_account_storage(CONTRACT, U256::ZERO, U256::from(42)).unwrap();
        database.insert_account_storage(CONTRACT, U256::from(1), U256::from(99)).unwrap();

        let mut account = AccountBal::default();
        // A two-transaction block writes again during post-execution (index 3).
        account.storage.storage.insert(
            U256::ZERO,
            BalWrites::new(vec![
                (BlockAccessIndex::new(1), U256::from(10)),
                (BlockAccessIndex::new(3), U256::from(42)),
            ]),
        );
        account.storage.storage.insert(U256::from(1), BalWrites::default());
        account.account_info.balance =
            BalWrites::new(vec![(BlockAccessIndex::new(3), U256::from(7))]);
        account.account_info.code =
            BalWrites::new(vec![(BlockAccessIndex::new(1), (code.hash_slow(), code))]);
        (
            CountingDatabase { database, storage_reads: Cell::new(0) },
            Arc::new(once((CONTRACT, account)).collect()),
        )
    }

    #[derive(Debug)]
    struct CountingDatabase {
        database: CacheDB<EmptyDBTyped<ProviderError>>,
        storage_reads: Cell<usize>,
    }

    impl DatabaseRef for CountingDatabase {
        type Error = ProviderError;

        fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
            self.database.basic_ref(address)
        }

        fn code_by_hash_ref(&self, hash: B256) -> Result<Bytecode, Self::Error> {
            self.database.code_by_hash_ref(hash)
        }

        fn storage_ref(&self, address: Address, slot: U256) -> Result<U256, Self::Error> {
            self.storage_reads.set(self.storage_reads.get() + 1);
            self.database.storage_ref(address, slot)
        }

        fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
            self.database.block_hash_ref(number)
        }
    }

    #[test]
    fn attach_bal_before_tx_serves_positioned_reads() {
        let covered = address!("0x0000000000000000000000000000000000000001");
        let uncovered = address!("0x0000000000000000000000000000000000000002");
        let written_slot = U256::from(1);
        let read_slot = U256::from(2);

        // pre-block state
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(
            covered,
            AccountInfo { balance: U256::from(7), nonce: 5, ..Default::default() },
        );
        db.insert_account_storage(covered, written_slot, U256::from(11)).unwrap();
        db.insert_account_storage(covered, read_slot, U256::from(99)).unwrap();
        db.insert_account_info(
            uncovered,
            AccountInfo { balance: U256::from(3), ..Default::default() },
        );

        // tx 1 writes the slot, tx 2 changes the balance
        let mut account = AccountBal::default();
        account.storage.storage.insert(
            written_slot,
            BalWrites::new(vec![(BlockAccessIndex::from_tx_index(1), U256::from(42))]),
        );
        account.account_info.balance =
            BalWrites::new(vec![(BlockAccessIndex::from_tx_index(2), U256::from(1000))]);
        let mut bal = Bal::default();
        bal.accounts.insert(covered, account);
        let bal = DecodedBal::new(Arc::new(bal), Bytes::new());

        let mut state = State::builder().with_database(db).build();

        // before tx 0, none of the block's writes are visible
        attach_bal_before_tx(&mut state, &bal, 0);
        assert_eq!(Database::storage(&mut state, covered, written_slot).unwrap(), U256::from(11));
        assert_eq!(Database::basic(&mut state, covered).unwrap().unwrap().balance, U256::from(7));

        // before tx 2, the storage write of tx 1 is visible, the balance change of tx 2 is not
        attach_bal_before_tx(&mut state, &bal, 2);
        assert_eq!(Database::storage(&mut state, covered, written_slot).unwrap(), U256::from(42));
        assert_eq!(Database::basic(&mut state, covered).unwrap().unwrap().balance, U256::from(7));

        // before tx 3, the balance change of tx 2 is visible
        attach_bal_before_tx(&mut state, &bal, 3);
        assert_eq!(
            Database::basic(&mut state, covered).unwrap().unwrap().balance,
            U256::from(1000)
        );

        // reads not covered by the BAL fall back to the underlying database
        assert_eq!(Database::storage(&mut state, covered, read_slot).unwrap(), U256::from(99));
        assert_eq!(Database::basic(&mut state, uncovered).unwrap().unwrap().balance, U256::from(3));
    }
}
