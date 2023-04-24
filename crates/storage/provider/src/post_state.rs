//! Output of execution.
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    models::{AccountBeforeTx, BlockNumberAddress},
    tables,
    transaction::{DbTx, DbTxMut},
    Error as DbError,
};
use reth_primitives::{
    bloom::logs_bloom, keccak256, proofs::calculate_receipt_root_ref, Account, Address,
    BlockNumber, Bloom, Bytecode, Log, Receipt, StorageEntry, H256, U256,
};
use reth_trie::{
    hashed_cursor::{HashedPostState, HashedPostStateCursorFactory, HashedStorage},
    StateRoot, StateRootError,
};
use std::collections::BTreeMap;

/// Storage for an account with the old and new values for each slot: (slot -> (old, new)).
pub type StorageChangeset = BTreeMap<U256, (U256, U256)>;

/// A mapping of `block -> address -> account` that represents what accounts were changed, and what
/// their state were prior to that change.
///
/// If the prior state was `None`, then the account is new.
pub type AccountChanges = BTreeMap<BlockNumber, BTreeMap<Address, Option<Account>>>;

/// A mapping of `block -> account -> slot -> old value` that represents what slots were changed,
/// and what their values were prior to that change.
pub type StorageChanges = BTreeMap<BlockNumber, BTreeMap<Address, Storage>>;

/// Storage for an account.
///
/// # Wiped Storage
///
/// The field `wiped` denotes whether the pre-existing storage in the database should be cleared or
/// not.
///
/// If `wiped` is true, then the account was selfdestructed at some point, and the values contained
/// in `storage` should be the only values written to the database.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct Storage {
    /// Whether the storage was wiped or not.
    pub wiped: bool,
    /// The storage slots.
    pub storage: BTreeMap<U256, U256>,
}

// todo: rewrite all the docs for this
/// The state of accounts after execution of one or more transactions, including receipts and new
/// bytecode.
///
/// The latest state can be found in `accounts`, `storage`, and `bytecode`. The receipts for the
/// transactions that lead to these changes can be found in `receipts`, and each change leading to
/// this state can be found in `changes`.
///
/// # Wiped Storage
///
/// The [Storage] type has a field, `wiped` which denotes whether the pre-existing storage in the
/// database should be cleared or not.
///
/// If `wiped` is true, then the account was selfdestructed at some point, and the values contained
/// in `storage` should be the only values written to the database.
///
/// # Transitions
///
/// The block level transition includes:
///
/// - Block rewards
/// - Ommer rewards
/// - Withdrawals
/// - The irregular state change for the DAO hardfork
///
/// For multi-block [PostState]s it is not possible to figure out what transition ID maps on to a
/// transaction or a block.
///
/// # Shaving Allocations
///
/// Since most [PostState]s in reth are for multiple blocks it is better to pre-allocate capacity
/// for receipts and changes, which [PostState::new] does, and thus it (or
/// [PostState::with_tx_capacity]) should be preferred to using the [Default] implementation.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct PostState {
    /// The state of all modified accounts after execution.
    ///
    /// If the value contained is `None`, then the account should be deleted.
    accounts: BTreeMap<Address, Option<Account>>,
    /// The state of all modified storage after execution
    ///
    /// If the contained [Storage] is marked as wiped, then all storage values should be cleared
    /// from the database.
    storage: BTreeMap<Address, Storage>,
    /// The state of accounts before they were changed in the given block.
    ///
    /// If the value is `None`, then the account is new, otherwise it is a change.
    account_changes: AccountChanges,
    /// The state of account storage before it was changed in the given block.
    ///
    /// This map only contains old values for storage slots.
    storage_changes: StorageChanges,
    /// New code created during the execution
    bytecode: BTreeMap<H256, Bytecode>,
    /// The receipt(s) of the executed transaction(s).
    receipts: Vec<Receipt>,
}

impl PostState {
    /// Create an empty [PostState].
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty [PostState] with pre-allocated space for a certain amount of transactions.
    pub fn with_tx_capacity(txs: usize) -> Self {
        Self { receipts: Vec::with_capacity(txs), ..Default::default() }
    }

    /// Get the latest state of all changed accounts.
    pub fn accounts(&self) -> &BTreeMap<Address, Option<Account>> {
        &self.accounts
    }

    /// Get a reference to all the account changes
    pub fn account_changes(&self) -> &AccountChanges {
        &self.account_changes
    }

    /// Get a reference to all the storage changes
    pub fn storage_changes(&self) -> &StorageChanges {
        &self.storage_changes
    }

    /// Get the latest state for a specific account.
    ///
    /// # Returns
    ///
    /// - `None` if the account does not exist
    /// - `Some(&None)` if the account existed, but has since been deleted.
    /// - `Some(..)` if the account currently exists
    pub fn account(&self, address: &Address) -> Option<&Option<Account>> {
        self.accounts.get(address)
    }

    /// Get the latest state of storage.
    pub fn storage(&self) -> &BTreeMap<Address, Storage> {
        &self.storage
    }

    /// Get the storage for an account.
    pub fn account_storage(&self, address: &Address) -> Option<&Storage> {
        self.storage.get(address)
    }

    /// Get the newly created bytecodes
    pub fn bytecodes(&self) -> &BTreeMap<H256, Bytecode> {
        &self.bytecode
    }

    /// Get a bytecode in the post-state.
    pub fn bytecode(&self, code_hash: &H256) -> Option<&Bytecode> {
        self.bytecode.get(code_hash)
    }

