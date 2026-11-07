//! Read-only parent post-state beneath `CachedReads` and the child executor's mutable `State`.

use alloy_primitives::{Address, B256};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, Bal, BalChange, Database},
    interpreter::Word,
};
use reth_execution_types::DecodedEvmBal;
use std::sync::Arc;

pub(super) type SharedBal = Arc<DecodedEvmBal>;

/// The provider must expose the post-state of the same parent as `bal`.
pub(super) struct ParentBalDb<'a, DB> {
    db: DB,
    bal: Option<&'a Bal>,
    reads: ParentBalReads,
}

impl<'a, DB> ParentBalDb<'a, DB> {
    pub(super) fn new(db: DB, bal: Option<&'a Bal>) -> Self {
        Self { db, bal, reads: Default::default() }
    }
}

impl<DB> std::fmt::Debug for ParentBalDb<'_, DB> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParentBalDb").field("has_bal", &self.bal.is_some()).finish_non_exhaustive()
    }
}

impl<DB: Database> Database for ParentBalDb<'_, DB> {
    type Error = DB::Error;

    fn is_fatal(error: &Self::Error) -> bool {
        DB::is_fatal(error)
    }

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        let Some(account) = self.bal.and_then(|bal| bal.accounts.get(address)) else {
            self.reads.provider_accounts += 1;
            return self.db.get_account(address)
        };
        // Select the last write directly, including post-execution writes. Indexed BAL reads
        // are exclusive and positioning at the post-execution index would miss its writes.
        let balance = account.account_info.balance.changes.last().map(|change| *change.value());
        let nonce = account.account_info.nonce.changes.last().map(|change| *change.value());
        let code = account.account_info.code.changes.last().map(BalChange::value);

        // BALs do not encode account existence or storage-wide clears. Only a complete,
        // nonempty account can be reconstructed without consulting the parent state provider.
        // Empty/deleted accounts and partial fields must retain provider existence semantics.
        #[cfg(not(feature = "account-ext"))]
        if let (Some(balance), Some(nonce), Some((code_hash, code))) = (balance, nonce, code) {
            let info = AccountInfo::new(balance, nonce, *code_hash, Some(code.clone()));
            if !info.is_empty() {
                self.reads.bal_accounts += 1;
                return Ok(Some(info))
            }
        }
        self.reads.provider_accounts += 1;
        let Some(mut info) = self.db.get_account(address)? else { return Ok(None) };
        if let Some(balance) = balance {
            info.balance = balance;
        }
        if let Some(nonce) = nonce {
            info.nonce = nonce;
        }
        if let Some((hash, code)) = code {
            info.code_hash = *hash;
            info.code = Some(code.clone());
        }
        Ok(Some(info))
    }

    fn get_storage(&mut self, address: &Address, index: &Word) -> Result<Word, Self::Error> {
        if let Some(value) = self
            .bal
            .and_then(|bal| bal.accounts.get(address))
            .and_then(|account| account.storage.storage.get(index))
            .and_then(|writes| writes.changes.last())
        {
            self.reads.bal_slots += 1;
            return Ok(*value.value())
        }
        // A read-only slot has no value, and an omitted slot may have been cleared by a
        // deletion. The underlying DB is already at the parent's post-state, not pre-state.
        self.reads.provider_slots += 1;
        self.db.get_storage(address, index)
    }

    fn get_code_by_hash(&mut self, hash: &B256) -> Result<Bytecode, Self::Error> {
        // Changed code is supplied inline by get_account. Avoid an O(BAL size) hash scan here;
        // code referenced only by hash (including unchanged code) uses the parent's provider.
        self.reads.provider_code += 1;
        self.db.get_code_by_hash(hash)
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<B256, Self::Error> {
        self.reads.provider_hashes += 1;
        self.db.get_block_hash(number)
    }
}

