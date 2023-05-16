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
use std::collections::{BTreeMap, BTreeSet};

mod account;
pub use account::AccountChanges;

mod storage;
pub use storage::{Storage, StorageChanges, StorageChangeset, StorageTransition, StorageWipe};

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
    receipts: BTreeMap<BlockNumber, Vec<Receipt>>,
}

impl PostState {
    /// Create an empty [PostState].
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty [PostState] with pre-allocated space for a certain amount of transactions.
    pub fn with_tx_capacity(block: BlockNumber, txs: usize) -> Self {
        Self { receipts: BTreeMap::from([(block, Vec::with_capacity(txs))]), ..Default::default() }
    }

    /// Return the current size of the poststate.
    ///
    /// Size is the sum of individual changes to accounts, storage, bytecode and receipts.
    pub fn size_hint(&self) -> usize {
        // The amount of plain state account entries to update.
        self.accounts.len()
            // The approximate amount of plain state storage entries to update.
            // NOTE: This can be improved by manually keeping track of the storage size for each account.
            + self.storage.len()
            // The amount of bytecodes to insert.
            + self.bytecode.len()
            // The approximate amount of receipts.
            // NOTE: This can be improved by manually keeping track of the receipt size for each block number.
            + self.receipts.len()
            // The approximate amount of changsets to update.
            + self.changeset_size_hint()
    }