    /// Get the receipts for the transactions executed to form this [PostState].
    pub fn receipts(&self) -> &[Receipt] {
        &self.receipts
    }

    /// Returns an iterator over all logs in this [PostState].
    pub fn logs(&self) -> impl Iterator<Item = &Log> + '_ {
        self.receipts().iter().flat_map(|r| r.logs.iter())
    }

    /// Returns the logs bloom for all recorded logs.
    pub fn logs_bloom(&self) -> Bloom {
        logs_bloom(self.logs())
    }

    /// Returns the receipt root for all recorded receipts.
    pub fn receipts_root(&self) -> H256 {
        calculate_receipt_root_ref(self.receipts().iter().map(Into::into))
    }

    /// Hash all changed accounts and storage entries that are currently stored in the post state.
    ///
    /// # Returns
    ///
    /// The hashed post state.
    pub fn hash_state_slow(&self) -> HashedPostState {
        let mut accounts = BTreeMap::default();
        for (address, account) in self.accounts() {
            accounts.insert(keccak256(address), *account);
        }

        let mut storages = BTreeMap::default();
        for (address, storage) in self.storage() {
            let mut hashed_storage = BTreeMap::default();
            for (slot, value) in &storage.storage {
                hashed_storage.insert(keccak256(H256(slot.to_be_bytes())), *value);
            }
            storages.insert(
                keccak256(address),
                HashedStorage { wiped: storage.wiped, storage: hashed_storage },
            );
        }

        HashedPostState { accounts, storages }
    }

    /// Calculate the state root for this [PostState].
    /// Internally, function calls [Self::hash_state_slow] to obtain the [HashedPostState].
    /// Afterwards, it retrieves the prefixsets from the [HashedPostState] and uses them to
    /// calculate the incremental state root.
    ///
    /// # Example
    ///
    /// ```
    /// use reth_primitives::{Address, Account};
    /// use reth_provider::PostState;
    /// use reth_db::{mdbx::{EnvKind, WriteMap, test_utils::create_test_db}, database::Database};
    ///
    /// // Initialize the database
    /// let db = create_test_db::<WriteMap>(EnvKind::RW);
    ///
    /// // Initialize the post state
    /// let mut post_state = PostState::new();
    ///
    /// // Create an account
    /// let block_number = 1;
    /// let address = Address::random();
    /// post_state.create_account(1, address, Account { nonce: 1, ..Default::default() });
    ///
    /// // Calculate the state root
    /// let tx = db.tx().expect("failed to create transaction");
    /// let state_root = post_state.state_root_slow(&tx);
    /// ```
    ///
    /// # Returns
    ///
    /// The state root for this [PostState].
    pub fn state_root_slow<'a, 'tx, TX: DbTx<'tx>>(
        &self,
        tx: &'a TX,
    ) -> Result<H256, StateRootError> {
        let hashed_post_state = self.hash_state_slow();
        let (account_prefix_set, storage_prefix_set) = hashed_post_state.construct_prefix_sets();
        let hashed_cursor_factory = HashedPostStateCursorFactory::new(tx, &hashed_post_state);
        StateRoot::new(tx)
            .with_hashed_cursor_factory(&hashed_cursor_factory)
            .with_changed_account_prefixes(account_prefix_set)
            .with_changed_storage_prefixes(storage_prefix_set)
            .root()
    }

    // todo: note overwrite behavior, i.e. changes in `other` take precedent
    /// Extend this [PostState] with the changes in another [PostState].
    pub fn extend(&mut self, mut other: PostState) {
        // Update plain state
        self.accounts.extend(other.accounts);
        for (address, their_storage) in other.storage {
            let our_storage = self.storage.entry(address).or_default();
            if their_storage.wiped {
                our_storage.wiped = true;
                our_storage.storage.clear();
            }
            our_storage.storage.extend(their_storage.storage);
        }

        // Insert account change sets
        for (block_number, account_changes) in std::mem::take(&mut other.account_changes) {
            let block = self.account_changes.entry(block_number).or_default();
            for (address, account) in account_changes {
                if block.contains_key(&address) {
                    continue
                }
                block.insert(address, account);
            }
        }

        // Insert storage change sets
        for (block_number, storage_changes) in std::mem::take(&mut other.storage_changes) {
            for (address, their_storage) in storage_changes {
                let our_storage = self
                    .storage_changes
                    .entry(block_number)
                    .or_default()
                    .entry(address)
                    .or_default();

                if their_storage.wiped {
                    our_storage.wiped = true;
                }
                for (slot, value) in their_storage.storage {
                    our_storage.storage.entry(slot).or_insert(value);
                }
            }
        }
        self.receipts.extend(other.receipts);
        self.bytecode.extend(other.bytecode);
    }

    /// Reverts each change up to and including any change that is part of `block_number`.
    ///
    /// The reverted changes are removed from this post-state, and their effects are reverted.
    pub fn revert_to(&mut self, target_block_number: BlockNumber) {
        let mut account_changes_to_revert = BTreeMap::new();
        self.account_changes.retain(|block_number, accounts| {
            if *block_number > target_block_number {
                account_changes_to_revert.insert(*block_number, accounts.clone());
                false
            } else {
                true
            }
        });
        for (_, accounts) in account_changes_to_revert.into_iter().rev() {
            self.accounts.extend(accounts);
        }

        let mut storage_changes_to_revert = BTreeMap::new();
        self.storage_changes.retain(|block_number, storages| {
            if *block_number > target_block_number {
                storage_changes_to_revert.insert(*block_number, storages.clone());
                false
            } else {
                true
            }
        });
        for (_, storages) in storage_changes_to_revert.into_iter().rev() {
            for (address, storage) in storages {
                self.storage.entry(address).and_modify(|head_storage| {
                    head_storage.wiped = storage.wiped;
                    head_storage.storage.extend(storage.clone().storage);
                });
            }
        }
    }

    /// Reverts each change up to and including any change that is part of `transition_id`.
    ///
    /// The reverted changes are removed from this post-state, and their effects are reverted.
    ///
    /// A new post-state containing the pre-revert state, as well as the reverted changes *only* is
    /// returned.
    ///
    /// This effectively splits the post state in two:
    ///
    /// 1. This post-state has the changes reverted
    /// 2. The returned post-state does *not* have the changes reverted, but only contains the
    /// descriptions of the changes that were reverted in the first post-state.
    pub fn split_at(&mut self, revert_to_block: BlockNumber) -> Self {
        // Clone ourselves
        let mut non_reverted_state = self.clone();

        // Revert the desired changes
        self.revert_to(revert_to_block);

        // Remove all changes in the returned post-state that were not reverted
        non_reverted_state
            .storage_changes
            .retain(|block_number, _| *block_number > revert_to_block);
        non_reverted_state
            .account_changes
            .retain(|block_number, _| *block_number > revert_to_block);

        non_reverted_state
    }

    /// Add a newly created account to the post-state.
    pub fn create_account(
        &mut self,
        block_number: BlockNumber,
        address: Address,
        account: Account,
    ) {
        self.accounts.insert(address, Some(account));
        self.account_changes.entry(block_number).or_default().entry(address).or_insert(None);
    }

    /// Add a changed account to the post-state.
    ///
    /// If the account also has changed storage values, [PostState::change_storage] should also be
    /// called.
    pub fn change_account(
        &mut self,
        block_number: BlockNumber,
        address: Address,
        old: Account,
        new: Account,
    ) {
        self.accounts.insert(address, Some(new));
        self.account_changes.entry(block_number).or_default().entry(address).or_insert(Some(old));
    }

    /// Mark an account as destroyed.
    pub fn destroy_account(
        &mut self,
        block_number: BlockNumber,
        address: Address,
        account: Account,
    ) {
        self.accounts.insert(address, None);
        self.account_changes
            .entry(block_number)
            .or_default()
            .entry(address)
            .or_insert(Some(account));
        let storage = self.storage.entry(address).or_default();
        storage.wiped = true;
        storage.storage.clear();
        let storage_changes =
            self.storage_changes.entry(block_number).or_default().entry(address).or_default();
        storage_changes.wiped = true;
    }

    /// Add changed storage values to the post-state.
    pub fn change_storage(
        &mut self,
        block_number: BlockNumber,
        address: Address,
        changeset: StorageChangeset,
    ) {
        self.storage
            .entry(address)
            .or_default()
            .storage
            .extend(changeset.iter().map(|(slot, (_, new))| (*slot, *new)));
        let storage_changes =
            self.storage_changes.entry(block_number).or_default().entry(address).or_default();
        for (slot, (old, _)) in changeset.into_iter() {
            storage_changes.storage.entry(slot).or_insert(old);
        }
    }

    /// Add new bytecode to the post-state.
    pub fn add_bytecode(&mut self, code_hash: H256, bytecode: Bytecode) {
        // Assumption: `insert` will override the value if present, but since the code hash for a
        // given bytecode will always be the same, we are overriding with the same value.
        //
        // In other words: if this entry already exists, replacing the bytecode will replace with
        // the same value, which is wasteful.
        self.bytecode.entry(code_hash).or_insert(bytecode);
    }

    /// Add a transaction receipt to the post-state.
    ///
    /// Transactions should always include their receipts in the post-state.
    pub fn add_receipt(&mut self, receipt: Receipt) {
        self.receipts.push(receipt);
    }

    /// Write the post state to the database.
    pub fn write_to_db<'a, TX: DbTxMut<'a> + DbTx<'a>>(self, tx: &TX) -> Result<(), DbError> {
        // Write account changes
        tracing::trace!(target: "provider::post_state", "Writing account changes");
        let mut account_changeset_cursor = tx.cursor_dup_write::<tables::AccountChangeSet>()?;
        for (block_number, account_changes) in self.account_changes.into_iter() {
            for (address, info) in account_changes.into_iter() {
                tracing::trace!(target: "provider::post_state", block_number, ?address, old = ?info, "Account changed");
                account_changeset_cursor
                    .append_dup(block_number, AccountBeforeTx { address, info })?;
            }
        }

        // Write storage changes
        tracing::trace!(target: "provider::post_state", "Writing storage changes");
        let mut storages_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
        let mut storage_changeset_cursor = tx.cursor_dup_write::<tables::StorageChangeSet>()?;
        for (block_number, storage_changes) in self.storage_changes.into_iter() {
            for (address, mut storage) in storage_changes.into_iter() {
                let storage_id = BlockNumberAddress((block_number, address));

                if storage.wiped {
                    if let Some((_, entry)) = storages_cursor.seek_exact(address)? {
                        tracing::trace!(target: "provider::post_state", ?storage_id, key = ?entry.key, "Storage wiped");
                        storage.storage.insert(entry.key.into(), entry.value);

                        while let Some(entry) = storages_cursor.next_dup_val()? {
                            storage.storage.insert(entry.key.into(), entry.value);
                        }
                    }
                }

                for (slot, old_value) in storage.storage {
                    tracing::trace!(target: "provider::post_state", ?storage_id, ?slot, ?old_value, "Storage changed");
                    storage_changeset_cursor.append_dup(
                        storage_id,
                        StorageEntry { key: H256(slot.to_be_bytes()), value: old_value },
                    )?;
                }
            }
        }

        // Write new storage state
        for (address, storage) in self.storage.into_iter() {
            // If the storage was wiped, remove all previous entries from the database.
            if storage.wiped {
                tracing::trace!(target: "provider::post_state", ?address, "Wiping storage from plain state");
                if storages_cursor.seek_exact(address)?.is_some() {
                    storages_cursor.delete_current_duplicates()?;
                }
            }

            for (key, value) in storage.storage {
                tracing::trace!(target: "provider::post_state", ?address, ?key, "Updating plain state storage");
                let key = H256(key.to_be_bytes());
                if let Some(entry) = storages_cursor.seek_by_key_subkey(address, key)? {
                    if entry.key == key {
                        storages_cursor.delete_current()?;
                    }
                }

                if value != U256::ZERO {
                    storages_cursor.upsert(address, StorageEntry { key, value })?;
                }
            }
        }

        // Write new account state
        tracing::trace!(target: "provider::post_state", len = self.accounts.len(), "Writing new account state");
        let mut accounts_cursor = tx.cursor_write::<tables::PlainAccountState>()?;
        for (address, account) in self.accounts.into_iter() {
            if let Some(account) = account {
                tracing::trace!(target: "provider::post_state", ?address, "Updating plain state account");
                accounts_cursor.upsert(address, account)?;
            } else if accounts_cursor.seek_exact(address)?.is_some() {
                tracing::trace!(target: "provider::post_state", ?address, "Deleting plain state account");
                accounts_cursor.delete_current()?;
            }
        }

        // Write bytecode
        tracing::trace!(target: "provider::post_state", len = self.bytecode.len(), "Writing bytecods");
        let mut bytecodes_cursor = tx.cursor_write::<tables::Bytecodes>()?;
        for (hash, bytecode) in self.bytecode.into_iter() {
            bytecodes_cursor.upsert(hash, bytecode)?;
        }

        // Write the receipts of the transactions
        let mut receipts_cursor = tx.cursor_write::<tables::Receipts>()?;
        let mut next_tx_num =
            if let Some(last_tx) = receipts_cursor.last()?.map(|(tx_num, _)| tx_num) {
                last_tx + 1
            } else {
                // The very first tx
                0
            };
        for receipt in self.receipts.into_iter() {
            receipts_cursor.append(next_tx_num, receipt)?;
            next_tx_num += 1;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db::{
        database::Database,
        mdbx::{test_utils, Env, EnvKind, WriteMap},
        transaction::DbTx,
    };
    use reth_primitives::proofs::EMPTY_ROOT;
    use reth_trie::test_utils::state_root;
    use std::sync::Arc;

    // Ensure that the transition id is not incremented if postate is extended by another empty
    // poststate.
    #[test]
    fn extend_empty() {
        let mut a = PostState::new();

        // Extend empty poststate with another empty poststate
        a.extend(PostState::new());

        // Add single transition and extend with empty poststate
        a.create_account(1, Address::zero(), Account::default());
        a.extend(PostState::new());
        assert_eq!(a.account_changes.iter().fold(0, |len, (_, changes)| len + changes.len()), 1);
    }

    #[test]
    fn extend() {
        let mut a = PostState::new();
        a.create_account(1, Address::zero(), Account::default());
        a.destroy_account(1, Address::zero(), Account::default());

        assert_eq!(a.account_changes.iter().fold(0, |len, (_, changes)| len + changes.len()), 1);

        let mut b = PostState::new();
        b.create_account(2, Address::repeat_byte(0xff), Account::default());

        assert_eq!(b.account_changes.iter().fold(0, |len, (_, changes)| len + changes.len()), 1);

        let mut c = a.clone();
        c.extend(b.clone());

        assert_eq!(c.account_changes.iter().fold(0, |len, (_, changes)| len + changes.len()), 2);
    }

    #[test]
    fn write_to_db_account_info() {
        let db: Arc<Env<WriteMap>> = test_utils::create_test_db(EnvKind::RW);
        let tx = db.tx_mut().expect("Could not get database tx");

        let mut post_state = PostState::new();

        let address_a = Address::zero();
        let address_b = Address::repeat_byte(0xff);

        let account_a = Account { balance: U256::from(1), nonce: 1, bytecode_hash: None };
        let account_b = Account { balance: U256::from(2), nonce: 2, bytecode_hash: None };
        let account_b_changed = Account { balance: U256::from(3), nonce: 3, bytecode_hash: None };

        // 0x00.. is created
        post_state.create_account(1, address_a, account_a);
        // 0x11.. is changed (balance + 1, nonce + 1)
        post_state.change_account(1, address_b, account_b, account_b_changed);
        post_state.write_to_db(&tx).expect("Could not write post state to DB");

        // Check plain state
        assert_eq!(
            tx.get::<tables::PlainAccountState>(address_a).expect("Could not read account state"),
            Some(account_a),
            "Account A state is wrong"
        );
        assert_eq!(
            tx.get::<tables::PlainAccountState>(address_b).expect("Could not read account state"),
            Some(account_b_changed),
            "Account B state is wrong"
        );

        // Check change set
        let mut changeset_cursor = tx
            .cursor_dup_read::<tables::AccountChangeSet>()
            .expect("Could not open changeset cursor");
        assert_eq!(
            changeset_cursor.seek_exact(1).expect("Could not read account change set"),
            Some((1, AccountBeforeTx { address: address_a, info: None })),
            "Account A changeset is wrong"
        );
        assert_eq!(
            changeset_cursor.next_dup().expect("Changeset table is malformed"),
            Some((1, AccountBeforeTx { address: address_b, info: Some(account_b) })),
            "Account B changeset is wrong"
        );

        let mut post_state = PostState::new();
        // 0x11.. is destroyed
        post_state.destroy_account(2, address_b, account_b_changed);
        post_state.write_to_db(&tx).expect("Could not write second post state to DB");

        // Check new plain state for account B
        assert_eq!(
            tx.get::<tables::PlainAccountState>(address_b).expect("Could not read account state"),
            None,
            "Account B should be deleted"
        );

        // Check change set
        assert_eq!(
            changeset_cursor.seek_exact(2).expect("Could not read account change set"),
            Some((2, AccountBeforeTx { address: address_b, info: Some(account_b_changed) })),
            "Account B changeset is wrong after deletion"
        );
    }

    #[test]
    fn write_to_db_storage() {
        let db: Arc<Env<WriteMap>> = test_utils::create_test_db(EnvKind::RW);
        let tx = db.tx_mut().expect("Could not get database tx");

        let mut post_state = PostState::new();

        let address_a = Address::zero();
        let address_b = Address::repeat_byte(0xff);

        // 0x00 => 0 => 1
        // 0x01 => 0 => 2
        let storage_a_changeset = BTreeMap::from([
            (U256::from(0), (U256::from(0), U256::from(1))),
            (U256::from(1), (U256::from(0), U256::from(2))),
        ]);

        // 0x01 => 1 => 2
        let storage_b_changeset = BTreeMap::from([(U256::from(1), (U256::from(1), U256::from(2)))]);

        post_state.change_storage(1, address_a, storage_a_changeset);
        post_state.change_storage(1, address_b, storage_b_changeset);
        post_state.write_to_db(&tx).expect("Could not write post state to DB");

        // Check plain storage state
        let mut storage_cursor = tx
            .cursor_dup_read::<tables::PlainStorageState>()
            .expect("Could not open plain storage state cursor");

        assert_eq!(
            storage_cursor.seek_exact(address_a).unwrap(),
            Some((address_a, StorageEntry { key: H256::zero(), value: U256::from(1) })),
            "Slot 0 for account A should be 1"
        );
        assert_eq!(
            storage_cursor.next_dup().unwrap(),
            Some((
                address_a,
                StorageEntry { key: H256::from(U256::from(1).to_be_bytes()), value: U256::from(2) }
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
                StorageEntry { key: H256::from(U256::from(1).to_be_bytes()), value: U256::from(2) }
            )),
            "Slot 1 for account B should be 2"
        );
        assert_eq!(
            storage_cursor.next_dup().unwrap(),
            None,
            "Account B should only have 1 storage slot"
        );

        // Check change set
        let mut changeset_cursor = tx
            .cursor_dup_read::<tables::StorageChangeSet>()
            .expect("Could not open storage changeset cursor");
        assert_eq!(
            changeset_cursor.seek_exact(BlockNumberAddress((1, address_a))).unwrap(),
            Some((
                BlockNumberAddress((1, address_a)),
                StorageEntry { key: H256::zero(), value: U256::from(0) }
            )),
            "Slot 0 for account A should have changed from 0"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            Some((
                BlockNumberAddress((1, address_a)),
                StorageEntry { key: H256::from(U256::from(1).to_be_bytes()), value: U256::from(0) }
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
                StorageEntry { key: H256::from(U256::from(1).to_be_bytes()), value: U256::from(1) }
            )),
            "Slot 1 for account B should have changed from 1"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            None,
            "Account B should only be in the changeset 1 time"
        );

        // Delete account A
        let mut post_state = PostState::new();
        post_state.destroy_account(2, address_a, Account::default());
        post_state.write_to_db(&tx).expect("Could not write post state to DB");

        assert_eq!(
            storage_cursor.seek_exact(address_a).unwrap(),
            None,
            "Account A should have no storage slots after deletion"
        );

        assert_eq!(
            changeset_cursor.seek_exact(BlockNumberAddress((2, address_a))).unwrap(),
            Some((
                BlockNumberAddress((2, address_a)),
                StorageEntry { key: H256::zero(), value: U256::from(1) }
            )),
            "Slot 0 for account A should have changed from 1 on deletion"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            Some((
                BlockNumberAddress((2, address_a)),
                StorageEntry { key: H256::from(U256::from(1).to_be_bytes()), value: U256::from(2) }
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
    fn reuse_selfdestructed_account() {
        let address_a = Address::zero();

        // 0x00 => 0 => 1
        // 0x01 => 0 => 2
        // 0x03 => 0 => 3
        let storage_changeset_one = BTreeMap::from([
            (U256::from(0), (U256::from(0), U256::from(1))),
            (U256::from(1), (U256::from(0), U256::from(2))),
            (U256::from(3), (U256::from(0), U256::from(3))),
        ]);
        // 0x00 => 0 => 3
        // 0x01 => 0 => 4
        let storage_changeset_two = BTreeMap::from([
            (U256::from(0), (U256::from(0), U256::from(3))),
            (U256::from(2), (U256::from(0), U256::from(4))),
        ]);

        let mut state = PostState::new();

        // Create some storage for account A (simulates a contract deployment)
        state.change_storage(1, address_a, storage_changeset_one);
        // Next transition destroys the account (selfdestruct)
        state.destroy_account(2, address_a, Account::default());
        // Next transition recreates account A with some storage (simulates a contract deployment)
        state.change_storage(3, address_a, storage_changeset_two);

        // All the storage of account A has to be deleted in the database (wiped)
        assert!(
            state.account_storage(&address_a).expect("Account A should have some storage").wiped,
            "The wiped flag should be set to discard all pre-existing storage from the database"
        );
        // Then, we must ensure that *only* the storage from the last transition will be written
        assert_eq!(
            state.account_storage(&address_a).expect("Account A should have some storage").storage,
            BTreeMap::from([(U256::from(0), U256::from(3)), (U256::from(2), U256::from(4))]),
            "Account A's storage should only have slots 0 and 2, and they should have values 3 and 4, respectively."
        );
    }

    #[test]
    fn revert_to() {
        let mut state = PostState::new();
        state.create_account(
            1,
            Address::repeat_byte(0),
            Account { nonce: 1, balance: U256::from(1), bytecode_hash: None },
        );
        let revert_to = 1;
        state.create_account(
            2,
            Address::repeat_byte(0xff),
            Account { nonce: 2, balance: U256::from(2), bytecode_hash: None },
        );

        assert_eq!(
            state.account_changes.iter().fold(0, |len, (_, changes)| len + changes.len()),
            2
        );

        state.revert_to(revert_to);
        assert_eq!(
            state.account_changes.iter().fold(0, |len, (_, changes)| len + changes.len()),
            1
        );
    }

    /// Checks that if an account is touched multiple times in the same block,
    /// then the old value from the first change is kept and not overwritten.
    ///
    /// This is important because post states from different transactions in the same block may see
    /// different states of the same account as the old value, but the changeset should reflect the
    /// state of the account before the block.
    #[test]
    fn account_changesets_keep_old_values() {
        let mut state = PostState::new();
        let block = 1;
        let address = Address::repeat_byte(0);

        // A transaction in block 1 creates the account
        state.create_account(
            block,
            address,
            Account { nonce: 1, balance: U256::from(1), bytecode_hash: None },
        );

        // A transaction in block 1 then changes the same account
        state.change_account(
            block,
            address,
            Account { nonce: 1, balance: U256::from(1), bytecode_hash: None },
            Account { nonce: 1, balance: U256::from(2), bytecode_hash: None },
        );

        // The value in the changeset for the account should be `None` since this was an account
        // creation
        assert_eq!(
            state.account_changes(),
            &BTreeMap::from([(block, BTreeMap::from([(address, None)]))]),
            "The changeset for the account is incorrect"
        );

        // The latest state of the account should be: nonce = 1, balance = 2, bytecode hash = None
        assert_eq!(
            state.accounts.get(&address).unwrap(),
            &Some(Account { nonce: 1, balance: U256::from(2), bytecode_hash: None }),
            "The latest state of the account is incorrect"
        );

        // Another transaction in block 1 then changes the account yet again
        state.change_account(
            block,
            address,
            Account { nonce: 1, balance: U256::from(2), bytecode_hash: None },
            Account { nonce: 2, balance: U256::from(1), bytecode_hash: None },
        );

        // The value in the changeset for the account should still be `None`
        assert_eq!(
            state.account_changes(),
            &BTreeMap::from([(block, BTreeMap::from([(address, None)]))]),
            "The changeset for the account is incorrect"
        );

        // The latest state of the account should be: nonce = 2, balance = 1, bytecode hash = None
        assert_eq!(
            state.accounts.get(&address).unwrap(),
            &Some(Account { nonce: 2, balance: U256::from(1), bytecode_hash: None }),
            "The latest state of the account is incorrect"
        );
    }

    /// Checks that if a storage slot is touched multiple times in the same block,
    /// then the old value from the first change is kept and not overwritten.
    ///
    /// This is important because post states from different transactions in the same block may see
    /// different states of the same account as the old value, but the changeset should reflect the
    /// state of the account before the block.
    #[test]
    fn storage_changesets_keep_old_values() {
        let mut state = PostState::new();
        let block = 1;
        let address = Address::repeat_byte(0);

        // A transaction in block 1 changes:
        //
        // Slot 0: 0 -> 1
        // Slot 1: 3 -> 4
        state.change_storage(
            block,
            address,
            BTreeMap::from([
                (U256::from(0), (U256::from(0), U256::from(1))),
                (U256::from(1), (U256::from(3), U256::from(4))),
            ]),
        );

        // A transaction in block 1 changes:
        //
        // Slot 0: 1 -> 2
        // Slot 1: 4 -> 5
        state.change_storage(
            block,
            address,
            BTreeMap::from([
                (U256::from(0), (U256::from(1), U256::from(2))),
                (U256::from(1), (U256::from(4), U256::from(5))),
            ]),
        );

        // The storage changeset for the account in block 1 should now be:
        //
        // Slot 0: 0 (the value before the first tx in the block)
        // Slot 1: 3
        assert_eq!(
            state.storage_changes(),
            &BTreeMap::from([(
                block,
                BTreeMap::from([(
                    address,
                    Storage {
                        storage: BTreeMap::from([
                            (U256::from(0), U256::from(0)),
                            (U256::from(1), U256::from(3))
                        ]),
                        wiped: false,
                    }
                )])
            )]),
            "The changeset for the storage is incorrect"
        );

        // The latest state of the storage should be:
        //
        // Slot 0: 2
        // Slot 1: 5
        assert_eq!(
            state.storage(),
            &BTreeMap::from([(
                address,
                Storage {
                    storage: BTreeMap::from([
                        (U256::from(0), U256::from(2)),
                        (U256::from(1), U256::from(5))
                    ]),
                    wiped: false
                }
            )]),
            "The latest state of the storage is incorrect"
        );
    }

    /// Tests that the oldest value for changesets is kept when extending a post state from another
    /// post state.
    ///
    /// In other words, this tests the same cases as `account_changesets_keep_old_values` and
    /// `storage_changesets_keep_old_values`, but in the case where accounts/slots are changed in
    /// different post states that are then merged.
    #[test]
    fn extending_preserves_changesets() {
        let mut a = PostState::new();
        let mut b = PostState::new();
        let block = 1;
        let address = Address::repeat_byte(0);

        // The first state (a) represents a transaction that creates an account with some storage
        // slots
        //
        // Expected changeset state:
        // - Account: None
        // - Storage: Slot 0: 0
        a.create_account(
            block,
            address,
            Account { nonce: 1, balance: U256::from(1), bytecode_hash: None },
        );
        a.change_storage(
            block,
            address,
            BTreeMap::from([(U256::from(0), (U256::from(0), U256::from(1)))]),
        );
        assert_eq!(
            a.account_changes(),
            &BTreeMap::from([(block, BTreeMap::from([(address, None)]))]),
            "The changeset for the account is incorrect in state A"
        );
        assert_eq!(
            a.storage_changes(),
            &BTreeMap::from([(
                block,
                BTreeMap::from([(
                    address,
                    Storage {
                        storage: BTreeMap::from([(U256::from(0), U256::from(0)),]),
                        wiped: false,
                    }
                )])
            )]),
            "The changeset for the storage is incorrect in state A"
        );

        // The second state (b) represents a transaction that changes some slots and account info
        // for the same account
        //
        // Expected changeset state is the same, i.e.:
        // - Account: None
        // - Storage: Slot 0: 0
        b.change_account(
            block,
            address,
            Account { nonce: 1, balance: U256::from(1), bytecode_hash: None },
            Account { nonce: 1, balance: U256::from(10), bytecode_hash: None },
        );
        b.change_storage(
            block,
            address,
            BTreeMap::from([(U256::from(0), (U256::from(1), U256::from(2)))]),
        );
        assert_eq!(
            b.account_changes(),
            &BTreeMap::from([(
                block,
                BTreeMap::from([(
                    address,
                    Some(Account { nonce: 1, balance: U256::from(1), bytecode_hash: None })
                )])
            )]),
            "The changeset for the account is incorrect in state B"
        );
        assert_eq!(
            b.storage_changes(),
            &BTreeMap::from([(
                block,
                BTreeMap::from([(
                    address,
                    Storage {
                        storage: BTreeMap::from([(U256::from(0), U256::from(1)),]),
                        wiped: false,
                    }
                )])
            )]),
            "The changeset for the storage is incorrect in state B"
        );

        // Now we merge the states
        a.extend(b);

        // The expected state is:
        //
        // Changesets:
        // - Account: None
        // - Storage: Slot 0: 0
        //
        // Accounts:
        // - Nonce 1, balance 10, bytecode hash None
        //
        // Storage:
        // - Slot 0: 2
        assert_eq!(
            a.account_changes(),
            &BTreeMap::from([(block, BTreeMap::from([(address, None)]))]),
            "The changeset for the account is incorrect in the merged state"
        );
        assert_eq!(
            a.storage_changes(),
            &BTreeMap::from([(
                block,
                BTreeMap::from([(
                    address,
                    Storage {
                        storage: BTreeMap::from([(U256::from(0), U256::from(0)),]),
                        wiped: false,
                    }
                )])
            )]),
            "The changeset for the storage is incorrect in the merged state"
        );
        assert_eq!(
            a.accounts(),
            &BTreeMap::from([(
                address,
                Some(Account { nonce: 1, balance: U256::from(10), bytecode_hash: None })
            )]),
            "The state of accounts in the merged state is incorrect"
        );
        assert_eq!(
            a.storage(),
            &BTreeMap::from([(
                address,
                Storage {
                    storage: BTreeMap::from([(U256::from(0), U256::from(2)),]),
                    wiped: false
                }
            )]),
            "The latest state of the storage is incorrect in the merged state"
        );
    }

    #[test]
    fn empty_post_state_state_root() {
        let db: Arc<Env<WriteMap>> = test_utils::create_test_db(EnvKind::RW);
        let tx = db.tx().unwrap();

        let post_state = PostState::new();
        let state_root = post_state.state_root_slow(&tx).expect("Could not get state root");
        assert_eq!(state_root, EMPTY_ROOT);
    }

    #[test]
    fn post_state_state_root() {
        let mut state: BTreeMap<Address, (Account, BTreeMap<H256, U256>)> = (0..10)
            .into_iter()
            .map(|key| {
                let account = Account { nonce: 1, balance: U256::from(key), bytecode_hash: None };
                let storage = (0..10)
                    .into_iter()
                    .map(|key| (H256::from_low_u64_be(key), U256::from(key)))
                    .collect();
                (Address::from_low_u64_be(key), (account, storage))
            })
            .collect();

        let db: Arc<Env<WriteMap>> = test_utils::create_test_db(EnvKind::RW);

        // insert initial state to the database
        db.update(|tx| {
            for (address, (account, storage)) in state.iter() {
                let hashed_address = keccak256(&address);
                tx.put::<tables::HashedAccount>(hashed_address, *account).unwrap();
                for (slot, value) in storage {
                    tx.put::<tables::HashedStorage>(
                        hashed_address,
                        StorageEntry { key: keccak256(slot), value: *value },
                    )
                    .unwrap();
                }
            }

            let (_, updates) = StateRoot::new(tx).root_with_updates().unwrap();
            updates.flush(tx).unwrap();
        })
        .unwrap();

        let block_number = 1;
        let tx = db.tx().unwrap();
        let mut post_state = PostState::new();

        // database only state root is correct
        assert_eq!(
            post_state.state_root_slow(&tx).unwrap(),
            state_root(
                state
                    .clone()
                    .into_iter()
                    .map(|(address, (account, storage))| (address, (account, storage.into_iter())))
            )
        );

        // destroy account 1
        let address_1 = Address::from_low_u64_be(1);
        let account_1_old = state.remove(&address_1).unwrap();
        post_state.destroy_account(block_number, address_1, account_1_old.0);
        assert_eq!(
            post_state.state_root_slow(&tx).unwrap(),
            state_root(
                state
                    .clone()
                    .into_iter()
                    .map(|(address, (account, storage))| (address, (account, storage.into_iter())))
            )
        );

        // change slot 2 in account 2
        let address_2 = Address::from_low_u64_be(2);
        let slot_2 = U256::from(2);
        let slot_2_key = H256(slot_2.to_be_bytes());
        let address_2_slot_2_old_value =
            state.get(&address_2).unwrap().1.get(&slot_2_key).unwrap().clone();
        let address_2_slot_2_new_value = U256::from(100);
        state.get_mut(&address_2).unwrap().1.insert(slot_2_key, address_2_slot_2_new_value);
        post_state.change_storage(
            block_number,
            address_2,
            BTreeMap::from([(slot_2, (address_2_slot_2_old_value, address_2_slot_2_new_value))]),
        );
        assert_eq!(
            post_state.state_root_slow(&tx).unwrap(),
            state_root(
                state
                    .clone()
                    .into_iter()
                    .map(|(address, (account, storage))| (address, (account, storage.into_iter())))
            )
        );

        // change balance of account 3
        let address_3 = Address::from_low_u64_be(3);
        let address_3_account_old = state.get(&address_3).unwrap().0;
        let address_3_account_new =
            Account { balance: U256::from(24), ..address_3_account_old.clone() };
        state.get_mut(&address_3).unwrap().0.balance = address_3_account_new.balance;
        post_state.change_account(
            block_number,
            address_3,
            address_3_account_old,
            address_3_account_new,
        );
        assert_eq!(
            post_state.state_root_slow(&tx).unwrap(),
            state_root(
                state
                    .clone()
                    .into_iter()
                    .map(|(address, (account, storage))| (address, (account, storage.into_iter())))
            )
        );

        // change nonce of account 4
        let address_4 = Address::from_low_u64_be(4);
        let address_4_account_old = state.get(&address_4).unwrap().0;
        let address_4_account_new = Account { nonce: 128, ..address_4_account_old.clone() };
        state.get_mut(&address_4).unwrap().0.nonce = address_4_account_new.nonce;
        post_state.change_account(
            block_number,
            address_4,
            address_4_account_old,
            address_4_account_new,
        );
        assert_eq!(
            post_state.state_root_slow(&tx).unwrap(),
            state_root(
                state
                    .clone()
                    .into_iter()
                    .map(|(address, (account, storage))| (address, (account, storage.into_iter())))
            )
        );

        // recreate account 1
        let account_1_new =
            Account { nonce: 56, balance: U256::from(123), bytecode_hash: Some(H256::random()) };
        state.insert(address_1, (account_1_new, BTreeMap::default()));
        post_state.create_account(block_number, address_1, account_1_new);
        assert_eq!(
            post_state.state_root_slow(&tx).unwrap(),
            state_root(
                state
                    .clone()
                    .into_iter()
                    .map(|(address, (account, storage))| (address, (account, storage.into_iter())))
            )
        );

        // update storage for account 1
        let slot_20 = U256::from(20);
        let slot_20_key = H256(slot_20.to_be_bytes());
        let account_1_slot_20_value = U256::from(12345);
        state.get_mut(&address_1).unwrap().1.insert(slot_20_key, account_1_slot_20_value);
        post_state.change_storage(
            block_number,
            address_1,
            BTreeMap::from([(slot_20, (U256::from(0), account_1_slot_20_value))]),
        );
        assert_eq!(
            post_state.state_root_slow(&tx).unwrap(),
            state_root(
                state
                    .clone()
                    .into_iter()
                    .map(|(address, (account, storage))| (address, (account, storage.into_iter())))
            )
        );
    }
}