/// Aggregate on the execution worker; publish once instead of resolving metric handles per read.
#[derive(Debug, Default)]
struct ParentBalReads {
    bal_accounts: u64,
    bal_slots: u64,
    provider_accounts: u64,
    provider_slots: u64,
    provider_code: u64,
    provider_hashes: u64,
}

impl Drop for ParentBalReads {
    fn drop(&mut self) {
        for (kind, value) in [
            ("bal_accounts", self.bal_accounts),
            ("bal_slots", self.bal_slots),
            ("provider_accounts", self.provider_accounts),
            ("provider_slots", self.provider_slots),
            ("provider_code", self.provider_code),
            ("provider_hashes", self.provider_hashes),
        ] {
            if value != 0 {
                reth_metrics::metrics::counter!("builder.validation.parent_bal.reads", "kind" => kind).increment(value);
            }
        }
    }
}

/// A parent without a BAL commitment, or mismatched cached bytes, uses provider reads.
pub(super) fn committed_bal(bal: Option<SharedBal>, expected: Option<B256>) -> Option<SharedBal> {
    let expected = expected?;
    bal.filter(|bal| {
        let matches = bal.hash() == expected;
        if !matches {
            tracing::warn!(target: "rpc::validation", %expected, actual = %bal.hash(), "Parent BAL commitment mismatch; using state provider");
        }
        matches
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Header, SignableTransaction, TxLegacy};
    use alloy_eip7928::bal::DecodedBal;
    use alloy_primitives::{Bytes, Signature, TxKind, U256};
    use evm2::evm::{
        AccountBal, AccountInfoBal, BalChanges, BlockAccessIndex, CacheDB, Db, EmptyDB, State,
    };
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_ethereum_primitives::{Block, BlockBody, TransactionSigned};
    use reth_evm::{cached::CachedReads, execute::Executor, ConfigureEvm};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::RecoveredBlock;
    use reth_trie_common::{
        root::{state_root_unhashed, storage_root_unhashed},
        TrieAccount,
    };

    const ADDRESS: Address = Address::repeat_byte(1);
    const OTHER: Address = Address::repeat_byte(2);

    #[derive(Default)]
    struct Reads {
        basic: usize,
        storage: usize,
        code: usize,
        hash: usize,
    }

    struct CountingDb<DB = CacheDB<EmptyDB>> {
        db: DB,
        reads: Reads,
    }

    impl<DB: Database> Database for CountingDb<DB> {
        type Error = DB::Error;

        fn is_fatal(error: &Self::Error) -> bool {
            DB::is_fatal(error)
        }

        fn get_account(&mut self, a: &Address) -> Result<Option<AccountInfo>, Self::Error> {
            self.reads.basic += 1;
            self.db.get_account(a)
        }

        fn get_storage(&mut self, a: &Address, k: &Word) -> Result<Word, Self::Error> {
            self.reads.storage += 1;
            self.db.get_storage(a, k)
        }

        fn get_code_by_hash(&mut self, h: &B256) -> Result<Bytecode, Self::Error> {
            self.reads.code += 1;
            self.db.get_code_by_hash(h)
        }

        fn get_block_hash(&mut self, n: &Word) -> Result<B256, Self::Error> {
            self.reads.hash += 1;
            self.db.get_block_hash(n)
        }
    }

    fn changes<T: BalChange>(writes: Vec<(BlockAccessIndex, T::Value)>) -> BalChanges<T> {
        BalChanges::new(writes.into_iter().map(|(index, value)| T::new(index, value)).collect())
    }

    fn fixture() -> (CountingDb, Bal) {
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(
            &ADDRESS,
            AccountInfo { balance: U256::from(100), nonce: 7, ..Default::default() },
        );
        for (slot, value) in [(1, 42), (2, 99), (3, 0)] {
            db.insert_account_storage(&ADDRESS, &U256::from(slot), &U256::from(value));
        }
        let mut account = AccountBal {
            account_info: AccountInfoBal {
                balance: changes(vec![
                    (BlockAccessIndex::new(0), U256::from(80)),
                    (BlockAccessIndex::new(4), U256::from(100)),
                ]),
                ..Default::default()
            },
            ..Default::default()
        };
        account.storage.storage.insert(
            U256::from(1),
            changes(vec![
                (BlockAccessIndex::new(1), U256::from(20)),
                (BlockAccessIndex::new(4), U256::from(42)),
            ]),
        );
        account.storage.storage.insert(U256::from(2), BalChanges::default());
        account
            .storage
            .storage
            .insert(U256::from(3), changes(vec![(BlockAccessIndex::new(4), U256::ZERO)]));
        let mut bal = Bal::default();
        bal.accounts.insert(ADDRESS, account);
        (CountingDb { db, reads: Reads::default() }, bal)
    }

    #[test]
    fn final_writes_partial_accounts_and_provider_fallback() {
        let (mut db, bal) = fixture();
        let mut db_bal = ParentBalDb::new(&mut db, Some(&bal));
        let info = db_bal.get_account(&ADDRESS).unwrap().unwrap();
        assert_eq!(info.balance, U256::from(100));
        assert_eq!(info.nonce, 7); // not in BAL: retain complete provider metadata
        assert_eq!(db_bal.db.reads.basic, 1);
        assert_eq!(db_bal.get_storage(&ADDRESS, &U256::from(1)).unwrap(), U256::from(42));
        assert_eq!(db_bal.get_storage(&ADDRESS, &U256::from(3)).unwrap(), U256::ZERO);
        assert_eq!(db_bal.db.reads.storage, 0);
        assert_eq!(db_bal.get_storage(&ADDRESS, &U256::from(2)).unwrap(), U256::from(99));
        assert_eq!(db_bal.get_storage(&ADDRESS, &U256::from(4)).unwrap(), U256::ZERO);
        assert_eq!(db_bal.db.reads.storage, 2);
        assert_eq!(db_bal.get_account(&OTHER).unwrap(), None);
        db_bal.get_code_by_hash(&B256::ZERO).unwrap();
        db_bal.get_block_hash(&U256::from(10)).unwrap();
        assert_eq!(db_bal.db.reads.code, 1);
        assert_eq!(db_bal.db.reads.hash, 1);
    }

    #[test]
    fn missing_bal_preserves_provider_reads() {
        let (mut db, _) = fixture();
        let expected = db.db.get_account(&ADDRESS).unwrap();
        let mut db_bal = ParentBalDb::new(&mut db, None);
        assert_eq!(db_bal.get_account(&ADDRESS).unwrap(), expected);
        assert_eq!(db_bal.get_storage(&ADDRESS, &U256::from(1)).unwrap(), U256::from(42));
        assert_eq!(db_bal.get_storage(&ADDRESS, &U256::from(2)).unwrap(), U256::from(99));
        assert_eq!(db_bal.db.reads.storage, 2);
    }

    #[test]
    fn cached_reads_reuse_complete_parent_values() {
        let (mut db, bal) = fixture();
        let mut cache = CachedReads::default();
        {
            let mut cached = cache.as_db_mut(ParentBalDb::new(&mut db, Some(&bal)));
            // Slot-first reads must still load complete metadata for a partial BAL account.
            assert_eq!(cached.get_storage(&ADDRESS, &U256::from(1)).unwrap(), U256::from(42));
            assert_eq!(cached.get_account(&ADDRESS).unwrap().unwrap().nonce, 7);
        }
        let mut cached = cache.as_db_mut(ParentBalDb::new(&mut db, None));
        assert_eq!(cached.get_storage(&ADDRESS, &U256::from(1)).unwrap(), U256::from(42));
        assert_eq!(cached.get_account(&ADDRESS).unwrap().unwrap().balance, U256::from(100));
        drop(cached);
        assert_eq!(db.reads.basic, 1);
        assert_eq!(db.reads.storage, 0);
    }

    #[test]
    fn complete_account_and_changed_code_avoid_provider_reads() {
        let (mut db, mut bal) = fixture();
        let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0, 0x00]));
        let account = bal.accounts.get_mut(&ADDRESS).unwrap();
        account.account_info.nonce = changes(vec![(BlockAccessIndex::new(4), 7)]);
        account.account_info.code =
            changes(vec![(BlockAccessIndex::new(4), (code.hash_slow(), code.clone()))]);
        let info = ParentBalDb::new(&mut db, Some(&bal)).get_account(&ADDRESS).unwrap().unwrap();
        assert_eq!(info.code, Some(code));
        assert_eq!(info.nonce, 7);
        #[cfg(not(feature = "account-ext"))]
        assert_eq!(db.reads.basic, 0);
    }

    #[test]
    fn partial_code_change_preserves_nonce_balance_and_existence() {
        let (mut db, mut bal) = fixture();
        let code = Bytecode::new_raw(Bytes::from_static(&[0x00]));
        let account = bal.accounts.get_mut(&ADDRESS).unwrap();
        account.account_info.balance = Default::default();
        account.account_info.code =
            changes(vec![(BlockAccessIndex::new(4), (code.hash_slow(), code.clone()))]);
        let info = ParentBalDb::new(&mut db, Some(&bal)).get_account(&ADDRESS).unwrap().unwrap();
        assert_eq!(info.code, Some(code));
        assert_eq!(info.nonce, 7);
        assert_eq!(info.balance, U256::from(100));
        assert_eq!(db.reads.basic, 1);
    }

    #[test]
    fn deletion_and_storage_wide_clears_use_parent_post_state() {
        let (mut db, mut bal) = fixture();
        // The BAL has no existence bit. All-zero final fields must not turn None into Some.
        db.db.cache.accounts.remove(&ADDRESS);
        db.db.cache.storage.remove(&ADDRESS);
        let account = bal.accounts.get_mut(&ADDRESS).unwrap();
        account.account_info.balance = changes(vec![(BlockAccessIndex::new(4), U256::ZERO)]);
        account.account_info.nonce = changes(vec![(BlockAccessIndex::new(4), 0)]);
        account.account_info.code = changes(vec![(
            BlockAccessIndex::new(4),
            (alloy_consensus::constants::KECCAK_EMPTY, Bytecode::default()),
        )]);
        account
            .storage
            .storage
            .get_mut(&U256::from(1))
            .unwrap()
            .force_update(BlockAccessIndex::new(4), U256::ZERO);
        let mut db_bal = ParentBalDb::new(&mut db, Some(&bal));
        assert_eq!(db_bal.get_account(&ADDRESS).unwrap(), None);
        assert_eq!(db_bal.get_storage(&ADDRESS, &U256::from(1)).unwrap(), U256::ZERO);
        // Omitted/read-only slots may have been wiped. Parent post-state, not BAL defaults.
        assert_eq!(db_bal.get_storage(&ADDRESS, &U256::from(2)).unwrap(), U256::ZERO);
        assert_eq!(db_bal.get_storage(&ADDRESS, &U256::from(4)).unwrap(), U256::ZERO);
    }

    #[test]
    fn child_writes_and_child_storage_clears_win_over_parent_bal() {
        let (mut db, bal) = fixture();
        let mut cache = CachedReads::default();
        {
            let mut state =
                State::new(Db::new(cache.as_db_mut(ParentBalDb::new(&mut db, Some(&bal)))));
            assert_eq!(
                state.storage_slot(&ADDRESS, U256::from(1)).unwrap().current(),
                U256::from(42)
            );
            let child_code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x01, 0x00]));
            {
                let mut account = state.account(&ADDRESS).unwrap();
                account.set_balance(U256::from(200));
                account.set_nonce(8);
                account.set_code_slow(child_code.clone());
            }
            state.storage_slot(&ADDRESS, U256::from(1)).unwrap().set(U256::from(88));
            state.commit_transaction();
            let info = state.account_info_untracked(&ADDRESS).unwrap().unwrap();
            assert_eq!(info.balance, U256::from(200));
            assert_eq!(info.nonce, 8);
            assert_eq!(info.code_hash, child_code.hash_slow());
            assert_eq!(state.account(&ADDRESS).unwrap().load_code().unwrap(), child_code);
            assert_eq!(
                state.storage_slot(&ADDRESS, U256::from(1)).unwrap().current(),
                U256::from(88)
            );
            state.storage(&ADDRESS).unwrap().wipe();
            state.commit_transaction();
            assert_eq!(state.storage_slot(&ADDRESS, U256::from(1)).unwrap().current(), U256::ZERO);
        }
        // CachedReads stores only parent values, never committed child modifications.
        let mut next = cache.as_db_mut(ParentBalDb::new(&mut db, Some(&bal)));
        assert_eq!(next.get_account(&ADDRESS).unwrap().unwrap().balance, U256::from(100));
        assert_eq!(next.get_storage(&ADDRESS, &U256::from(1)).unwrap(), U256::from(42));
    }

    #[test]
    fn ethereum_execution_receipts_bal_and_state_roots_match() {
        let code = Bytecode::new_raw(Bytes::from_static(&[
            0x60, 0x01, 0x54, 0x50, // SLOAD(1), POP
            0x60, 0x58, 0x60, 0x01, 0x55, 0x00, // SSTORE(1,88), STOP
        ]));
        let evm = EthEvmConfig::new(Arc::new(
            ChainSpecBuilder::from(&*MAINNET).amsterdam_activated().build(),
        ));
        let transactions = (0..2)
            .map(|nonce| {
                TransactionSigned::from(
                    TxLegacy {
                        chain_id: Some(1),
                        nonce,
                        gas_price: 0,
                        gas_limit: 500_000,
                        to: TxKind::Call(ADDRESS),
                        value: U256::ZERO,
                        input: Bytes::new(),
                    }
                    .into_signed(Signature::new(
                        U256::from(1),
                        U256::from(1),
                        false,
                    )),
                )
            })
            .collect();
        let block = RecoveredBlock::new_unhashed(
            Block {
                header: Header {
                    number: 1,
                    timestamp: 1,
                    gas_limit: 2_000_000,
                    base_fee_per_gas: Some(0),
                    excess_blob_gas: Some(0),
                    parent_beacon_block_root: Some(B256::ZERO),
                    block_access_list_hash: Some(B256::ZERO),
                    ..Default::default()
                },
                body: BlockBody {
                    transactions,
                    withdrawals: Some(Default::default()),
                    ..Default::default()
                },
            },
            vec![OTHER; 2],
        );

        // partial/code writes, unavailable BAL, explicit storage clear, complete created
        // account metadata, and deletion all execute against exactly the same parent state.
        for case in ["partial", "missing", "cleared", "created", "deleted"] {
            let mut outputs = Vec::new();
            for enabled in [false, true] {
                let (mut db, mut bal) = fixture();
                db.db.insert_account_info(
                    &OTHER,
                    AccountInfo { balance: U256::from(1_000_000), ..Default::default() },
                );
                let mut contract = db.db.get_account(&ADDRESS).unwrap().unwrap();
                contract.code_hash = code.hash_slow();
                contract.code = Some(code.clone());
                db.db.insert_account_info(&ADDRESS, contract);
                bal.accounts.get_mut(&ADDRESS).unwrap().account_info.code =
                    changes(vec![(BlockAccessIndex::new(4), (code.hash_slow(), code.clone()))]);
                if matches!(case, "cleared" | "deleted") {
                    db.db.insert_account_storage(&ADDRESS, &U256::from(1), &U256::ZERO);
                    bal.accounts
                        .get_mut(&ADDRESS)
                        .unwrap()
                        .storage
                        .storage
                        .get_mut(&U256::from(1))
                        .unwrap()
                        .force_update(BlockAccessIndex::new(4), U256::ZERO);
                }
                if case == "created" {
                    bal.accounts.get_mut(&ADDRESS).unwrap().account_info.nonce =
                        changes(vec![(BlockAccessIndex::new(4), 7)]);
                }
                if case == "deleted" {
                    db.db.cache.accounts.remove(&ADDRESS);
                    db.db.cache.storage.remove(&ADDRESS);
                    let account = bal.accounts.get_mut(&ADDRESS).unwrap();
                    account.account_info.balance =
                        changes(vec![(BlockAccessIndex::new(4), U256::ZERO)]);
                    account.account_info.nonce = changes(vec![(BlockAccessIndex::new(4), 0)]);
                    account.account_info.code = changes(vec![(
                        BlockAccessIndex::new(4),
                        (alloy_consensus::constants::KECCAK_EMPTY, Bytecode::default()),
                    )]);
                }
                let mut cache = CachedReads::default();
                let parent_bal = (enabled && case != "missing").then_some(&bal);
                let mut executor =
                    evm.batch_executor(cache.as_db_mut(ParentBalDb::new(&mut db, parent_bal)));
                let result = executor.execute_one(&block).unwrap();
                assert!(
                    result.receipts.iter().all(|receipt| receipt.success),
                    "case {case}, enabled {enabled}, receipts {:?}",
                    result.receipts
                );
                let rebuilt_bal = executor.take_bal().unwrap();
                let bundle = executor.into_state();
                if case != "deleted" {
                    assert_eq!(
                        bundle.state[&ADDRESS].storage[&U256::from(1)].present_value,
                        U256::from(88)
                    );
                }

                // Compute a real MPT root from the complete post-state, including untouched slots.
                let mut addresses: Vec<_> = db
                    .db
                    .cache
                    .accounts
                    .keys()
                    .copied()
                    .chain(bundle.state.keys().copied())
                    .collect();
                addresses.sort_unstable();
                addresses.dedup();
                let accounts = addresses.into_iter().filter_map(|address| {
                    let changed = bundle.state.get(&address);
                    let info = changed
                        .map(|account| account.info.clone())
                        .unwrap_or_else(|| db.db.get_account(&address).unwrap())?;
                    if info.is_empty() {
                        return None
                    }
                    let mut slots = db
                        .db
                        .cache
                        .storage
                        .get(&address)
                        .map(|a| a.slots.clone())
                        .unwrap_or_default();
                    if let Some(account) = changed {
                        if account.was_destroyed() {
                            slots.clear();
                        }
                        for (key, value) in &account.storage {
                            slots.insert(*key, value.present_value);
                        }
                    }
                    let storage_root = storage_root_unhashed(
                        slots
                            .into_iter()
                            .filter(|(_, value)| !value.is_zero())
                            .map(|(key, value)| (B256::from(key), value)),
                    );
                    Some((
                        address,
                        TrieAccount::new(info.nonce, info.balance, storage_root, info.code_hash),
                    ))
                });
                let root = state_root_unhashed(accounts);
                outputs.push((result, rebuilt_bal, bundle, root, db.reads.storage));
            }
            assert_eq!(outputs[0].0, outputs[1].0);
            assert_eq!(outputs[0].1, outputs[1].1);
            assert_eq!(outputs[0].2, outputs[1].2);
            assert_eq!(outputs[0].3, outputs[1].3);
            if case != "missing" && case != "deleted" {
                assert!(outputs[1].4 < outputs[0].4, "BAL should reduce provider storage reads");
            } else {
                assert_eq!(outputs[1].4, outputs[0].4);
            }
        }
    }
    #[test]
    fn every_partial_account_field_combination_preserves_provider_metadata() {
        let code = Bytecode::new_raw(Bytes::from_static(&[0x00]));
        for fields in 0..8 {
            let (mut db, mut bal) = fixture();
            let provider_info = AccountInfo {
                balance: U256::from(100),
                nonce: 7,
                code_hash: code.hash_slow(),
                code: None,
                ..Default::default()
            };
            db.db.insert_account_info(&ADDRESS, provider_info.clone());
            let account = bal.accounts.get_mut(&ADDRESS).unwrap();
            account.account_info.balance = if fields & 1 != 0 {
                changes(vec![(BlockAccessIndex::new(4), U256::from(100))])
            } else {
                Default::default()
            };
            account.account_info.nonce = if fields & 2 != 0 {
                changes(vec![(BlockAccessIndex::new(4), 7)])
            } else {
                Default::default()
            };
            account.account_info.code = if fields & 4 != 0 {
                changes(vec![(BlockAccessIndex::new(4), (code.hash_slow(), code.clone()))])
            } else {
                Default::default()
            };
            let info =
                ParentBalDb::new(&mut db, Some(&bal)).get_account(&ADDRESS).unwrap().unwrap();
            assert_eq!(info, provider_info, "field mask {fields}");
            if fields & 4 != 0 {
                assert_eq!(info.code, Some(code.clone()));
            }
            #[cfg(not(feature = "account-ext"))]
            assert_eq!(db.reads.basic, usize::from(fields != 7));
        }
    }

    #[test]
    fn commitment_guard_preserves_fallback_and_shares_payload() {
        let (_, bal) = fixture();
        let raw: Bytes = alloy_rlp::encode(bal.to_alloy_bal()).into();
        let hash = alloy_primitives::keccak256(&raw);
        let bal = Arc::new(DecodedBal::new_unchecked(Arc::new(bal), raw, hash));
        assert!(committed_bal(Some(bal.clone()), None).is_none());
        assert!(committed_bal(Some(bal.clone()), Some(B256::ZERO)).is_none());
        let verified = committed_bal(Some(bal.clone()), Some(hash)).unwrap();
        assert!(Arc::ptr_eq(&bal, &verified));
        let weak = Arc::downgrade(&bal);
        drop(verified);
        drop(bal);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn fallback_provider_errors_are_not_hidden() {
        struct FailingDb;
        impl Database for FailingDb {
            type Error = reth_errors::ProviderError;

            fn is_fatal(_: &Self::Error) -> bool {
                false
            }

            fn get_account(&mut self, _: &Address) -> Result<Option<AccountInfo>, Self::Error> {
                Err(reth_errors::ProviderError::other(std::io::Error::other("account read failed")))
            }
            fn get_storage(&mut self, _: &Address, _: &Word) -> Result<Word, Self::Error> {
                Err(reth_errors::ProviderError::other(std::io::Error::other("storage read failed")))
            }
            fn get_code_by_hash(&mut self, _: &B256) -> Result<Bytecode, Self::Error> {
                Err(reth_errors::ProviderError::other(std::io::Error::other("code read failed")))
            }
            fn get_block_hash(&mut self, _: &Word) -> Result<B256, Self::Error> {
                Err(reth_errors::ProviderError::other(std::io::Error::other("hash read failed")))
            }
        }
        let (_, bal) = fixture();
        let error = reth_errors::ProviderError::other(std::io::Error::other("test"));
        assert!(!ParentBalDb::<FailingDb>::is_fatal(&error));
        let mut db = CountingDb { db: FailingDb, reads: Reads::default() };
        let mut db_bal = ParentBalDb::new(&mut db, Some(&bal));
        assert_eq!(db_bal.get_storage(&ADDRESS, &U256::from(1)).unwrap(), U256::from(42));
        assert_eq!(
            db_bal.get_storage(&ADDRESS, &U256::from(2)).unwrap_err().to_string(),
            "storage read failed"
        );
        assert_eq!(db_bal.get_account(&ADDRESS).unwrap_err().to_string(), "account read failed");
        assert_eq!(
            db_bal.get_code_by_hash(&B256::ZERO).unwrap_err().to_string(),
            "code read failed"
        );
        assert_eq!(
            db_bal.get_block_hash(&U256::from(1)).unwrap_err().to_string(),
            "hash read failed"
        );
    }
}