    /// Return the current size of history changes in the poststate.
    pub fn changeset_size_hint(&self) -> usize {
        // The amount of account changesets to insert.
        self.account_changes.size
            // The approximate amount of storage changes to insert.
            // NOTE: This does not include the entries for primary storage wipes,
            // which need to be read from plain state.
            + self.storage_changes.size
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
    pub fn receipts(&self, block: BlockNumber) -> &[Receipt] {
        self.receipts.get(&block).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Returns an iterator over all logs in this [PostState].
    pub fn logs(&self, block: BlockNumber) -> impl Iterator<Item = &Log> {
        self.receipts(block).iter().flat_map(|r| r.logs.iter())
    }

    /// Returns the logs bloom for all recorded logs.
    pub fn logs_bloom(&self, block: BlockNumber) -> Bloom {
        logs_bloom(self.logs(block))
    }

    /// Returns the receipt root for all recorded receipts.
    /// TODO: This function hides an expensive operation (bloom). We should probably make it more
    /// explicit.
    pub fn receipts_root(&self, block: BlockNumber) -> H256 {
        calculate_receipt_root_ref(self.receipts(block))
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
                HashedStorage { wiped: storage.wiped(), storage: hashed_storage },
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
        // Insert storage change sets
        for (block_number, storage_changes) in std::mem::take(&mut other.storage_changes).inner {
            for (address, their_storage_transition) in storage_changes {
                let our_storage = self.storage.entry(address).or_default();
                let (wipe, storage) = if their_storage_transition.wipe.is_wiped() {
                    // Check existing storage change.
                    match self.storage_changes.get(&block_number).and_then(|ch| ch.get(&address)) {
                        Some(change) if change.wipe.is_wiped() => (), // already counted
                        _ => {
                            our_storage.times_wiped += 1;
                        }
                    };
                    // Check if this is the first wipe.
                    let wipe = if our_storage.times_wiped == 1 {
                        StorageWipe::Primary
                    } else {
                        // Even if the wipe in other poststate was primary before, demote it to
                        // secondary.
                        StorageWipe::Secondary
                    };
                    let mut wiped_storage = std::mem::take(&mut our_storage.storage);
                    wiped_storage.extend(their_storage_transition.storage);
                    (wipe, wiped_storage)
                } else {
                    (StorageWipe::None, their_storage_transition.storage)
                };
                self.storage_changes.insert_for_block_and_address(
                    block_number,
                    address,
                    wipe,
                    storage.into_iter(),
                );
            }
        }

        // Insert account change sets
        for (block_number, account_changes) in std::mem::take(&mut other.account_changes).inner {
            self.account_changes.insert_for_block(block_number, account_changes);
        }

        // Update plain state
        self.accounts.extend(other.accounts);
        for (address, their_storage) in other.storage {
            let our_storage = self.storage.entry(address).or_default();
            our_storage.storage.extend(their_storage.storage);
        }

        self.receipts.extend(other.receipts);
        self.bytecode.extend(other.bytecode);
    }

    /// Reverts each change up to the `target_block_number` (excluding).
    ///
    /// The reverted changes are removed from this post-state, and their effects are reverted.
    pub fn revert_to(&mut self, target_block_number: BlockNumber) {
        // Revert account state & changes
        let removed_account_changes = self.account_changes.drain_above(target_block_number);
        let changed_accounts = self
            .account_changes
            .iter()
            .flat_map(|(_, account_changes)| account_changes.iter().map(|(address, _)| *address))
            .collect::<BTreeSet<_>>();
        let mut account_state: BTreeMap<Address, Option<Account>> = BTreeMap::default();
        for address in changed_accounts {
            let info = removed_account_changes
                .iter()
                .find_map(|(_, changes)| {
                    changes.iter().find_map(|ch| (ch.0 == &address).then_some(*ch.1))
                })
                .unwrap_or(*self.accounts.get(&address).expect("exists"));
            account_state.insert(address, info);
        }
        self.accounts = account_state;

        // Revert changes and recreate the storage state
        let removed_storage_changes = self.storage_changes.drain_above(target_block_number);
        let mut storage_state: BTreeMap<Address, Storage> = BTreeMap::default();
        for (_, storage_changes) in self.storage_changes.iter() {
            for (address, storage_change) in storage_changes {
                let entry = storage_state.entry(*address).or_default();
                if storage_change.wipe.is_wiped() {
                    entry.times_wiped += 1;
                }
                for (slot, _) in storage_change.storage.iter() {
                    let value = removed_storage_changes
                        .iter()
                        .find_map(|(_, changes)| {
                            changes.iter().find_map(|ch| {
                                if ch.0 == address {
                                    match ch.1.storage.iter().find_map(|(changed_slot, value)| {
                                        (slot == changed_slot).then_some(*value)
                                    }) {
                                        value @ Some(_) => Some(value),
                                        None if ch.1.wipe.is_wiped() => Some(None),
                                        None => None,
                                    }
                                } else {
                                    None
                                }
                            })
                        })
                        .unwrap_or_else(|| {
                            self.storage.get(address).and_then(|s| s.storage.get(slot).copied())
                        });
                    if let Some(value) = value {
                        entry.storage.insert(*slot, value);
                    }
                }
            }
        }
        self.storage = storage_state;

        // Revert receipts
        self.receipts.retain(|block_number, _| *block_number <= target_block_number);
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
        non_reverted_state.account_changes.retain_above(revert_to_block);
        let updated_times_wiped = non_reverted_state.storage_changes.retain_above(revert_to_block);
        // Update or reset the number of times the account was wiped.
        for (address, storage) in non_reverted_state.storage.iter_mut() {
            storage.times_wiped = updated_times_wiped.get(address).cloned().unwrap_or_default();
        }
        // Remove receipts
        non_reverted_state.receipts.retain(|block_number, _| *block_number > revert_to_block);

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
        self.account_changes.insert(block_number, address, None, Some(account));
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
        self.account_changes.insert(block_number, address, Some(old), Some(new));
    }

    /// Mark an account as destroyed.
    pub fn destroy_account(
        &mut self,
        block_number: BlockNumber,
        address: Address,
        account: Account,
    ) {
        self.accounts.insert(address, None);
        self.account_changes.insert(block_number, address, Some(account), None);

        let storage = self.storage.entry(address).or_default();
        storage.times_wiped += 1;
        let wipe =
            if storage.times_wiped == 1 { StorageWipe::Primary } else { StorageWipe::Secondary };

        let wiped_storage = std::mem::take(&mut storage.storage);
        self.storage_changes.insert_for_block_and_address(
            block_number,
            address,
            wipe,
            wiped_storage.into_iter(),
        );
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
        self.storage_changes.insert_for_block_and_address(
            block_number,
            address,
            StorageWipe::None,
            changeset.into_iter().map(|(slot, (old, _))| (slot, old)),
        );
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
    pub fn add_receipt(&mut self, block: BlockNumber, receipt: Receipt) {
        self.receipts.entry(block).or_default().push(receipt);
    }

    /// Write changeset history to the database.
    pub fn write_history_to_db<'a, TX: DbTxMut<'a> + DbTx<'a>>(
        &mut self,
        tx: &TX,
    ) -> Result<(), DbError> {
        // Write storage changes
        tracing::trace!(target: "provider::post_state", "Writing storage changes");
        let mut storages_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
        let mut storage_changeset_cursor = tx.cursor_dup_write::<tables::StorageChangeSet>()?;
        for (block_number, storage_changes) in
            std::mem::take(&mut self.storage_changes).inner.into_iter()
        {
            for (address, mut storage) in storage_changes.into_iter() {
                let storage_id = BlockNumberAddress((block_number, address));

                // If the account was created and wiped at the same block, skip all storage changes
                if storage.wipe.is_wiped() &&
                    self.account_changes
                        .get(&block_number)
                        .and_then(|changes| changes.get(&address).map(|info| info.is_none()))
                        // No account info available, fallback to `false`
                        .unwrap_or_default()
                {
                    continue
                }

                // If we are writing the primary storage wipe transition, the pre-existing plain
                // storage state has to be taken from the database and written to storage history.
                // See [StorageWipe::Primary] for more details.
                if storage.wipe.is_primary() {
                    if let Some((_, entry)) = storages_cursor.seek_exact(address)? {
                        tracing::trace!(target: "provider::post_state", ?storage_id, key = ?entry.key, "Storage wiped");
                        let key = U256::from_be_bytes(entry.key.to_fixed_bytes());
                        if !storage.storage.contains_key(&key) {
                            storage.storage.insert(entry.key.into(), entry.value);
                        }

                        while let Some(entry) = storages_cursor.next_dup_val()? {
                            let key = U256::from_be_bytes(entry.key.to_fixed_bytes());
                            if !storage.storage.contains_key(&key) {
                                storage.storage.insert(entry.key.into(), entry.value);
                            }
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

        // Write account changes
        tracing::trace!(target: "provider::post_state", "Writing account changes");
        let mut account_changeset_cursor = tx.cursor_dup_write::<tables::AccountChangeSet>()?;
        for (block_number, account_changes) in
            std::mem::take(&mut self.account_changes).inner.into_iter()
        {
            for (address, info) in account_changes.into_iter() {
                tracing::trace!(target: "provider::post_state", block_number, ?address, old = ?info, "Account changed");
                account_changeset_cursor
                    .append_dup(block_number, AccountBeforeTx { address, info })?;
            }
        }

        Ok(())
    }

    /// Write the post state to the database.
    pub fn write_to_db<'a, TX: DbTxMut<'a> + DbTx<'a>>(mut self, tx: &TX) -> Result<(), DbError> {
        self.write_history_to_db(tx)?;

        // Write new storage state
        let mut storages_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
        for (address, storage) in self.storage.into_iter() {
            // If the storage was wiped at least once, remove all previous entries from the
            // database.
            if storage.wiped() {
                tracing::trace!(target: "provider::post_state", ?address, "Wiping storage from plain state");
                if storages_cursor.seek_exact(address)?.is_some() {
                    storages_cursor.delete_current_duplicates()?;
                }
            }

            for (key, value) in storage.storage {
                tracing::trace!(target: "provider::post_state", ?address, ?key, "Updating plain state storage");
                let key: H256 = key.into();
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
        let mut bodies_cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
        let mut receipts_cursor = tx.cursor_write::<tables::Receipts>()?;
        for (block, receipts) in self.receipts {
            let (_, body_indices) = bodies_cursor.seek_exact(block)?.expect("body indices exist");
            let tx_range = body_indices.tx_num_range();
            assert_eq!(receipts.len(), tx_range.clone().count(), "Receipt length mismatch");
            for (tx_num, receipt) in tx_range.zip(receipts) {
                receipts_cursor.append(tx_num, receipt)?;
            }
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

        assert_eq!(a.account_changes.iter().fold(0, |len, (_, changes)| len + changes.len()), 0);

        let mut b = PostState::new();
        b.create_account(2, Address::repeat_byte(0xff), Account::default());

        assert_eq!(b.account_changes.iter().fold(0, |len, (_, changes)| len + changes.len()), 1);

        let mut c = a.clone();
        c.extend(b.clone());

        assert_eq!(c.account_changes.iter().fold(0, |len, (_, changes)| len + changes.len()), 1);

        let mut d = PostState::new();
        d.create_account(3, Address::zero(), Account::default());
        d.destroy_account(3, Address::zero(), Account::default());
        c.extend(d);
        assert_eq!(c.account_storage(&Address::zero()).unwrap().times_wiped, 2);
        // Primary wipe occurred at block #1.
        assert_eq!(
            c.storage_changes.get(&1).unwrap().get(&Address::zero()).unwrap().wipe,
            StorageWipe::Primary
        );
        // Primary wipe occurred at block #3.
        assert_eq!(
            c.storage_changes.get(&3).unwrap().get(&Address::zero()).unwrap().wipe,
            StorageWipe::Secondary
        );
    }

    #[test]
    fn revert_to() {
        let mut state = PostState::new();
        let address1 = Address::repeat_byte(0);
        let account1 = Account { nonce: 1, balance: U256::from(1), bytecode_hash: None };
        state.create_account(1, address1, account1);
        state.create_account(
            2,
            Address::repeat_byte(0xff),
            Account { nonce: 2, balance: U256::from(2), bytecode_hash: None },
        );
        assert_eq!(
            state.account_changes.iter().fold(0, |len, (_, changes)| len + changes.len()),
            2
        );

        let revert_to = 1;
        state.revert_to(revert_to);
        assert_eq!(state.accounts, BTreeMap::from([(address1, Some(account1))]));
        assert_eq!(
            state.account_changes.iter().fold(0, |len, (_, changes)| len + changes.len()),
            1
        );
    }

    #[test]
    fn wiped_revert() {
        let address = Address::random();

        let init_block_number = 0;
        let init_account = Account { balance: U256::from(3), ..Default::default() };
        let init_slot = U256::from(1);

        // Create init state for demonstration purposes
        // Block 0
        // Account: exists
        // Storage: 0x01: 1
        let mut init_state = PostState::new();
        init_state.create_account(init_block_number, address, init_account);
        init_state.change_storage(
            init_block_number,
            address,
            BTreeMap::from([(init_slot, (U256::ZERO, U256::from(1)))]),
        );
        assert_eq!(
            init_state.storage.get(&address),
            Some(&Storage {
                storage: BTreeMap::from([(init_slot, U256::from(1))]),
                times_wiped: 0
            })
        );

        let mut post_state = PostState::new();
        // Block 1
        // <nothing>

        // Block 2
        // Account: destroyed
        // Storage: wiped
        post_state.destroy_account(2, address, init_account);
        assert!(post_state.storage.get(&address).unwrap().wiped());

        // Block 3
        // Account: recreated
        // Storage: wiped, then 0x01: 2
        let recreated_account = Account { balance: U256::from(4), ..Default::default() };
        post_state.create_account(3, address, recreated_account);
        post_state.change_storage(
            3,
            address,
            BTreeMap::from([(init_slot, (U256::ZERO, U256::from(2)))]),
        );
        assert!(post_state.storage.get(&address).unwrap().wiped());

        // Revert to block 2
        post_state.revert_to(2);
        assert!(post_state.storage.get(&address).unwrap().wiped());
        assert_eq!(
            post_state.storage.get(&address).unwrap(),
            &Storage { times_wiped: 1, storage: BTreeMap::default() }
        );

        // Revert to block 1
        post_state.revert_to(1);
        assert_eq!(post_state.storage.get(&address), None);
    }

    #[test]
    fn split_at() {
        let address1 = Address::random();
        let address2 = Address::random();
        let slot1 = U256::from(1);
        let slot2 = U256::from(2);

        let mut state = PostState::new();
        // Block #1
        // Create account 1 and change its storage
        // Assume account 2 already exists in the database and change storage for it
        state.create_account(1, address1, Account::default());
        state.change_storage(1, address1, BTreeMap::from([(slot1, (U256::ZERO, U256::from(1)))]));
        state.change_storage(1, address1, BTreeMap::from([(slot2, (U256::ZERO, U256::from(1)))]));
        state.change_storage(1, address2, BTreeMap::from([(slot2, (U256::ZERO, U256::from(2)))]));
        let block1_account_changes = (1, BTreeMap::from([(address1, None)]));
        let block1_storage_changes = (
            1,
            BTreeMap::from([
                (
                    address1,
                    StorageTransition {
                        storage: BTreeMap::from([(slot1, U256::ZERO), (slot2, U256::ZERO)]),
                        wipe: StorageWipe::None,
                    },
                ),
                (
                    address2,
                    StorageTransition {
                        storage: BTreeMap::from([(slot2, U256::ZERO)]),
                        wipe: StorageWipe::None,
                    },
                ),
            ]),
        );
        assert_eq!(
            state.account_changes,
            AccountChanges { inner: BTreeMap::from([block1_account_changes.clone()]), size: 1 }
        );
        assert_eq!(
            state.storage_changes,
            StorageChanges { inner: BTreeMap::from([block1_storage_changes.clone()]), size: 3 }
        );

        // Block #2
        // Destroy account 1
        // Change storage for account 2
        state.destroy_account(2, address1, Account::default());
        state.change_storage(
            2,
            address2,
            BTreeMap::from([(slot2, (U256::from(2), U256::from(4)))]),
        );
        let account_state_after_block_2 = state.accounts.clone();
        let storage_state_after_block_2 = state.storage.clone();
        let block2_account_changes = (2, BTreeMap::from([(address1, Some(Account::default()))]));
        let block2_storage_changes = (
            2,
            BTreeMap::from([
                (
                    address1,
                    StorageTransition {
                        storage: BTreeMap::from([(slot1, U256::from(1)), (slot2, U256::from(1))]),
                        wipe: StorageWipe::Primary,
                    },
                ),
                (
                    address2,
                    StorageTransition {
                        storage: BTreeMap::from([(slot2, U256::from(2))]),
                        wipe: StorageWipe::None,
                    },
                ),
            ]),
        );
        assert_eq!(
            state.account_changes,
            AccountChanges {
                inner: BTreeMap::from([
                    block1_account_changes.clone(),
                    block2_account_changes.clone()
                ]),
                size: 2
            }
        );
        assert_eq!(
            state.storage_changes,
            StorageChanges {
                inner: BTreeMap::from([
                    block1_storage_changes.clone(),
                    block2_storage_changes.clone()
                ]),
                size: 6,
            }
        );

        // Block #3
        // Recreate account 1
        // Destroy account 2
        state.create_account(3, address1, Account::default());
        state.change_storage(
            3,
            address2,
            BTreeMap::from([(slot2, (U256::from(4), U256::from(1)))]),
        );
        state.destroy_account(3, address2, Account::default());
        let block3_account_changes =
            (3, BTreeMap::from([(address1, None), (address2, Some(Account::default()))]));
        let block3_storage_changes = (
            3,
            BTreeMap::from([(
                address2,
                StorageTransition {
                    storage: BTreeMap::from([(slot2, U256::from(4))]),
                    wipe: StorageWipe::Primary,
                },
            )]),
        );
        assert_eq!(
            state.account_changes,
            AccountChanges {
                inner: BTreeMap::from([
                    block1_account_changes.clone(),
                    block2_account_changes.clone(),
                    block3_account_changes.clone()
                ]),
                size: 4
            }
        );
        assert_eq!(
            state.storage_changes,
            StorageChanges {
                inner: BTreeMap::from([
                    block1_storage_changes.clone(),
                    block2_storage_changes.clone(),
                    block3_storage_changes.clone()
                ]),
                size: 7,
            }
        );

        // Block #4
        // Destroy account 1 again
        state.destroy_account(4, address1, Account::default());
        let account_state_after_block_4 = state.accounts.clone();
        let storage_state_after_block_4 = state.storage.clone();
        let block4_account_changes = (4, BTreeMap::from([(address1, Some(Account::default()))]));
        let block4_storage_changes = (
            4,
            BTreeMap::from([(
                address1,
                StorageTransition { storage: BTreeMap::default(), wipe: StorageWipe::Secondary },
            )]),
        );

        // Blocks #1-4
        // Account 1. Info: <none>. Storage: <none>. Times Wiped: 2.
        // Account 2. Info: <none>. Storage: <none>. Times Wiped: 1.
        assert_eq!(state.accounts, BTreeMap::from([(address1, None), (address2, None)]));
        assert_eq!(
            state.storage,
            BTreeMap::from([
                (address1, Storage { times_wiped: 2, storage: BTreeMap::default() }),
                (address2, Storage { times_wiped: 1, storage: BTreeMap::default() })
            ])
        );
        assert_eq!(
            state.account_changes,
            AccountChanges {
                inner: BTreeMap::from([
                    block1_account_changes.clone(),
                    block2_account_changes.clone(),
                    block3_account_changes.clone(),
                    block4_account_changes.clone(),
                ]),
                size: 5
            }
        );
        assert_eq!(
            state.storage_changes,
            StorageChanges {
                inner: BTreeMap::from([
                    block1_storage_changes.clone(),
                    block2_storage_changes.clone(),
                    block3_storage_changes.clone(),
                    block4_storage_changes,
                ]),
                size: 7,
            }
        );

        // Split state at block #2
        let mut state_1_2 = state.clone();
        let state_3_4 = state_1_2.split_at(2);

        // Blocks #1-2
        // Account 1. Info: <none>. Storage: <none>.
        // Account 2. Info: exists. Storage: slot2 - 4.
        assert_eq!(state_1_2.accounts, account_state_after_block_2);
        assert_eq!(state_1_2.storage, storage_state_after_block_2);
        assert_eq!(
            state_1_2.account_changes,
            AccountChanges {
                inner: BTreeMap::from([block1_account_changes, block2_account_changes]),
                size: 2
            }
        );
        assert_eq!(
            state_1_2.storage_changes,
            StorageChanges {
                inner: BTreeMap::from([block1_storage_changes, block2_storage_changes]),
                size: 6,
            }
        );

        // Plain state for blocks #3-4 should match plain state from blocks #1-4
        // Account 1. Info: <none>. Storage: <none>.
        // Account 2. Info: exists. Storage: slot2 - 4.
        assert_eq!(state_3_4.accounts, account_state_after_block_4);
        // Not equal because the `times_wiped` value is different.
        assert_ne!(state_3_4.storage, storage_state_after_block_4);
        assert_eq!(
            state_3_4.storage,
            BTreeMap::from([
                (address1, Storage { times_wiped: 1, storage: BTreeMap::default() }),
                (address2, Storage { times_wiped: 1, storage: BTreeMap::default() })
            ])
        );

        // Account changes should match
        assert_eq!(
            state_3_4.account_changes,
            AccountChanges {
                inner: BTreeMap::from([block3_account_changes, block4_account_changes,]),
                size: 3
            }
        );
        // Storage changes should match except for the wipe flag being promoted to primary
        assert_eq!(
            state_3_4.storage_changes,
            StorageChanges {
                inner: BTreeMap::from([
                    block3_storage_changes,
                    // Block #4. Wipe flag must be promoted to primary
                    (
                        4,
                        BTreeMap::from([(
                            address1,
                            StorageTransition {
                                storage: BTreeMap::default(),
                                wipe: StorageWipe::Primary
                            },
                        )]),
                    ),
                ]),
                size: 1,
            }
        )
    }

    #[test]
    fn receipts_split_at() {
        let mut state = PostState::new();
        (1..=4).for_each(|block| {
            state.add_receipt(block, Receipt::default());
        });
        let state2 = state.split_at(2);
        assert_eq!(
            state.receipts,
            BTreeMap::from([(1, vec![Receipt::default()]), (2, vec![Receipt::default()])])
        );
        assert_eq!(
            state2.receipts,
            BTreeMap::from([(3, vec![Receipt::default()]), (4, vec![Receipt::default()])])
        );
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
    fn write_to_db_multiple_selfdestructs() {
        let db: Arc<Env<WriteMap>> = test_utils::create_test_db(EnvKind::RW);
        let tx = db.tx_mut().expect("Could not get database tx");

        let address1 = Address::random();

        let mut init_state = PostState::new();
        init_state.create_account(0, address1, Account::default());
        init_state.change_storage(
            0,
            address1,
            // 0x00 => 0 => 1
            // 0x01 => 0 => 2
            BTreeMap::from([
                (U256::from(0), (U256::ZERO, U256::from(1))),
                (U256::from(1), (U256::ZERO, U256::from(2))),
            ]),
        );
        init_state.write_to_db(&tx).expect("Could not write init state to DB");

        let mut post_state = PostState::new();
        post_state.change_storage(
            1,
            address1,
            // 0x00 => 1 => 2
            BTreeMap::from([(U256::from(0), (U256::from(1), U256::from(2)))]),
        );
        post_state.destroy_account(2, address1, Account::default());
        post_state.create_account(3, address1, Account::default());
        post_state.change_storage(
            4,
            address1,
            // 0x00 => 0 => 2
            // 0x02 => 0 => 4
            // 0x06 => 0 => 6
            BTreeMap::from([
                (U256::from(0), (U256::ZERO, U256::from(2))),
                (U256::from(2), (U256::ZERO, U256::from(4))),
                (U256::from(6), (U256::ZERO, U256::from(6))),
            ]),
        );
        post_state.destroy_account(5, address1, Account::default());

        // Create, change, destroy and recreate in the same block.
        post_state.create_account(6, address1, Account::default());
        post_state.change_storage(
            6,
            address1,
            // 0x00 => 0 => 2
            BTreeMap::from([(U256::from(0), (U256::ZERO, U256::from(2)))]),
        );
        post_state.destroy_account(6, address1, Account::default());
        post_state.create_account(6, address1, Account::default());

        post_state.change_storage(
            7,
            address1,
            // 0x00 => 0 => 9
            BTreeMap::from([(U256::from(0), (U256::ZERO, U256::from(9)))]),
        );

        post_state.write_to_db(&tx).expect("Could not write post state to DB");

        let mut storage_changeset_cursor = tx
            .cursor_dup_read::<tables::StorageChangeSet>()
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
                StorageEntry { key: H256::from_low_u64_be(0), value: U256::ZERO }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((0, address1)),
                StorageEntry { key: H256::from_low_u64_be(1), value: U256::ZERO }
            )))
        );

        // Block #1
        // 0x00: 1
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((1, address1)),
                StorageEntry { key: H256::from_low_u64_be(0), value: U256::from(1) }
            )))
        );

        // Block #2 (destroyed)
        // 0x00: 2
        // 0x01: 2
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((2, address1)),
                StorageEntry { key: H256::from_low_u64_be(0), value: U256::from(2) }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((2, address1)),
                StorageEntry { key: H256::from_low_u64_be(1), value: U256::from(2) }
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
                StorageEntry { key: H256::from_low_u64_be(0), value: U256::ZERO }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((4, address1)),
                StorageEntry { key: H256::from_low_u64_be(2), value: U256::ZERO }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((4, address1)),
                StorageEntry { key: H256::from_low_u64_be(6), value: U256::ZERO }
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
                StorageEntry { key: H256::from_low_u64_be(0), value: U256::from(2) }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((5, address1)),
                StorageEntry { key: H256::from_low_u64_be(2), value: U256::from(4) }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((5, address1)),
                StorageEntry { key: H256::from_low_u64_be(6), value: U256::from(6) }
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
                StorageEntry { key: H256::from_low_u64_be(0), value: U256::ZERO }
            )))
        );
        assert_eq!(storage_changes.next(), None);
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
            state.account_storage(&address_a).expect("Account A should have some storage").wiped(),
            "The wiped flag should be set to discard all pre-existing storage from the database"
        );
        // Then, we must ensure that *only* the storage from the last transition will be written
        assert_eq!(
            state.account_storage(&address_a).expect("Account A should have some storage").storage,
            BTreeMap::from([(U256::from(0), U256::from(3)), (U256::from(2), U256::from(4))]),
            "Account A's storage should only have slots 0 and 2, and they should have values 3 and 4, respectively."
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
            state.account_changes().inner,
            BTreeMap::from([(block, BTreeMap::from([(address, None)]))]),
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
            state.account_changes().inner,
            BTreeMap::from([(block, BTreeMap::from([(address, None)]))]),
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
            state.storage_changes().inner,
            BTreeMap::from([(
                block,
                BTreeMap::from([(
                    address,
                    StorageTransition {
                        storage: BTreeMap::from([
                            (U256::from(0), U256::from(0)),
                            (U256::from(1), U256::from(3))
                        ]),
                        wipe: StorageWipe::None,
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
                    times_wiped: 0,
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
            a.account_changes().inner,
            BTreeMap::from([(block, BTreeMap::from([(address, None)]))]),
            "The changeset for the account is incorrect in state A"
        );
        assert_eq!(
            a.storage_changes().inner,
            BTreeMap::from([(
                block,
                BTreeMap::from([(
                    address,
                    StorageTransition {
                        storage: BTreeMap::from([(U256::from(0), U256::from(0)),]),
                        wipe: StorageWipe::None,
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
            b.account_changes().inner,
            BTreeMap::from([(
                block,
                BTreeMap::from([(
                    address,
                    Some(Account { nonce: 1, balance: U256::from(1), bytecode_hash: None })
                )])
            )]),
            "The changeset for the account is incorrect in state B"
        );
        assert_eq!(
            b.storage_changes().inner,
            BTreeMap::from([(
                block,
                BTreeMap::from([(
                    address,
                    StorageTransition {
                        storage: BTreeMap::from([(U256::from(0), U256::from(1)),]),
                        wipe: StorageWipe::None,
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
            a.account_changes().inner,
            BTreeMap::from([(block, BTreeMap::from([(address, None)]))]),
            "The changeset for the account is incorrect in the merged state"
        );
        assert_eq!(
            a.storage_changes().inner,
            BTreeMap::from([(
                block,
                BTreeMap::from([(
                    address,
                    StorageTransition {
                        storage: BTreeMap::from([(U256::from(0), U256::from(0)),]),
                        wipe: StorageWipe::None,
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
                    times_wiped: 0,
                }
            )]),
            "The latest state of the storage is incorrect in the merged state"
        );
    }

    #[test]
    fn collapsible_account_changes() {
        let address = Address::random();
        let mut post_state = PostState::default();

        // Create account on block #1
        let account_at_block_1 = Account { nonce: 1, ..Default::default() };
        post_state.create_account(1, address, account_at_block_1);

        // Modify account on block #2 and return it to original state.
        post_state.change_account(
            2,
            address,
            Account { nonce: 1, ..Default::default() },
            Account { nonce: 1, balance: U256::from(1), ..Default::default() },
        );
        post_state.change_account(
            2,
            address,
            Account { nonce: 1, balance: U256::from(1), ..Default::default() },
            Account { nonce: 1, ..Default::default() },
        );

        assert_eq!(post_state.account_changes().get(&2).and_then(|ch| ch.get(&address)), None);
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
            .map(|key| {
                let account = Account { nonce: 1, balance: U256::from(key), bytecode_hash: None };
                let storage =
                    (0..10).map(|key| (H256::from_low_u64_be(key), U256::from(key))).collect();
                (Address::from_low_u64_be(key), (account, storage))
            })
            .collect();

        let db: Arc<Env<WriteMap>> = test_utils::create_test_db(EnvKind::RW);

        // insert initial state to the database
        db.update(|tx| {
            for (address, (account, storage)) in state.iter() {
                let hashed_address = keccak256(address);
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
            *state.get(&address_2).unwrap().1.get(&slot_2_key).unwrap();
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
        let address_3_account_new = Account { balance: U256::from(24), ..address_3_account_old };
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
        let address_4_account_new = Account { nonce: 128, ..address_4_account_old };
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
