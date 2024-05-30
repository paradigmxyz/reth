use crate::{providers::StaticFileProviderRWRefMut, StateChanges, StateReverts, StateWriter};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
};
pub use reth_execution_types::*;
use reth_primitives::StaticFileSegment;
use reth_storage_errors::provider::{ProviderError, ProviderResult};
pub use revm::db::states::OriginalValuesKnown;

impl StateWriter for BundleStateWithReceipts {
    fn write_to_storage<TX>(
        self,
        tx: &TX,
        mut static_file_producer: Option<StaticFileProviderRWRefMut<'_>>,
        is_value_known: OriginalValuesKnown,
    ) -> ProviderResult<()>
    where
        TX: DbTxMut + DbTx,
    {
        let (plain_state, reverts) = self.bundle.into_plain_state_and_reverts(is_value_known);

        StateReverts(reverts).write_to_db(tx, self.first_block)?;

        // write receipts
        let mut bodies_cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
        let mut receipts_cursor = tx.cursor_write::<tables::Receipts>()?;

        // ATTENTION: Any potential future refactor or change to how this loop works should keep in
        // mind that the static file producer must always call `increment_block` even if the block
        // has no receipts. Keeping track of the exact block range of the segment is needed for
        // consistency, querying and file range segmentation.
        let blocks = self.receipts.into_iter().enumerate();
        for (idx, receipts) in blocks {
            let block_number = self.first_block + idx as u64;
            let first_tx_index = bodies_cursor
                .seek_exact(block_number)?
                .map(|(_, indices)| indices.first_tx_num())
                .ok_or_else(|| ProviderError::BlockBodyIndicesNotFound(block_number))?;

            if let Some(static_file_producer) = &mut static_file_producer {
                // Increment block on static file header.
                static_file_producer.increment_block(StaticFileSegment::Receipts, block_number)?;

                for (tx_idx, receipt) in receipts.into_iter().enumerate() {
                    let receipt = receipt
                        .expect("receipt should not be filtered when saving to static files.");
                    static_file_producer.append_receipt(first_tx_index + tx_idx as u64, receipt)?;
                }
            } else if !receipts.is_empty() {
                for (tx_idx, receipt) in receipts.into_iter().enumerate() {
                    if let Some(receipt) = receipt {
                        receipts_cursor.append(first_tx_index + tx_idx as u64, receipt)?;
                    }
                }
            }
        }

        StateChanges(plain_state).write_to_db(tx)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::create_test_provider_factory, AccountReader};
    use reth_db::{
        cursor::DbDupCursorRO,
        database::Database,
        models::{AccountBeforeTx, BlockNumberAddress},
        test_utils::create_test_rw_db,
    };
    use reth_primitives::{
        keccak256,
        revm::compat::{into_reth_acc, into_revm_acc},
        Account, Address, Receipt, Receipts, StorageEntry, B256, U256,
    };
    use reth_trie::{test_utils::state_root, StateRoot};
    use revm::{
        db::{
            states::{
                bundle_state::BundleRetention, changes::PlainStorageRevert, PlainStorageChangeset,
            },
            BundleState, EmptyDB,
        },
        primitives::{
            Account as RevmAccount, AccountInfo as RevmAccountInfo, AccountStatus, EvmStorageSlot,
        },
        DatabaseCommit, State,
    };
    use std::collections::{BTreeMap, HashMap};

    #[test]
    fn write_to_db_account_info() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let address_a = Address::ZERO;
        let address_b = Address::repeat_byte(0xff);

        let account_a = RevmAccountInfo { balance: U256::from(1), nonce: 1, ..Default::default() };
        let account_b = RevmAccountInfo { balance: U256::from(2), nonce: 2, ..Default::default() };
        let account_b_changed =
            RevmAccountInfo { balance: U256::from(3), nonce: 3, ..Default::default() };

        let mut state = State::builder().with_bundle_update().build();
        state.insert_not_existing(address_a);
        state.insert_account(address_b, account_b.clone());

