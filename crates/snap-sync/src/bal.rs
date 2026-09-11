//! Carries downloaded state forward through verified block access lists.
//!
//! [EIP-8189](https://eips.ethereum.org/EIPS/eip-8189#synchronization-algorithm) advances the
//! pivot by applying later blocks' lists to the state downloaded so far. Each list records the
//! final value of every field it changes, so untouched fields come from the downloaded account.

use crate::SnapSyncError;
use alloy_eip7928::AccountChanges;
use alloy_primitives::{keccak256, map::B256Map, Bytes, B256, KECCAK256_EMPTY};
use reth_primitives_traits::Account;
use reth_trie_common::{bal::BalAccountState, HashedPostState, HashedStorage};

/// Changes one block access list makes to the downloaded state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BalStateUpdate {
    // Post-state of every entry whose account is downloaded.
    state: HashedPostState,
    // Final code of accounts whose code changed, keyed by code hash.
    bytecodes: B256Map<Bytes>,
    // Entries left out because their account is not downloaded yet.
    unresolved: Vec<B256>,
}

impl BalStateUpdate {
    /// Applies `bal` on top of the downloaded state, resolved by hashed address through `base`.
    ///
    /// The list must already be verified against its header.
    pub fn from_block_access_list(
        bal: &[AccountChanges],
        mut base: impl FnMut(B256) -> Result<DownloadedAccount, SnapSyncError>,
    ) -> Result<Self, SnapSyncError> {
        let mut update = Self::default();
        for account_changes in bal {
            let account_fields = BalAccountState::from_changes(account_changes);
            // Read-only entries record accesses, not changes.
            if !account_fields.changes_state_root(account_changes) {
                continue
            }

            let hashed_address = keccak256(account_changes.address());
            let existing_account = match base(hashed_address)? {
                // Its range is downloaded against a later root, which includes this change.
                DownloadedAccount::Unknown => {
                    update.unresolved.push(hashed_address);
                    continue
                }
                DownloadedAccount::Absent => None,
                DownloadedAccount::Present(account) => Some(account),
            };

            let mut account = account_fields.merge_onto(existing_account.as_ref());
            // Persist accounts without code using the storage representation.
            account.bytecode_hash = account.bytecode_hash.filter(|hash| *hash != KECCAK256_EMPTY);
            // Execution removes accounts a block leaves empty, see EIP-161.
            update.state.accounts.insert(hashed_address, (!account.is_empty()).then_some(account));
            if !account_changes.storage_changes().is_empty() {
                update
                    .state
                    .storages
                    .insert(hashed_address, HashedStorage::from_account_changes(account_changes));
            }
            if let Some((code_hash, code)) =
                account_fields.code_hash().flatten().zip(account_changes.code_post_state())
            {
                update.bytecodes.insert(code_hash, code.clone());
            }
        }
        Ok(update)
    }

    /// Post-state of every entry whose account is downloaded, keyed by hashed address.
    pub const fn state(&self) -> &HashedPostState {
        &self.state
    }

    /// Final code of accounts whose code changed, keyed by code hash.
    pub const fn bytecodes(&self) -> &B256Map<Bytes> {
        &self.bytecodes
    }

    /// Hashed addresses of entries left out because their account is not downloaded yet.
    pub fn unresolved(&self) -> &[B256] {
        &self.unresolved
    }
}

