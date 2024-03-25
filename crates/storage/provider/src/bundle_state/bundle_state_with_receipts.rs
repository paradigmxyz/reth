use crate::{providers::StaticFileProviderRWRefMut, StateChanges, StateReverts};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_primitives::{
    logs_bloom,
    revm::compat::{into_reth_acc, into_revm_acc},
    Account, Address, BlockNumber, Bloom, Bytecode, Log, Receipt, Receipts, StaticFileSegment,
    StorageEntry, B256, U256,
};
use reth_trie::HashedPostState;
pub use revm::db::states::OriginalValuesKnown;
use revm::{
    db::{states::BundleState, BundleAccount},
    primitives::AccountInfo,
};
use std::collections::HashMap;

/// Bundle state of post execution changes and reverts
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct BundleStateWithReceipts {
    /// Bundle state with reverts.
    bundle: BundleState,
    /// The collection of receipts.
    /// Outer vector stores receipts for each block sequentially.
    /// The inner vector stores receipts ordered by transaction number.
    ///
    /// If receipt is None it means it is pruned.
    receipts: Receipts,
    /// First block of bundle state.
    first_block: BlockNumber,
}

/// Type used to initialize revms bundle state.
pub type BundleStateInit =
    HashMap<Address, (Option<Account>, Option<Account>, HashMap<B256, (U256, U256)>)>;

/// Types used inside RevertsInit to initialize revms reverts.
pub type AccountRevertInit = (Option<Option<Account>>, Vec<StorageEntry>);

/// Type used to initialize revms reverts.
pub type RevertsInit = HashMap<BlockNumber, HashMap<Address, AccountRevertInit>>;

impl BundleStateWithReceipts {
    /// Create Bundle State.
    pub fn new(bundle: BundleState, receipts: Receipts, first_block: BlockNumber) -> Self {
        Self { bundle, receipts, first_block }
    }

    /// Create new bundle state with receipts.
    pub fn new_init(
        state_init: BundleStateInit,
        revert_init: RevertsInit,
        contracts_init: Vec<(B256, Bytecode)>,
        receipts: Receipts,
        first_block: BlockNumber,
    ) -> Self {
        // sort reverts by block number
        let mut reverts = revert_init.into_iter().collect::<Vec<_>>();
        reverts.sort_unstable_by_key(|a| a.0);

        // initialize revm bundle
        let bundle = BundleState::new(
            state_init.into_iter().map(|(address, (original, present, storage))| {
                (
                    address,
                    original.map(into_revm_acc),
                    present.map(into_revm_acc),
                    storage.into_iter().map(|(k, s)| (k.into(), s)).collect(),
                )
            }),
            reverts.into_iter().map(|(_, reverts)| {
                // does not needs to be sorted, it is done when taking reverts.
                reverts.into_iter().map(|(address, (original, storage))| {
                    (
                        address,
                        original.map(|i| i.map(into_revm_acc)),
                        storage.into_iter().map(|entry| (entry.key.into(), entry.value)),
                    )
                })
            }),
            contracts_init.into_iter().map(|(code_hash, bytecode)| (code_hash, bytecode.0)),
        );

        Self { bundle, receipts, first_block }
    }

    /// Return revm bundle state.
    pub fn state(&self) -> &BundleState {
        &self.bundle
    }

    /// Returns mutable revm bundle state.
    pub fn state_mut(&mut self) -> &mut BundleState {
        &mut self.bundle
    }

    /// Set first block.
    pub fn set_first_block(&mut self, first_block: BlockNumber) {
        self.first_block = first_block;
    }

    /// Return iterator over all accounts
    pub fn accounts_iter(&self) -> impl Iterator<Item = (Address, Option<&AccountInfo>)> {
        self.bundle.state().iter().map(|(a, acc)| (*a, acc.info.as_ref()))
    }

    /// Return iterator over all [BundleAccount]s in the bundle
    pub fn bundle_accounts_iter(&self) -> impl Iterator<Item = (Address, &BundleAccount)> {
        self.bundle.state().iter().map(|(a, acc)| (*a, acc))
    }

    /// Get account if account is known.
    pub fn account(&self, address: &Address) -> Option<Option<Account>> {
        self.bundle.account(address).map(|a| a.info.clone().map(into_reth_acc))
    }