        // 0x00.. is created
        state.commit(HashMap::from([(
            address_a,
            RevmAccount {
                info: account_a.clone(),
                status: AccountStatus::Touched | AccountStatus::Created,
                storage: HashMap::default(),
            },
        )]));

        // 0xff.. is changed (balance + 1, nonce + 1)
        state.commit(HashMap::from([(
            address_b,
            RevmAccount {
                info: account_b_changed.clone(),
                status: AccountStatus::Touched,
                storage: HashMap::default(),
            },
        )]));

        state.merge_transitions(BundleRetention::Reverts);
        let mut revm_bundle_state = state.take_bundle();

        // Write plain state and reverts separately.
        let reverts = revm_bundle_state.take_all_reverts().into_plain_state_reverts();
        let plain_state = revm_bundle_state.into_plain_state(OriginalValuesKnown::Yes);
        assert!(plain_state.storage.is_empty());
        assert!(plain_state.contracts.is_empty());
        StateChanges(plain_state)
            .write_to_db(provider.tx_ref())
            .expect("Could not write plain state to DB");

        assert_eq!(reverts.storage, [[]]);
        StateReverts(reverts)
            .write_to_db(provider.tx_ref(), 1)
            .expect("Could not write reverts to DB");

        let reth_account_a = into_reth_acc(account_a);
        let reth_account_b = into_reth_acc(account_b);
        let reth_account_b_changed = into_reth_acc(account_b_changed.clone());

        // Check plain state
        assert_eq!(
            provider.basic_account(address_a).expect("Could not read account state"),
            Some(reth_account_a),
            "Account A state is wrong"
        );
        assert_eq!(
            provider.basic_account(address_b).expect("Could not read account state"),
            Some(reth_account_b_changed),
            "Account B state is wrong"
        );

        // Check change set
        let mut changeset_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::AccountChangeSets>()
            .expect("Could not open changeset cursor");
        assert_eq!(
            changeset_cursor.seek_exact(1).expect("Could not read account change set"),
            Some((1, AccountBeforeTx { address: address_a, info: None })),
            "Account A changeset is wrong"
        );
        assert_eq!(
            changeset_cursor.next_dup().expect("Changeset table is malformed"),
            Some((1, AccountBeforeTx { address: address_b, info: Some(reth_account_b) })),
            "Account B changeset is wrong"
        );

        let mut state = State::builder().with_bundle_update().build();
        state.insert_account(address_b, account_b_changed.clone());

