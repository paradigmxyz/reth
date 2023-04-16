//! Output of execution.
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    models::{AccountBeforeTx, BlockNumberAddress},
    tables,
    transaction::{DbTx, DbTxMut},
    Error as DbError,
};
use reth_primitives::{
    bloom::logs_bloom, proofs::calculate_receipt_root_ref, Account, Address, BlockNumber, Bloom,
    Bytecode, Log, Receipt, StorageEntry, H256, U256,
};
use std::collections::BTreeMap;

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

/// Storage for an account with the old and new values for each slot.
/// TODO: Do we actually need (old, new) anymore, or is (old) sufficient? (Check the writes)
/// If we don't, we can unify this and [Storage].
pub type StorageChangeset = BTreeMap<U256, (U256, U256)>;

/// A change to the state of accounts or storage.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Change {
    /// A new account was created.
    AccountCreated {
        /// The ID of the transition this change is a part of.
        block_number: BlockNumber,
        /// The address of the account that was created.
        address: Address,
        /// The account.
        account: Account,
    },
    /// An existing account was changed.
    AccountChanged {
        /// The ID of the transition this change is a part of.
        block_number: BlockNumber,
        /// The address of the account that was changed.
        address: Address,
        /// The account before the change.
        old: Account,
        /// The account after the change.
        new: Account,
    },
    /// Storage slots for an account were changed.
    StorageChanged {
        /// The ID of the transition this change is a part of.
        block_number: BlockNumber,
        /// The address of the account associated with the storage slots.
        address: Address,
        /// The storage changeset.
        changeset: StorageChangeset,
    },
    /// Storage was wiped
    StorageWiped {
        /// The ID of the transition this change is a part of.
        block_number: BlockNumber,
        /// The address of the account whose storage was wiped.
        address: Address,
    },
    /// An account was destroyed.
    ///
    /// This removes all of the information associated with the account. An accompanying
    /// [Change::StorageWiped] will also be present to mark the deletion of storage.
    ///
    /// If a change to an account satisfies the conditions for EIP-158, this change variant is also
    /// applied instead of the change that would otherwise have happened.
    AccountDestroyed {
        /// The ID of the transition this change is a part of.
        block_number: BlockNumber,
        /// The address of the destroyed account.
        address: Address,
        /// The account before it was destroyed.
        old: Account,
    },
}

impl Change {
    /// Get the transition ID for the change
    pub fn block_number(&self) -> BlockNumber {
        match self {
            Change::AccountChanged { block_number, .. } |
            Change::AccountCreated { block_number, .. } |
            Change::StorageChanged { block_number, .. } |
            Change::StorageWiped { block_number, .. } |
            Change::AccountDestroyed { block_number, .. } => *block_number,
        }
    }

    /// Get the address of the account this change operates on.
    pub fn address(&self) -> Address {
        match self {
            Change::AccountChanged { address, .. } |
            Change::AccountCreated { address, .. } |
            Change::StorageChanged { address, .. } |
            Change::StorageWiped { address, .. } |
            Change::AccountDestroyed { address, .. } => *address,
        }
    }
}

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
/// Each [Change] has an `id` field that marks what transition it is part of. Each transaction is
/// its own transition, but there may be 0 or 1 transitions associated with the block.
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
#[derive(Debug, Clone, Eq, PartialEq)]
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
    /// The changes to state that happened during execution
    changes: Vec<Change>,
    /// New code created during the execution
    bytecode: BTreeMap<H256, Bytecode>,
    /// The receipt(s) of the executed transaction(s).
    receipts: Vec<Receipt>,
}

/// Used to determine preallocation sizes of [PostState]'s internal [Vec]s. It denotes the number of
/// best-guess changes each transaction causes to state.
const BEST_GUESS_CHANGES_PER_TX: usize = 8;

/// How many [Change]s to preallocate for in [PostState].
///
/// This is just a guesstimate based on:
///
/// - Each block having ~200-300 transactions
/// - Each transaction having some amount of changes
const PREALLOC_CHANGES_SIZE: usize = 256 * BEST_GUESS_CHANGES_PER_TX;

impl PostState {
    /// Create an empty [PostState].
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty [PostState] with pre-allocated space for a certain amount of transactions.
    pub fn with_tx_capacity(txs: usize) -> Self {
        Self {
            changes: Vec::with_capacity(txs * BEST_GUESS_CHANGES_PER_TX),
            receipts: Vec::with_capacity(txs),
            ..Default::default()
        }
    }