/// What the downloaded state holds for an account a list changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadedAccount {
    /// The account's range is not downloaded yet.
    Unknown,
    /// The account's range is downloaded and does not hold it.
    Absent,
    /// The downloaded account.
    Present(Account),
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Header, TxLegacy};
    use alloy_eip7928::{
        BalanceChange, BlockAccessIndex, CodeChange, NonceChange, SlotChanges, StorageChange,
    };
    use alloy_eips::{
        eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE},
        eip4788::{BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE},
        eip4895::Withdrawal,
        eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE},
    };
    use alloy_primitives::{bytes, Address, Signature, TxKind, U256};
    use reth_chainspec::ChainSpecBuilder;
    use reth_ethereum_primitives::{Block, BlockBody, Transaction, TransactionSigned};
    use reth_evm::{execute::BlockExecutor, ConfigureEvm, Evm};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::{Block as _, Recovered};
    use reth_trie_common::KeccakKeyHasher;
    use revm::{
        database::{states::bundle_state::BundleRetention, CacheDB, EmptyDB, State},
        state::{AccountInfo, Bytecode},
    };
    use std::{collections::BTreeMap, sync::Arc};

    const ACCOUNT: Address = Address::repeat_byte(0xaa);
    const SENDER: Address = Address::repeat_byte(0x11);

    fn index(value: u64) -> BlockAccessIndex {
        BlockAccessIndex::new(value)
    }

    fn apply(changes: AccountChanges, base: DownloadedAccount) -> BalStateUpdate {
        BalStateUpdate::from_block_access_list(&[changes], |_| Ok(base)).unwrap()
    }

    #[test]
    fn read_only_entries_write_nothing() {
        let changes = AccountChanges::new(ACCOUNT).with_storage_read(U256::from(1));

        let update = BalStateUpdate::from_block_access_list(&[changes], |_| {
            panic!("read-only entries need no downloaded account")
        })
        .unwrap();

        assert_eq!(update, BalStateUpdate::default());
    }

    #[test]
    fn accounts_not_downloaded_yet_are_left_unresolved() {
        // Even a fully determined account is left to the download against the later root.
        let changes = AccountChanges::new(ACCOUNT)
            .with_balance_change(BalanceChange::new(index(1), U256::from(10)))
            .with_nonce_change(NonceChange::new(index(1), 1))
            .with_code_change(CodeChange::new(index(1), bytes!("6001")));

        let update = apply(changes, DownloadedAccount::Unknown);

        assert_eq!(update.unresolved, vec![keccak256(ACCOUNT)]);
        assert!(update.state.is_empty());
        assert!(update.bytecodes.is_empty());
    }

    #[test]
    fn untouched_fields_keep_their_downloaded_values() {
        let existing =
            Account { balance: U256::from(9), nonce: 4, bytecode_hash: Some(B256::repeat_byte(1)) };
        let changes = AccountChanges::new(ACCOUNT)
            .with_balance_change(BalanceChange::new(index(1), U256::from(10)))
            .with_balance_change(BalanceChange::new(index(2), U256::from(20)));

        let update = apply(changes.clone(), DownloadedAccount::Present(existing));
        assert_eq!(
            update.state.accounts[&keccak256(ACCOUNT)],
            Some(Account { balance: U256::from(20), ..existing })
        );

        // An absent account starts from the empty one.
        let update = apply(changes, DownloadedAccount::Absent);
        assert_eq!(
            update.state.accounts[&keccak256(ACCOUNT)],
            Some(Account { balance: U256::from(20), nonce: 0, bytecode_hash: None })
        );
    }

    #[test]
    fn zeroed_slots_and_cleared_code_are_written() {
        let existing =
            Account { balance: U256::from(1), nonce: 1, bytecode_hash: Some(B256::repeat_byte(1)) };
        let changes = AccountChanges::new(ACCOUNT)
            .with_code_change(CodeChange::new(index(1), bytes!("6001")))
            .with_code_change(CodeChange::new(index(2), Bytes::new()))
            .with_storage_change(SlotChanges::new(
                U256::from(1),
                vec![
                    StorageChange::new(index(1), U256::from(5)),
                    StorageChange::new(index(2), U256::ZERO),
                ],
            ));

        let update = apply(changes, DownloadedAccount::Present(existing));

        let hashed_address = keccak256(ACCOUNT);
        assert_eq!(update.state.accounts[&hashed_address].unwrap().bytecode_hash, None);
        assert_eq!(
            update.state.storages[&hashed_address],
            HashedStorage::from_iter([(keccak256(B256::from(U256::from(1))), U256::ZERO)])
        );
        assert!(update.bytecodes.is_empty());
    }

    #[test]
    fn an_account_the_block_empties_is_removed() {
        let changes = AccountChanges::new(ACCOUNT)
            .with_balance_change(BalanceChange::new(index(1), U256::from(100)))
            .with_balance_change(BalanceChange::new(index(2), U256::ZERO));

        let update = apply(changes, DownloadedAccount::Absent);

        assert_eq!(update.state.accounts[&keccak256(ACCOUNT)], None);
    }

    // Final state as a flat map: accounts, and non-zero slots by hashed address and slot.
    type FlatState = (BTreeMap<B256, Account>, BTreeMap<(B256, B256), U256>);

    fn flatten(db: &CacheDB<EmptyDB>) -> FlatState {
        let mut state = FlatState::default();
        for (address, account) in &db.cache.accounts {
            let hashed_address = keccak256(address);
            state.0.insert(hashed_address, Account::from(&account.info));
            for (slot, value) in &account.storage {
                state.1.insert((hashed_address, keccak256(B256::from(*slot))), *value);
            }
        }
        state
    }

    fn fold(mut state: FlatState, update: &HashedPostState) -> FlatState {
        for (hashed_address, account) in &update.accounts {
            match account {
                Some(account) => state.0.insert(*hashed_address, *account),
                None => state.0.remove(hashed_address),
            };
        }
        for (hashed_address, storage) in &update.storages {
            for (slot, value) in &storage.storage {
                if value.is_zero() {
                    state.1.remove(&(*hashed_address, *slot));
                } else {
                    state.1.insert((*hashed_address, *slot), *value);
                }
            }
        }
        state
    }

    fn insert(db: &mut CacheDB<EmptyDB>, address: Address, nonce: u64, code: Bytes) {
        let code = Bytecode::new_raw(code);
        let info = AccountInfo {
            nonce,
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        };
        db.insert_account_info(address, info);
    }

    fn tx(nonce: u64, to: TxKind, value: u64, input: Bytes) -> Recovered<TransactionSigned> {
        let tx = Transaction::Legacy(TxLegacy {
            nonce,
            gas_price: 1,
            gas_limit: 1_000_000,
            to,
            value: U256::from(value),
            input,
            ..Default::default()
        });
        Recovered::new_unchecked(
            TransactionSigned::new_unhashed(tx, Signature::test_signature()),
            SENDER,
        )
    }

    #[test]
    fn applying_the_list_matches_execution() {
        let contract = Address::repeat_byte(0xc0);
        let beneficiary = Address::repeat_byte(0xbe);

        let mut db = CacheDB::<EmptyDB>::new(Default::default());
        insert(&mut db, BEACON_ROOTS_ADDRESS, 1, BEACON_ROOTS_CODE.clone());
        insert(&mut db, HISTORY_STORAGE_ADDRESS, 1, HISTORY_STORAGE_CODE.clone());
        insert(
            &mut db,
            WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
            1,
            WITHDRAWAL_REQUEST_PREDEPLOY_CODE.clone(),
        );
        db.insert_account_info(SENDER, AccountInfo::from_balance(U256::from(u64::MAX)));
        // Zeroes slot 1, reads slot 3, stores the block number in slot 2 and the call value in 4.
        insert(&mut db, contract, 1, bytes!("6000600155600354504360025534600455"));
        db.insert_account_storage(contract, U256::from(1), U256::from(5)).unwrap();
        db.insert_account_storage(contract, U256::from(3), U256::from(7)).unwrap();
        let pre = flatten(&db);

        let txs = vec![
            // Repeated writes, the second of slots 1 and 2 being no-ops.
            tx(0, TxKind::Call(contract), 1, Bytes::new()),
            tx(1, TxKind::Call(contract), 2, Bytes::new()),
            // Deploys `INVALID` as runtime code.
            tx(2, TxKind::Create, 0, bytes!("60fe60005360016000f3")),
            // Funds an address, then creates and destroys a contract there in one transaction.
            tx(3, TxKind::Call(SENDER.create(4)), 100, Bytes::new()),
            tx(
                4,
                TxKind::Create,
                0,
                [&[0x73][..], beneficiary.as_slice(), &[0xff]].concat().into(),
            ),
        ];
        let header = Header {
            number: 1,
            timestamp: 1,
            gas_limit: 30_000_000,
            parent_beacon_block_root: Some(B256::repeat_byte(1)),
            excess_blob_gas: Some(0),
            blob_gas_used: Some(0),
            block_access_list_hash: Some(B256::ZERO),
            ..Default::default()
        };
        let withdrawal = Withdrawal { address: beneficiary, amount: 1, ..Default::default() };
        let block = Block {
            header,
            body: BlockBody { withdrawals: Some(vec![withdrawal].into()), ..Default::default() },
        }
        .seal_slow();

        let evm_config =
            EthEvmConfig::new(Arc::new(ChainSpecBuilder::mainnet().amsterdam_activated().build()));
        let mut state =
            State::builder().with_database(&mut db).with_bundle_update().with_bal_builder().build();
        {
            let mut executor = evm_config.executor_for_block(&mut state, &block).unwrap();
            executor.apply_pre_execution_changes().unwrap();
            for tx in txs {
                executor.evm_mut().db_mut().bump_bal_index();
                executor.execute_transaction(tx).unwrap();
            }
            executor.evm_mut().db_mut().bump_bal_index();
            executor.apply_post_execution_changes().unwrap();
        }
        let bal = state.take_built_alloy_bal().unwrap();
        state.merge_transitions(BundleRetention::PlainState);
        let bundle = state.take_bundle();

        let update = BalStateUpdate::from_block_access_list(&bal, |hashed_address| {
            Ok(pre
                .0
                .get(&hashed_address)
                .map_or(DownloadedAccount::Absent, |account| DownloadedAccount::Present(*account)))
        })
        .unwrap();
        let executed = HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle.state());

        let post = fold(pre.clone(), &update.state);
        assert!(update.unresolved.is_empty());
        assert_eq!(post, fold(pre, &executed));

        // The block did what the cases need.
        let slot = |address: Address, slot: u64| {
            post.1.get(&(keccak256(address), keccak256(B256::from(U256::from(slot)))))
        };
        assert_eq!((slot(contract, 1), slot(contract, 4)), (None, Some(&U256::from(2))));
        assert!(!post.0.contains_key(&keccak256(SENDER.create(4))));
        assert_eq!(post.0[&keccak256(beneficiary)].balance, U256::from(100 + 1_000_000_000u64));
        assert!(post.1.keys().any(|(address, _)| *address == keccak256(BEACON_ROOTS_ADDRESS)));
        assert_eq!(
            update.bytecodes,
            B256Map::from_iter([(keccak256([0xfe]), Bytes::from_static(&[0xfe]))])
        );
    }
}