        // 0xff.. is destroyed
        state.commit(HashMap::from([(
            address_b,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account_b_changed,
                storage: HashMap::default(),
            },
        )]));

        state.merge_transitions(BundleRetention::Reverts);
        let mut revm_bundle_state = state.take_bundle();

        // Write plain state and reverts separately.
        let reverts = revm_bundle_state.take_all_reverts().into_plain_state_reverts();
        let plain_state = revm_bundle_state.into_plain_state(OriginalValuesKnown::Yes);
        // Account B selfdestructed so flag for it should be present.
        assert_eq!(
            plain_state.storage,
            [PlainStorageChangeset { address: address_b, wipe_storage: true, storage: vec![] }]
        );
        assert!(plain_state.contracts.is_empty());
        StateChanges(plain_state)
            .write_to_db(provider.tx_ref())
            .expect("Could not write plain state to DB");

        assert_eq!(
            reverts.storage,
            [[PlainStorageRevert { address: address_b, wiped: true, storage_revert: vec![] }]]
        );
        StateReverts(reverts)
            .write_to_db(provider.tx_ref(), 2)
            .expect("Could not write reverts to DB");

        // Check new plain state for account B
        assert_eq!(
            provider.basic_account(address_b).expect("Could not read account state"),
            None,
            "Account B should be deleted"
        );

        // Check change set
        assert_eq!(
            changeset_cursor.seek_exact(2).expect("Could not read account change set"),
            Some((2, AccountBeforeTx { address: address_b, info: Some(reth_account_b_changed) })),
            "Account B changeset is wrong after deletion"
        );
    }

    #[test]
    fn write_to_db_storage() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let address_a = Address::ZERO;
        let address_b = Address::repeat_byte(0xff);

        let account_b = RevmAccountInfo { balance: U256::from(2), nonce: 2, ..Default::default() };

        let mut state = State::builder().with_bundle_update().build();
        state.insert_not_existing(address_a);
        state.insert_account_with_storage(
            address_b,
            account_b.clone(),
            HashMap::from([(U256::from(1), U256::from(1))]),
        );

        state.commit(HashMap::from([
            (
                address_a,
                RevmAccount {
                    status: AccountStatus::Touched | AccountStatus::Created,
                    info: RevmAccountInfo::default(),
                    // 0x00 => 0 => 1
                    // 0x01 => 0 => 2
                    storage: HashMap::from([
                        (
                            U256::from(0),
                            EvmStorageSlot { present_value: U256::from(1), ..Default::default() },
                        ),
                        (
                            U256::from(1),
                            EvmStorageSlot { present_value: U256::from(2), ..Default::default() },
                        ),
                    ]),
                },
            ),
            (
                address_b,
                RevmAccount {
                    status: AccountStatus::Touched,
                    info: account_b,
                    // 0x01 => 1 => 2
                    storage: HashMap::from([(
                        U256::from(1),
                        EvmStorageSlot {
                            present_value: U256::from(2),
                            original_value: U256::from(1),
                        },
                    )]),
                },
            ),
        ]));

        state.merge_transitions(BundleRetention::Reverts);

        BundleStateWithReceipts::new(state.take_bundle(), Receipts::new(), 1)
            .write_to_storage(provider.tx_ref(), None, OriginalValuesKnown::Yes)
            .expect("Could not write bundle state to DB");

        // Check plain storage state
        let mut storage_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::PlainStorageState>()
            .expect("Could not open plain storage state cursor");

        assert_eq!(
            storage_cursor.seek_exact(address_a).unwrap(),
            Some((address_a, StorageEntry { key: B256::ZERO, value: U256::from(1) })),
            "Slot 0 for account A should be 1"
        );
        assert_eq!(
            storage_cursor.next_dup().unwrap(),
            Some((
                address_a,
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(2) }
            )),
            "Slot 1 for account A should be 2"
        );
        assert_eq!(
            storage_cursor.next_dup().unwrap(),
            None,
            "Account A should only have 2 storage slots"
        );

        assert_eq!(
            storage_cursor.seek_exact(address_b).unwrap(),
            Some((
                address_b,
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(2) }
            )),
            "Slot 1 for account B should be 2"
        );
        assert_eq!(
            storage_cursor.next_dup().unwrap(),
            None,
            "Account B should only have 1 storage slot"
        );

        // Check change set
        let mut changeset_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::StorageChangeSets>()
            .expect("Could not open storage changeset cursor");
        assert_eq!(
            changeset_cursor.seek_exact(BlockNumberAddress((1, address_a))).unwrap(),
            Some((
                BlockNumberAddress((1, address_a)),
                StorageEntry { key: B256::ZERO, value: U256::from(0) }
            )),
            "Slot 0 for account A should have changed from 0"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            Some((
                BlockNumberAddress((1, address_a)),
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(0) }
            )),
            "Slot 1 for account A should have changed from 0"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            None,
            "Account A should only be in the changeset 2 times"
        );

        assert_eq!(
            changeset_cursor.seek_exact(BlockNumberAddress((1, address_b))).unwrap(),
            Some((
                BlockNumberAddress((1, address_b)),
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(1) }
            )),
            "Slot 1 for account B should have changed from 1"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            None,
            "Account B should only be in the changeset 1 time"
        );

        // Delete account A
        let mut state = State::builder().with_bundle_update().build();
        state.insert_account(address_a, RevmAccountInfo::default());

        state.commit(HashMap::from([(
            address_a,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: RevmAccountInfo::default(),
                storage: HashMap::default(),
            },
        )]));

        state.merge_transitions(BundleRetention::Reverts);
        BundleStateWithReceipts::new(state.take_bundle(), Receipts::new(), 2)
            .write_to_storage(provider.tx_ref(), None, OriginalValuesKnown::Yes)
            .expect("Could not write bundle state to DB");

        assert_eq!(
            storage_cursor.seek_exact(address_a).unwrap(),
            None,
            "Account A should have no storage slots after deletion"
        );

        assert_eq!(
            changeset_cursor.seek_exact(BlockNumberAddress((2, address_a))).unwrap(),
            Some((
                BlockNumberAddress((2, address_a)),
                StorageEntry { key: B256::ZERO, value: U256::from(1) }
            )),
            "Slot 0 for account A should have changed from 1 on deletion"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            Some((
                BlockNumberAddress((2, address_a)),
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(2) }
            )),
            "Slot 1 for account A should have changed from 2 on deletion"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            None,
            "Account A should only be in the changeset 2 times on deletion"
        );
    }

    #[test]
    fn write_to_db_multiple_selfdestructs() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let address1 = Address::random();
        let account_info = RevmAccountInfo { nonce: 1, ..Default::default() };

        // Block #0: initial state.
        let mut init_state = State::builder().with_bundle_update().build();
        init_state.insert_not_existing(address1);
        init_state.commit(HashMap::from([(
            address1,
            RevmAccount {
                info: account_info.clone(),
                status: AccountStatus::Touched | AccountStatus::Created,
                // 0x00 => 0 => 1
                // 0x01 => 0 => 2
                storage: HashMap::from([
                    (
                        U256::ZERO,
                        EvmStorageSlot { present_value: U256::from(1), ..Default::default() },
                    ),
                    (
                        U256::from(1),
                        EvmStorageSlot { present_value: U256::from(2), ..Default::default() },
                    ),
                ]),
            },
        )]));
        init_state.merge_transitions(BundleRetention::Reverts);
        BundleStateWithReceipts::new(init_state.take_bundle(), Receipts::new(), 0)
            .write_to_storage(provider.tx_ref(), None, OriginalValuesKnown::Yes)
            .expect("Could not write init bundle state to DB");

        let mut state = State::builder().with_bundle_update().build();
        state.insert_account_with_storage(
            address1,
            account_info.clone(),
            HashMap::from([(U256::ZERO, U256::from(1)), (U256::from(1), U256::from(2))]),
        );

        // Block #1: change storage.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account_info.clone(),
                // 0x00 => 1 => 2
                storage: HashMap::from([(
                    U256::ZERO,
                    EvmStorageSlot { original_value: U256::from(1), present_value: U256::from(2) },
                )]),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #2: destroy account.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #3: re-create account and change storage.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #4: change storage.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account_info.clone(),
                // 0x00 => 0 => 2
                // 0x02 => 0 => 4
                // 0x06 => 0 => 6
                storage: HashMap::from([
                    (
                        U256::ZERO,
                        EvmStorageSlot { present_value: U256::from(2), ..Default::default() },
                    ),
                    (
                        U256::from(2),
                        EvmStorageSlot { present_value: U256::from(4), ..Default::default() },
                    ),
                    (
                        U256::from(6),
                        EvmStorageSlot { present_value: U256::from(6), ..Default::default() },
                    ),
                ]),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #5: Destroy account again.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #6: Create, change, destroy and re-create in the same block.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account_info.clone(),
                // 0x00 => 0 => 2
                storage: HashMap::from([(
                    U256::ZERO,
                    EvmStorageSlot { present_value: U256::from(2), ..Default::default() },
                )]),
            },
        )]));
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #7: Change storage.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account_info,
                // 0x00 => 0 => 9
                storage: HashMap::from([(
                    U256::ZERO,
                    EvmStorageSlot { present_value: U256::from(9), ..Default::default() },
                )]),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        let bundle = state.take_bundle();

        BundleStateWithReceipts::new(bundle, Receipts::new(), 1)
            .write_to_storage(provider.tx_ref(), None, OriginalValuesKnown::Yes)
            .expect("Could not write bundle state to DB");

        let mut storage_changeset_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::StorageChangeSets>()
            .expect("Could not open plain storage state cursor");
        let mut storage_changes = storage_changeset_cursor.walk_range(..).unwrap();

        // Iterate through all storage changes

        // Block <number>
        // <slot>: <expected value before>
        // ...

        // Block #0
        // 0x00: 0
        // 0x01: 0
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((0, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::ZERO }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((0, address1)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::ZERO }
            )))
        );

        // Block #1
        // 0x00: 1
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((1, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::from(1) }
            )))
        );

        // Block #2 (destroyed)
        // 0x00: 2
        // 0x01: 2
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((2, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::from(2) }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((2, address1)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::from(2) }
            )))
        );

        // Block #3
        // no storage changes

        // Block #4
        // 0x00: 0
        // 0x02: 0
        // 0x06: 0
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((4, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::ZERO }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((4, address1)),
                StorageEntry { key: B256::with_last_byte(2), value: U256::ZERO }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((4, address1)),
                StorageEntry { key: B256::with_last_byte(6), value: U256::ZERO }
            )))
        );

        // Block #5 (destroyed)
        // 0x00: 2
        // 0x02: 4
        // 0x06: 6
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((5, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::from(2) }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((5, address1)),
                StorageEntry { key: B256::with_last_byte(2), value: U256::from(4) }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((5, address1)),
                StorageEntry { key: B256::with_last_byte(6), value: U256::from(6) }
            )))
        );

        // Block #6
        // no storage changes (only inter block changes)

        // Block #7
        // 0x00: 0
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((7, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::ZERO }
            )))
        );
        assert_eq!(storage_changes.next(), None);
    }

    #[test]
    fn storage_change_after_selfdestruct_within_block() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let address1 = Address::random();
        let account1 = RevmAccountInfo { nonce: 1, ..Default::default() };

        // Block #0: initial state.
        let mut init_state = State::builder().with_bundle_update().build();
        init_state.insert_not_existing(address1);
        init_state.commit(HashMap::from([(
            address1,
            RevmAccount {
                info: account1.clone(),
                status: AccountStatus::Touched | AccountStatus::Created,
                // 0x00 => 0 => 1
                // 0x01 => 0 => 2
                storage: HashMap::from([
                    (
                        U256::ZERO,
                        EvmStorageSlot { present_value: U256::from(1), ..Default::default() },
                    ),
                    (
                        U256::from(1),
                        EvmStorageSlot { present_value: U256::from(2), ..Default::default() },
                    ),
                ]),
            },
        )]));
        init_state.merge_transitions(BundleRetention::Reverts);
        BundleStateWithReceipts::new(init_state.take_bundle(), Receipts::new(), 0)
            .write_to_storage(provider.tx_ref(), None, OriginalValuesKnown::Yes)
            .expect("Could not write init bundle state to DB");

        let mut state = State::builder().with_bundle_update().build();
        state.insert_account_with_storage(
            address1,
            account1.clone(),
            HashMap::from([(U256::ZERO, U256::from(1)), (U256::from(1), U256::from(2))]),
        );

        // Block #1: Destroy, re-create, change storage.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account1.clone(),
                storage: HashMap::default(),
            },
        )]));

        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account1.clone(),
                storage: HashMap::default(),
            },
        )]));

        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account1,
                // 0x01 => 0 => 5
                storage: HashMap::from([(
                    U256::from(1),
                    EvmStorageSlot { present_value: U256::from(5), ..Default::default() },
                )]),
            },
        )]));

        // Commit block #1 changes to the database.
        state.merge_transitions(BundleRetention::Reverts);
        BundleStateWithReceipts::new(state.take_bundle(), Receipts::new(), 1)
            .write_to_storage(provider.tx_ref(), None, OriginalValuesKnown::Yes)
            .expect("Could not write bundle state to DB");

        let mut storage_changeset_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::StorageChangeSets>()
            .expect("Could not open plain storage state cursor");
        let range = BlockNumberAddress::range(1..=1);
        let mut storage_changes = storage_changeset_cursor.walk_range(range).unwrap();

        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((1, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::from(1) }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((1, address1)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::from(2) }
            )))
        );
        assert_eq!(storage_changes.next(), None);
    }

    #[test]
    fn revert_to_indices() {
        let base = BundleStateWithReceipts {
            bundle: BundleState::default(),
            receipts: Receipts::from_vec(vec![vec![Some(Receipt::default()); 2]; 7]),
            first_block: 10,
        };

        let mut this = base.clone();
        assert!(this.revert_to(10));
        assert_eq!(this.receipts.len(), 1);

        let mut this = base.clone();
        assert!(!this.revert_to(9));
        assert_eq!(this.receipts.len(), 7);

        let mut this = base.clone();
        assert!(this.revert_to(15));
        assert_eq!(this.receipts.len(), 6);

        let mut this = base.clone();
        assert!(this.revert_to(16));
        assert_eq!(this.receipts.len(), 7);

        let mut this = base;
        assert!(!this.revert_to(17));
        assert_eq!(this.receipts.len(), 7);
    }

    #[test]
    fn bundle_state_state_root() {
        type PreState = BTreeMap<Address, (Account, BTreeMap<B256, U256>)>;
        let mut prestate: PreState = (0..10)
            .map(|key| {
                let account = Account { nonce: 1, balance: U256::from(key), bytecode_hash: None };
                let storage =
                    (1..11).map(|key| (B256::with_last_byte(key), U256::from(key))).collect();
                (Address::with_last_byte(key), (account, storage))
            })
            .collect();

        let db = create_test_rw_db();

        // insert initial state to the database
        db.update(|tx| {
            for (address, (account, storage)) in prestate.iter() {
                let hashed_address = keccak256(address);
                tx.put::<tables::HashedAccounts>(hashed_address, *account).unwrap();
                for (slot, value) in storage {
                    tx.put::<tables::HashedStorages>(
                        hashed_address,
                        StorageEntry { key: keccak256(slot), value: *value },
                    )
                    .unwrap();
                }
            }

            let (_, updates) = StateRoot::from_tx(tx).root_with_updates().unwrap();
            updates.flush(tx).unwrap();
        })
        .unwrap();

        let tx = db.tx().unwrap();
        let mut state = State::builder().with_bundle_update().build();

        let assert_state_root = |state: &State<EmptyDB>, expected: &PreState, msg| {
            assert_eq!(
                BundleStateWithReceipts::new(state.bundle_state.clone(), Receipts::default(), 0)
                    .hash_state_slow()
                    .state_root(&tx)
                    .unwrap(),
                state_root(expected.clone().into_iter().map(|(address, (account, storage))| (
                    address,
                    (account, storage.into_iter())
                ))),
                "{msg}"
            );
        };

        // database only state root is correct
        assert_state_root(&state, &prestate, "empty");

        // destroy account 1
        let address1 = Address::with_last_byte(1);
        let account1_old = prestate.remove(&address1).unwrap();
        state.insert_account(address1, into_revm_acc(account1_old.0));
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: RevmAccountInfo::default(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::PlainState);
        assert_state_root(&state, &prestate, "destroyed account");

        // change slot 2 in account 2
        let address2 = Address::with_last_byte(2);
        let slot2 = U256::from(2);
        let slot2_key = B256::from(slot2);
        let account2 = prestate.get_mut(&address2).unwrap();
        let account2_slot2_old_value = *account2.1.get(&slot2_key).unwrap();
        state.insert_account_with_storage(
            address2,
            into_revm_acc(account2.0),
            HashMap::from([(slot2, account2_slot2_old_value)]),
        );

        let account2_slot2_new_value = U256::from(100);
        account2.1.insert(slot2_key, account2_slot2_new_value);
        state.commit(HashMap::from([(
            address2,
            RevmAccount {
                status: AccountStatus::Touched,
                info: into_revm_acc(account2.0),
                storage: HashMap::from_iter([(
                    slot2,
                    EvmStorageSlot::new_changed(account2_slot2_old_value, account2_slot2_new_value),
                )]),
            },
        )]));
        state.merge_transitions(BundleRetention::PlainState);
        assert_state_root(&state, &prestate, "changed storage");

        // change balance of account 3
        let address3 = Address::with_last_byte(3);
        let account3 = prestate.get_mut(&address3).unwrap();
        state.insert_account(address3, into_revm_acc(account3.0));

        account3.0.balance = U256::from(24);
        state.commit(HashMap::from([(
            address3,
            RevmAccount {
                status: AccountStatus::Touched,
                info: into_revm_acc(account3.0),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::PlainState);
        assert_state_root(&state, &prestate, "changed balance");

        // change nonce of account 4
        let address4 = Address::with_last_byte(4);
        let account4 = prestate.get_mut(&address4).unwrap();
        state.insert_account(address4, into_revm_acc(account4.0));

        account4.0.nonce = 128;
        state.commit(HashMap::from([(
            address4,
            RevmAccount {
                status: AccountStatus::Touched,
                info: into_revm_acc(account4.0),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::PlainState);
        assert_state_root(&state, &prestate, "changed nonce");

        // recreate account 1
        let account1_new =
            Account { nonce: 56, balance: U256::from(123), bytecode_hash: Some(B256::random()) };
        prestate.insert(address1, (account1_new, BTreeMap::default()));
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: into_revm_acc(account1_new),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::PlainState);
        assert_state_root(&state, &prestate, "recreated");

        // update storage for account 1
        let slot20 = U256::from(20);
        let slot20_key = B256::from(slot20);
        let account1_slot20_value = U256::from(12345);
        prestate.get_mut(&address1).unwrap().1.insert(slot20_key, account1_slot20_value);
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: into_revm_acc(account1_new),
                storage: HashMap::from_iter([(
                    slot20,
                    EvmStorageSlot::new_changed(U256::ZERO, account1_slot20_value),
                )]),
            },
        )]));
        state.merge_transitions(BundleRetention::PlainState);
        assert_state_root(&state, &prestate, "recreated changed storage");
    }

    #[test]
    fn prepend_state() {
        let address1 = Address::random();
        let address2 = Address::random();

        let account1 = RevmAccountInfo { nonce: 1, ..Default::default() };
        let account1_changed = RevmAccountInfo { nonce: 1, ..Default::default() };
        let account2 = RevmAccountInfo { nonce: 1, ..Default::default() };

        let present_state = BundleState::builder(2..=2)
            .state_present_account_info(address1, account1_changed.clone())
            .build();
        assert_eq!(present_state.reverts.len(), 1);
        let previous_state = BundleState::builder(1..=1)
            .state_present_account_info(address1, account1)
            .state_present_account_info(address2, account2.clone())
            .build();
        assert_eq!(previous_state.reverts.len(), 1);

        let mut test = BundleStateWithReceipts {
            bundle: present_state,
            receipts: Receipts::from_vec(vec![vec![Some(Receipt::default()); 2]; 1]),
            first_block: 2,
        };

        test.prepend_state(previous_state);

        assert_eq!(test.receipts.len(), 1);
        let end_state = test.state();
        assert_eq!(end_state.state.len(), 2);
        // reverts num should stay the same.
        assert_eq!(end_state.reverts.len(), 1);
        // account1 is not overwritten.
        assert_eq!(end_state.state.get(&address1).unwrap().info, Some(account1_changed));
        // account2 got inserted
        assert_eq!(end_state.state.get(&address2).unwrap().info, Some(account2));
    }
}
