//! Output of execution.
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    models::{AccountBeforeTx, TransitionIdAddress},
    tables,
    transaction::{DbTx, DbTxMut},
    Error as DbError,
};
use reth_primitives::{
    Account, Address, Bytecode, Receipt, StorageEntry, TransitionId, H256, U256,
};
use std::collections::BTreeMap;

/// Storage for an account.
///
/// # Wiped Storage
///
/// The field `wiped` denotes whether any of the values contained in storage are valid or not; if
/// `wiped` is `true`, the storage should be considered empty.
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
        id: TransitionId,
        /// The address of the account that was created.
        address: Address,
        /// The account.
        account: Account,
    },
    /// An existing account was changed.
    AccountChanged {
        /// The ID of the transition this change is a part of.
        id: TransitionId,
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
        id: TransitionId,
        /// The address of the account associated with the storage slots.
        address: Address,
        /// The storage changeset.
        changeset: StorageChangeset,
    },
    /// Storage was wiped
    StorageWiped {
        /// The ID of the transition this change is a part of.
        id: TransitionId,
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
        id: TransitionId,
        /// The address of the destroyed account.
        address: Address,
        /// The account before it was destroyed.
        old: Account,
    },
}

impl Change {
    /// Get the transition ID for the change
    pub fn transition_id(&self) -> TransitionId {
        match self {
            Change::AccountChanged { id, .. } |
            Change::AccountCreated { id, .. } |
            Change::StorageChanged { id, .. } |
            Change::StorageWiped { id, .. } |
            Change::AccountDestroyed { id, .. } => *id,
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

    /// Set the transition ID of this change.
    pub fn set_transition_id(&mut self, new_id: TransitionId) {
        match self {
            Change::AccountChanged { ref mut id, .. } |
            Change::AccountCreated { ref mut id, .. } |
            Change::StorageChanged { ref mut id, .. } |
            Change::StorageWiped { ref mut id, .. } |
            Change::AccountDestroyed { ref mut id, .. } => {
                *id = new_id;
            }
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
/// The [Storage] type has a field, `wiped`, which denotes whether any of the values contained
/// in storage are valid or not; if `wiped` is `true`, the storage for the account should be
/// considered empty.
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
/// [PostState::finish_transition] *must* be called after every transaction, and after every block.
///
/// The first transaction executed and added to the [PostState] has a transition ID of 0, the next
/// one a transition ID of 1, and so on. If the [PostState] is for a single block, and the number of
/// transitions ([PostState::transitions_count]) is greater than the number of transactions in the
/// block, then the last transition is the block transition.
///
/// For multi-block [PostState]s it is not possible to figure out what transition ID maps on to a
/// transaction or a block.
///
/// # Shaving Allocations
///
/// Since most [PostState]s in reth are for multiple blocks it is better to pre-allocate capacity
/// for receipts and changes, which [PostState::new] does, and thus it (or
/// [PostState::with_tx_capacity]) should be preferred to using the [Default] implementation.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct PostState {
    /// The ID of the current transition.
    current_transition_id: TransitionId,
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
        Self { changes: Vec::with_capacity(PREALLOC_CHANGES_SIZE), ..Default::default() }
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

    /// Get the number of transitions causing this [PostState]
    pub fn transitions_count(&self) -> usize {
        self.current_transition_id as usize
    }

    /// Extend this [PostState] with the changes in another [PostState].
    pub fn extend(&mut self, other: PostState) {
        self.changes.reserve(other.changes.len());

        let mut next_transition_id = self.current_transition_id;
        for mut change in other.changes.into_iter() {
            next_transition_id = self.current_transition_id + change.transition_id();
            change.set_transition_id(next_transition_id);
            self.add_and_apply(change);
        }
        self.receipts.extend(other.receipts);
        self.bytecode.extend(other.bytecode);
        self.current_transition_id = next_transition_id + 1;
    }

    /// Reverts each change up to and including any change that is part of `transition_id`.
    ///
    /// The reverted changes are removed from this post-state, and their effects are reverted.
    ///
    /// The reverted changes are returned.
    pub fn revert_to(&mut self, transition_id: usize) -> Vec<Change> {
        let mut changes_to_revert = Vec::new();
        self.changes.retain(|change| {
            if change.transition_id() >= transition_id as u64 {
                changes_to_revert.push(change.clone());
                false
            } else {
                true
            }
        });

        for change in changes_to_revert.iter_mut().rev() {
            change.set_transition_id(change.transition_id() - transition_id as TransitionId);
            self.revert(change.clone());
        }
        self.current_transition_id = transition_id as TransitionId;
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
    pub fn split_at(&mut self, transition_id: usize) -> Self {
        // Clone ourselves
        let mut non_reverted_state = self.clone();

        // Revert the desired changes
        let reverted_changes = self.revert_to(transition_id);

        // Compute the new `current_transition_id` for `non_reverted_state`.
        let new_transition_id =
            reverted_changes.last().map(|c| c.transition_id()).unwrap_or_default();
        non_reverted_state.changes = reverted_changes;
        non_reverted_state.current_transition_id = new_transition_id + 1;

        non_reverted_state
    }

    /// Add a newly created account to the post-state.
    pub fn create_account(&mut self, address: Address, account: Account) {
        self.add_and_apply(Change::AccountCreated {
            id: self.current_transition_id,
            address,
            account,
        });
    }

    /// Add a changed account to the post-state.
    ///
    /// If the account also has changed storage values, [PostState::change_storage] should also be
    /// called.
    pub fn change_account(&mut self, address: Address, old: Account, new: Account) {
        self.add_and_apply(Change::AccountChanged {
            id: self.current_transition_id,
            address,
            old,
            new,
        });
    }

    /// Mark an account as destroyed.
    pub fn destroy_account(&mut self, address: Address, account: Account) {
        self.add_and_apply(Change::AccountDestroyed {
            id: self.current_transition_id,
            address,
            old: account,
        });
        self.add_and_apply(Change::StorageWiped { id: self.current_transition_id, address });
    }

    /// Add changed storage values to the post-state.
    pub fn change_storage(&mut self, address: Address, changeset: StorageChangeset) {
        self.add_and_apply(Change::StorageChanged {
            id: self.current_transition_id,
            address,
            changeset,
        });
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

    /// Mark all prior changes as being part of one transition, and start a new one.
    pub fn finish_transition(&mut self) {
        self.current_transition_id += 1;
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
                storage.wiped = false;
                for (slot, (_, current_value)) in changeset {
                    storage.storage.insert(*slot, *current_value);
                }
            }
            Change::StorageWiped { address, .. } => {
                let storage = self.storage.entry(*address).or_default();
                storage.wiped = true;
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
                storage.wiped = false;
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
    pub fn write_to_db<'a, TX: DbTxMut<'a> + DbTx<'a>>(
        mut self,
        tx: &TX,
        first_transition_id: TransitionId,
    ) -> Result<(), DbError> {
        // Collect and sort changesets by their key to improve write performance
        let mut changesets = std::mem::take(&mut self.changes);
        changesets
            .sort_unstable_by_key(|changeset| (changeset.transition_id(), changeset.address()));

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
                Change::AccountDestroyed { id, address, old } |
                Change::AccountChanged { id, address, old, .. } => {
                    let destroyed = matches!(changeset, Change::AccountDestroyed { .. });
                    tracing::trace!(target: "provider::post_state", id, ?address, ?old, destroyed, "Account changed");
                    account_changeset_cursor.append_dup(
                        first_transition_id + id,
                        AccountBeforeTx { address, info: Some(old) },
                    )?;
                }
                Change::AccountCreated { id, address, .. } => {
                    tracing::trace!(target: "provider::post_state", id, ?address, "Account created");
                    account_changeset_cursor.append_dup(
                        first_transition_id + id,
                        AccountBeforeTx { address, info: None },
                    )?;
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
                Change::StorageChanged { id, address, changeset } => {
                    let storage_id = TransitionIdAddress((first_transition_id + id, address));

                    for (key, (old_value, _)) in changeset {
                        tracing::trace!(target: "provider::post_state", ?storage_id, ?key, ?old_value, "Storage changed");
                        storage_changeset_cursor.append_dup(
                            storage_id,
                            StorageEntry { key: H256(key.to_be_bytes()), value: old_value },
                        )?;
                    }
                }
                Change::StorageWiped { id, address } => {
                    let storage_id = TransitionIdAddress((first_transition_id + id, address));

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
            if storage.wiped {
                tracing::trace!(target: "provider::post_state", ?address, "Wiping storage from plain state");
                if storages_cursor.seek_exact(address)?.is_some() {
                    storages_cursor.delete_current_duplicates()?;
                }

                // If the storage is marked as wiped, it might still contain values. This is to
                // avoid deallocating where possible, but these values should not be written to the
                // database.
                continue
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
    use std::sync::Arc;

    #[test]
    fn extend() {
        let mut a = PostState::new();
        a.create_account(Address::zero(), Account::default());
        a.destroy_account(Address::zero(), Account::default());
        a.finish_transition();

        assert_eq!(a.transitions_count(), 1);
        assert_eq!(a.changes().len(), 3);

        let mut b = PostState::new();
        b.create_account(Address::repeat_byte(0xff), Account::default());
        b.finish_transition();

        assert_eq!(b.transitions_count(), 1);
        assert_eq!(b.changes.len(), 1);

        let mut c = a.clone();
        c.extend(b.clone());

        assert_eq!(c.transitions_count(), 2);
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
        post_state.create_account(address_a, account_a);
        // 0x11.. is changed (balance + 1, nonce + 1)
        post_state.change_account(address_b, account_b, account_b_changed);
        post_state.write_to_db(&tx, 0).expect("Could not write post state to DB");

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
            changeset_cursor.seek_exact(0).expect("Could not read account change set"),
            Some((0, AccountBeforeTx { address: address_a, info: None })),
            "Account A changeset is wrong"
        );
        assert_eq!(
            changeset_cursor.next_dup().expect("Changeset table is malformed"),
            Some((0, AccountBeforeTx { address: address_b, info: Some(account_b) })),
            "Account B changeset is wrong"
        );

        let mut post_state = PostState::new();
        // 0x11.. is destroyed
        post_state.destroy_account(address_b, account_b_changed);
        post_state.write_to_db(&tx, 1).expect("Could not write second post state to DB");

        // Check new plain state for account B
        assert_eq!(
            tx.get::<tables::PlainAccountState>(address_b).expect("Could not read account state"),
            None,
            "Account B should be deleted"
        );

        // Check change set
        assert_eq!(
            changeset_cursor.seek_exact(1).expect("Could not read account change set"),
            Some((1, AccountBeforeTx { address: address_b, info: Some(account_b_changed) })),
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

        post_state.change_storage(address_a, storage_a_changeset);
        post_state.change_storage(address_b, storage_b_changeset);
        post_state.write_to_db(&tx, 0).expect("Could not write post state to DB");

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
            changeset_cursor.seek_exact(TransitionIdAddress((0, address_a))).unwrap(),
            Some((
                TransitionIdAddress((0, address_a)),
                StorageEntry { key: H256::zero(), value: U256::from(0) }
            )),
            "Slot 0 for account A should have changed from 0"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            Some((
                TransitionIdAddress((0, address_a)),
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
            changeset_cursor.seek_exact(TransitionIdAddress((0, address_b))).unwrap(),
            Some((
                TransitionIdAddress((0, address_b)),
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
        post_state.destroy_account(address_a, Account::default());
        post_state.write_to_db(&tx, 1).expect("Could not write post state to DB");

        assert_eq!(
            storage_cursor.seek_exact(address_a).unwrap(),
            None,
            "Account A should have no storage slots after deletion"
        );

        assert_eq!(
            changeset_cursor.seek_exact(TransitionIdAddress((1, address_a))).unwrap(),
            Some((
                TransitionIdAddress((1, address_a)),
                StorageEntry { key: H256::zero(), value: U256::from(1) }
            )),
            "Slot 0 for account A should have changed from 1 on deletion"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            Some((
                TransitionIdAddress((1, address_a)),
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
    fn revert_to() {
        let mut state = PostState::new();
        state.create_account(
            Address::repeat_byte(0),
            Account { nonce: 1, balance: U256::from(1), bytecode_hash: None },
        );
        state.finish_transition();
        let revert_to = state.current_transition_id;
        state.create_account(
            Address::repeat_byte(0xff),
            Account { nonce: 2, balance: U256::from(2), bytecode_hash: None },
        );
        state.finish_transition();

        assert_eq!(state.transitions_count(), 2);
        assert_eq!(state.accounts().len(), 2);

        let reverted_changes = state.revert_to(revert_to as usize);
        assert_eq!(state.accounts().len(), 1);
        assert_eq!(state.transitions_count(), 1);
        assert_eq!(reverted_changes.len(), 1);
    }
}