    /// Get storage if value is known.
    ///
    /// This means that depending on status we can potentially return U256::ZERO.
    pub fn storage(&self, address: &Address, storage_key: U256) -> Option<U256> {
        self.bundle.account(address).and_then(|a| a.storage_slot(storage_key))
    }

    /// Return bytecode if known.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Bytecode> {
        self.bundle.bytecode(code_hash).map(Bytecode)
    }

    /// Returns [HashedPostState] for this bundle state.
    /// See [HashedPostState::from_bundle_state] for more info.
    pub fn hash_state_slow(&self) -> HashedPostState {
        HashedPostState::from_bundle_state(&self.bundle.state)
    }

    /// Transform block number to the index of block.
    fn block_number_to_index(&self, block_number: BlockNumber) -> Option<usize> {
        if self.first_block > block_number {
            return None
        }
        let index = block_number - self.first_block;
        if index >= self.receipts.len() as u64 {
            return None
        }
        Some(index as usize)
    }

    /// Returns an iterator over all block logs.
    pub fn logs(&self, block_number: BlockNumber) -> Option<impl Iterator<Item = &Log>> {
        let index = self.block_number_to_index(block_number)?;
        Some(self.receipts[index].iter().filter_map(|r| Some(r.as_ref()?.logs.iter())).flatten())
    }

    /// Return blocks logs bloom
    pub fn block_logs_bloom(&self, block_number: BlockNumber) -> Option<Bloom> {
        Some(logs_bloom(self.logs(block_number)?))
    }

    /// Returns the receipt root for all recorded receipts.
    /// Note: this function calculated Bloom filters for every receipt and created merkle trees
    /// of receipt. This is a expensive operation.
    #[allow(unused_variables)]
    pub fn receipts_root_slow(&self, block_number: BlockNumber) -> Option<B256> {
        #[cfg(feature = "optimism")]
        panic!("This should not be called in optimism mode. Use `optimism_receipts_root_slow` instead.");
        #[cfg(not(feature = "optimism"))]
        self.receipts.root_slow(self.block_number_to_index(block_number)?)
    }

    /// Returns the receipt root for all recorded receipts.
    /// Note: this function calculated Bloom filters for every receipt and created merkle trees
    /// of receipt. This is a expensive operation.
    #[cfg(feature = "optimism")]
    pub fn optimism_receipts_root_slow(
        &self,
        block_number: BlockNumber,
        chain_spec: &reth_primitives::ChainSpec,
        timestamp: u64,
    ) -> Option<B256> {
        self.receipts.optimism_root_slow(
            self.block_number_to_index(block_number)?,
            chain_spec,
            timestamp,
        )
    }

    /// Returns reference to receipts.
    pub fn receipts(&self) -> &Receipts {
        &self.receipts
    }

    /// Returns mutable reference to receipts.
    pub fn receipts_mut(&mut self) -> &mut Receipts {
        &mut self.receipts
    }

    /// Return all block receipts
    pub fn receipts_by_block(&self, block_number: BlockNumber) -> &[Option<Receipt>] {
        let Some(index) = self.block_number_to_index(block_number) else { return &[] };
        &self.receipts[index]
    }

    /// Is bundle state empty of blocks.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of blocks in bundle state.
    pub fn len(&self) -> usize {
        self.receipts.len()
    }

    /// Return first block of the bundle
    pub fn first_block(&self) -> BlockNumber {
        self.first_block
    }

    /// Revert to given block number.
    ///
    /// If number is in future, or in the past return false
    ///
    /// NOTE: Provided block number will stay inside the bundle state.
    pub fn revert_to(&mut self, block_number: BlockNumber) -> bool {
        let Some(index) = self.block_number_to_index(block_number) else { return false };

        // +1 is for number of blocks that we have as index is included.
        let new_len = index + 1;
        let rm_trx: usize = self.len() - new_len;

        // remove receipts
        self.receipts.truncate(new_len);
        // Revert last n reverts.
        self.bundle.revert(rm_trx);

        true
    }

    /// Splits the block range state at a given block number.
    /// Returns two split states ([..at], [at..]).
    /// The plain state of the 2nd bundle state will contain extra changes
    /// that were made in state transitions belonging to the lower state.
    ///
    /// # Panics
    ///
    /// If the target block number is not included in the state block range.
    pub fn split_at(self, at: BlockNumber) -> (Option<Self>, Self) {
        if at == self.first_block {
            return (None, self)
        }

        let (mut lower_state, mut higher_state) = (self.clone(), self);

        // Revert lower state to [..at].
        lower_state.revert_to(at.checked_sub(1).unwrap());

        // Truncate higher state to [at..].
        let at_idx = higher_state.block_number_to_index(at).unwrap();
        higher_state.receipts = Receipts::from_vec(higher_state.receipts.split_off(at_idx));
        higher_state.bundle.take_n_reverts(at_idx);
        higher_state.first_block = at;

        (Some(lower_state), higher_state)
    }

    /// Extend one state from another
    ///
    /// For state this is very sensitive operation and should be used only when
    /// we know that other state was build on top of this one.
    /// In most cases this would be true.
    pub fn extend(&mut self, other: Self) {
        self.bundle.extend(other.bundle);
        self.receipts.extend(other.receipts.receipt_vec);
    }

    /// Prepends present the state with the given BundleState.
    /// It adds changes from the given state but does not override any existing changes.
    ///
    /// Reverts  and receipts are not updated.
    pub fn prepend_state(&mut self, mut other: BundleState) {
        let other_len = other.reverts.len();
        // take this bundle
        let this_bundle = std::mem::take(&mut self.bundle);
        // extend other bundle with this
        other.extend(this_bundle);
        // discard other reverts
        other.take_n_reverts(other_len);
        // swap bundles
        std::mem::swap(&mut self.bundle, &mut other)
    }

    /// Write the [BundleStateWithReceipts] to database and receipts to either database or static
    /// files if `static_file_producer` is `Some`. It should be none if there is any kind of
    /// pruning/filtering over the receipts.
    ///
    /// `omit_changed_check` should be set to true of bundle has some of it data
    /// detached, This would make some original values not known.
    pub fn write_to_storage<TX>(
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

        for (idx, receipts) in self.receipts.into_iter().enumerate() {
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
    use reth_primitives::keccak256;
    use reth_trie::{test_utils::state_root, StateRoot};
    use revm::{
        db::{
            states::{
                bundle_state::BundleRetention, changes::PlainStorageRevert, PlainStorageChangeset,
            },
            EmptyDB,
        },
        primitives::{
            Account as RevmAccount, AccountInfo as RevmAccountInfo, AccountStatus, StorageSlot,
        },
        DatabaseCommit, State,
    };
    use std::collections::BTreeMap;

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
                            StorageSlot { present_value: U256::from(1), ..Default::default() },
                        ),
                        (
                            U256::from(1),
                            StorageSlot { present_value: U256::from(2), ..Default::default() },
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
                        StorageSlot {
                            present_value: U256::from(2),
                            previous_or_original_value: U256::from(1),
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
                        StorageSlot { present_value: U256::from(1), ..Default::default() },
                    ),
                    (
                        U256::from(1),
                        StorageSlot { present_value: U256::from(2), ..Default::default() },
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
                    StorageSlot {
                        previous_or_original_value: U256::from(1),
                        present_value: U256::from(2),
                    },
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
                        StorageSlot { present_value: U256::from(2), ..Default::default() },
                    ),
                    (
                        U256::from(2),
                        StorageSlot { present_value: U256::from(4), ..Default::default() },
                    ),
                    (
                        U256::from(6),
                        StorageSlot { present_value: U256::from(6), ..Default::default() },
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
                    StorageSlot { present_value: U256::from(2), ..Default::default() },
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
                    StorageSlot { present_value: U256::from(9), ..Default::default() },
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
                        StorageSlot { present_value: U256::from(1), ..Default::default() },
                    ),
                    (
                        U256::from(1),
                        StorageSlot { present_value: U256::from(2), ..Default::default() },
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
                    StorageSlot { present_value: U256::from(5), ..Default::default() },
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
                    StorageSlot::new_changed(account2_slot2_old_value, account2_slot2_new_value),
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
                    StorageSlot::new_changed(U256::ZERO, account1_slot20_value),
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