    /// Get the latest state of accounts.
    pub fn accounts(&self) -> &BTreeMap<Address, Option<Account>> {
        &self.accounts
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

    /// Get the changes causing this [PostState].
    pub fn changes(&self) -> &[Change] {
        &self.changes
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

    /// Extend this [PostState] with the changes in another [PostState].
    pub fn extend(&mut self, other: PostState) {
        if other.changes.is_empty() {
            return
        }

        self.changes.reserve(other.changes.len());

        for change in other.changes.into_iter() {
            self.add_and_apply(change);
        }
        self.receipts.extend(other.receipts);
        self.bytecode.extend(other.bytecode);
    }

    /// Reverts each change up to and including any change that is part of `transition_id`.
    ///
    /// The reverted changes are removed from this post-state, and their effects are reverted.
    ///
    /// The reverted changes are returned.
    pub fn revert_to(&mut self, block_number: BlockNumber) -> Vec<Change> {
        let mut changes_to_revert = Vec::new();
        self.changes.retain(|change| {
            if change.block_number() > block_number {
                changes_to_revert.push(change.clone());
                false
            } else {
                true
            }
        });

        for change in changes_to_revert.iter_mut().rev() {
            self.revert(change.clone());
        }
        changes_to_revert
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
    pub fn split_at(&mut self, block_number: BlockNumber) -> Self {
        // Clone ourselves
        let mut non_reverted_state = self.clone();

        // Revert the desired changes
        let reverted_changes = self.revert_to(block_number);

        non_reverted_state.changes = reverted_changes;

        non_reverted_state
    }

    /// Add a newly created account to the post-state.
    pub fn create_account(
        &mut self,
        block_number: BlockNumber,
        address: Address,
        account: Account,
    ) {
        self.add_and_apply(Change::AccountCreated { block_number, address, account });
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
        self.add_and_apply(Change::AccountChanged { block_number, address, old, new });
    }

    /// Mark an account as destroyed.
    pub fn destroy_account(
        &mut self,
        block_number: BlockNumber,
        address: Address,
        account: Account,
    ) {
        self.add_and_apply(Change::AccountDestroyed { block_number, address, old: account });
        self.add_and_apply(Change::StorageWiped { block_number, address });
    }

    /// Add changed storage values to the post-state.
    pub fn change_storage(
        &mut self,
        block_number: BlockNumber,
        address: Address,
        changeset: StorageChangeset,
    ) {
        self.add_and_apply(Change::StorageChanged { block_number, address, changeset });
    }

    /// Add new bytecode to the post-state.
    pub fn add_bytecode(&mut self, code_hash: H256, bytecode: Bytecode) {
        // TODO: Is this faster than just doing `.insert`?
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

    /// Add a new change, and apply its transformations to the current state
    pub fn add_and_apply(&mut self, change: Change) {
        match &change {
            Change::AccountCreated { address, account, .. } |
            Change::AccountChanged { address, new: account, .. } => {
                self.accounts.insert(*address, Some(*account));
            }
            Change::AccountDestroyed { address, .. } => {
                self.accounts.insert(*address, None);
            }
            Change::StorageChanged { address, changeset, .. } => {
                let storage = self.storage.entry(*address).or_default();
                for (slot, (_, current_value)) in changeset {
                    storage.storage.insert(*slot, *current_value);
                }
            }
            Change::StorageWiped { address, .. } => {
                let storage = self.storage.entry(*address).or_default();
                storage.wiped = true;
                storage.storage.clear();
            }
        }

        self.changes.push(change);
    }

    /// Revert a change, applying the inverse of its transformations to the current state.
    fn revert(&mut self, change: Change) {
        match &change {
            Change::AccountCreated { address, .. } => {
                self.accounts.remove(address);
            }
            Change::AccountChanged { address, old, .. } => {
                self.accounts.insert(*address, Some(*old));
            }
            Change::AccountDestroyed { address, old, .. } => {
                self.accounts.insert(*address, Some(*old));
            }
            Change::StorageChanged { address, changeset, .. } => {
                let storage = self.storage.entry(*address).or_default();
                for (slot, (old_value, _)) in changeset {
                    storage.storage.insert(*slot, *old_value);
                }
            }
            Change::StorageWiped { address, .. } => {
                let storage = self.storage.entry(*address).or_default();
                storage.wiped = false;
            }
        }
    }

    /// Write the post state to the database.
    pub fn write_to_db<'a, TX: DbTxMut<'a> + DbTx<'a>>(mut self, tx: &TX) -> Result<(), DbError> {
        // Collect and sort changesets by their key to improve write performance
        let mut changesets = std::mem::take(&mut self.changes);
        changesets
            .sort_unstable_by_key(|changeset| (changeset.block_number(), changeset.address()));

        // Partition changesets into account and storage changes
        let (account_changes, storage_changes): (Vec<Change>, Vec<Change>) =
            changesets.into_iter().partition(|changeset| {
                matches!(
                    changeset,
                    Change::AccountChanged { .. } |
                        Change::AccountCreated { .. } |
                        Change::AccountDestroyed { .. }
                )
            });

        // Write account changes
        tracing::trace!(target: "provider::post_state", len = account_changes.len(), "Writing account changes");
        let mut account_changeset_cursor = tx.cursor_dup_write::<tables::AccountChangeSet>()?;
        for changeset in account_changes.into_iter() {
            match changeset {
                Change::AccountDestroyed { block_number, address, old } |
                Change::AccountChanged { block_number, address, old, .. } => {
                    let destroyed = matches!(changeset, Change::AccountDestroyed { .. });
                    tracing::trace!(target: "provider::post_state", block_number, ?address, ?old, destroyed, "Account changed");
                    account_changeset_cursor
                        .append_dup(block_number, AccountBeforeTx { address, info: Some(old) })?;
                }
                Change::AccountCreated { block_number, address, .. } => {
                    tracing::trace!(target: "provider::post_state", block_number, ?address, "Account created");
                    account_changeset_cursor
                        .append_dup(block_number, AccountBeforeTx { address, info: None })?;
                }
                _ => unreachable!(),
            }
        }

        // Write storage changes
        tracing::trace!(target: "provider::post_state", len = storage_changes.len(), "Writing storage changes");
        let mut storages_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
        let mut storage_changeset_cursor = tx.cursor_dup_write::<tables::StorageChangeSet>()?;
        for changeset in storage_changes.into_iter() {
            match changeset {
                Change::StorageChanged { block_number, address, changeset } => {
                    let storage_id = BlockNumberAddress((block_number, address));

                    for (key, (old_value, _)) in changeset {
                        tracing::trace!(target: "provider::post_state", ?storage_id, ?key, ?old_value, "Storage changed");
                        storage_changeset_cursor.append_dup(
                            storage_id,
                            StorageEntry { key: H256(key.to_be_bytes()), value: old_value },
                        )?;
                    }
                }
                Change::StorageWiped { block_number, address } => {
                    let storage_id = BlockNumberAddress((block_number, address));

                    if let Some((_, entry)) = storages_cursor.seek_exact(address)? {
                        tracing::trace!(target: "provider::post_state", ?storage_id, key = ?entry.key, "Storage wiped");
                        storage_changeset_cursor.append_dup(storage_id, entry)?;

                        while let Some(entry) = storages_cursor.next_dup_val()? {
                            storage_changeset_cursor.append_dup(storage_id, entry)?;
                        }
                    }
                }
                _ => unreachable!(),
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

        // write the receipts of the transactions
        let mut receipts_cursor = tx.cursor_write::<tables::Receipts>()?;

        let mut next_tx_num =
            if let Some(last_tx) = receipts_cursor.last()?.map(|(tx_num, _)| tx_num) {
                last_tx + 1
            } else {
                // the very first tx
                0
            };
        for receipt in self.receipts.into_iter() {
            receipts_cursor.append(next_tx_num, receipt)?;
            next_tx_num += 1;
        }

        Ok(())
    }
}

impl Default for PostState {
    fn default() -> Self {
        Self {
            accounts: Default::default(),
            storage: Default::default(),
            changes: Vec::with_capacity(PREALLOC_CHANGES_SIZE),
            bytecode: Default::default(),
            receipts: vec![],
        }
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
        assert_eq!(a.changes.len(), 1);
    }

    #[test]
    fn extend() {
        let mut a = PostState::new();
        a.create_account(1, Address::zero(), Account::default());
        a.destroy_account(1, Address::zero(), Account::default());

        assert_eq!(a.changes().len(), 3);

        let mut b = PostState::new();
        b.create_account(2, Address::repeat_byte(0xff), Account::default());

        assert_eq!(b.changes.len(), 1);

        let mut c = a.clone();
        c.extend(b.clone());

        assert_eq!(c.changes.len(), a.changes.len() + b.changes.len());
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

        assert_eq!(state.accounts().len(), 2);

        let reverted_changes = state.revert_to(revert_to);
        assert_eq!(state.accounts().len(), 1);
        assert_eq!(reverted_changes.len(), 1);
    }
}
