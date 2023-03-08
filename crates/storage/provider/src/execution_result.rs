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
/// TODO: Better docs
#[derive(Debug, Default, Clone)]
pub struct Storage {
    /// Whether the storage was wiped or not.
    pub wiped: bool,
    /// The storage slots.
    pub storage: BTreeMap<U256, U256>,
}

/// Storage for an account with the old and new values for each slot.
/// TODO: Better docs
/// TODO: Do we actually need (old, new) anymore, or is (old) sufficient? (Check the writes)
/// If we don't, we can unify this and [Storage].
#[derive(Debug, Default, Clone)]
pub struct StorageChangeset {
    /// Whether the storage was wiped or not.
    pub wiped: bool,
    /// The storage slots, where each entry is a tuple of the old and new value.
    pub storage: BTreeMap<U256, (U256, U256)>,
}

/// A set of changes for an account.
#[derive(Debug, Default, Clone)]
pub struct ChangeSet {
    /// The transition ID associated with this changeset.
    pub id: TransitionId,
    /// The address of the account that was changed.
    pub address: Address,
    /// The storage slots that were changed.
    pub storage: StorageChangeset,
    /// The type of change the account info underwent.
    pub account: AccountInfoChangeSet,
}

/// Execution Result containing vector of transaction changesets
/// and block reward if present
#[derive(Debug, Default, Clone)]
pub struct ExecutionResult {
    /// The ID of the next transition.
    next_transition_id: TransitionId,
    /// The state of all modified accounts after execution
    pub accounts: BTreeMap<Address, Option<Account>>,
    /// The state of all modified storage after execution
    pub storage: BTreeMap<Address, Storage>,
    /// The changes to state that happened during execution
    pub changesets: Vec<ChangeSet>,
    /// New code created during the execution
    pub bytecode: BTreeMap<H256, Bytecode>,
    /// The receipts of the executed transactions.
    pub receipts: Vec<Receipt>,
}

// TODO: Reduce clones
impl ExecutionResult {
    /// Extend this [ExecutionResult] with the changes in another [ExecutionResult].
    pub fn extend(&mut self, other: ExecutionResult) {
        let mut next_transition_id = self.next_transition_id;
        for mut changeset in other.changesets {
            next_transition_id = self.next_transition_id + changeset.id;
            changeset.id = next_transition_id;
            self.apply_changeset(changeset.clone());
            self.changesets.push(changeset);
        }
        self.next_transition_id = next_transition_id;
    }

    /// Add a [TransactionChangeSet] to this execution result.
    pub fn push_transaction_changes(&mut self, changeset: TransactionChangeSet) {
        let changeset_id = self.next_transition_id;
        for (address, changes) in changeset.changeset.into_iter() {
            let changes = ChangeSet {
                id: changeset_id,
                address,
                account: changes.account,
                storage: StorageChangeset { storage: changes.storage, wiped: changes.wipe_storage },
            };
            self.apply_changeset(changes.clone());
            self.changesets.push(changes);
        }
        self.receipts.push(changeset.receipt);
        self.bytecode.extend(changeset.new_bytecodes);
        self.next_transition_id += 1;
    }

    /// Add block specific account changes.
    pub fn push_block_changes(&mut self, changesets: BTreeMap<Address, AccountInfoChangeSet>) {
        let changeset_id = self.next_transition_id;
        for (address, changes) in changesets {
            let changes =
                ChangeSet { id: changeset_id, address, account: changes, ..Default::default() };
            self.apply_changeset(changes.clone());
            self.changesets.push(changes);
        }
        self.next_transition_id += 1;
    }

    /// Write the execution result to the database.
    pub fn write_to_db<'a, TX: DbTxMut<'a> + DbTx<'a>>(
        self,
        tx: &TX,
        first_transition_id: TransitionId,
    ) -> Result<(), DbError> {
        // Write account changes
        let mut account_changeset_cursor = tx.cursor_dup_write::<tables::AccountChangeSet>()?;
        let account_changes: BTreeMap<(TransitionId, Address), AccountInfoChangeSet> = self
            .changesets
            .iter()
            .map(|changeset| ((changeset.id, changeset.address), changeset.account.clone()))
            .collect();
        for ((id, address), changeset) in account_changes {
            match changeset {
                AccountInfoChangeSet::Changed { old, .. } |
                AccountInfoChangeSet::Destroyed { old } => {
                    // insert old account in AccountChangeSet
                    // check for old != new was already done
                    account_changeset_cursor.append_dup(
                        first_transition_id + id,
                        AccountBeforeTx { address, info: Some(old) },
                    )?;
                }
                AccountInfoChangeSet::Created { .. } => {
                    // Ignore account that are created empty and state clear (SpuriousDragon)
                    // hardfork is activated.
                    //if has_state_clear_eip && new.is_empty() {
                    //    continue
                    //}
                    account_changeset_cursor.append_dup(
                        first_transition_id + id,
                        AccountBeforeTx { address, info: None },
                    )?;
                }
                _ => (),
            }
        }

        // Write storage changes
        let mut storage_changeset_cursor = tx.cursor_dup_write::<tables::StorageChangeSet>()?;
        for changeset in self.changesets.iter() {
            let address = changeset.address;
            let storage_id =
                TransitionIdAddress((first_transition_id + changeset.id, changeset.address));
            if changeset.storage.wiped {
                // iterate over storage and save them before entry is deleted.
                tx.cursor_read::<tables::PlainStorageState>()?
                    .walk(Some(address))?
                    .take_while(|res| res.as_ref().map(|(k, _)| *k == address).unwrap_or_default())
                    .try_for_each(|entry| {
                        let (_, old_value) = entry?;
                        storage_changeset_cursor.append_dup(storage_id, old_value)
                    })?;
            } else {
                for (key, (old_value, _)) in changeset.storage.storage.iter() {
                    storage_changeset_cursor.append_dup(
                        storage_id,
                        StorageEntry { key: H256(key.to_be_bytes()), value: *old_value },
                    )?;
                }
            }
        }

        // Write new storage state
        let mut storages_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
        for (address, storage) in self.storage {
            if storage.wiped && storages_cursor.seek_exact(address)?.is_some() {
                storages_cursor.delete_current_duplicates()?;
            }

            for (key, value) in storage.storage {
                let key = H256(key.to_be_bytes());
                if storages_cursor.seek_by_key_subkey(address, key)?.is_some() {
                    storages_cursor.delete_current()?;
                }

                if value != U256::ZERO {
                    storages_cursor.upsert(address, StorageEntry { key, value })?;
                }
            }
        }

        // Write new account state
        let mut accounts_cursor = tx.cursor_write::<tables::PlainAccountState>()?;
        for (address, account) in self.accounts {
            let current = accounts_cursor.seek_exact(address)?;
            if let Some(account) = account {
                /*if has_state_clear_eip && account.is_empty() {
                    // TODO: seek and then delete
                    continue
                }*/
                accounts_cursor.upsert(address, account)?;
            } else if current.is_some() {
                accounts_cursor.delete_current()?;
            }
        }

        // Write bytecode
        let mut bytecodes_cursor = tx.cursor_write::<tables::Bytecodes>()?;
        for (hash, bytecode) in self.bytecode {
            bytecodes_cursor.upsert(hash, bytecode)?;
        }

        Ok(())
    }

    /// Internally apply a changeset to update the current state of the accounts and storage values.
    fn apply_changeset(&mut self, changeset: ChangeSet) {
        match changeset.account {
            AccountInfoChangeSet::Destroyed { .. } => {
                self.accounts.insert(changeset.address, None);
            }
            AccountInfoChangeSet::Changed { new, .. } |
            AccountInfoChangeSet::Created { new, .. } => {
                self.accounts.insert(changeset.address, Some(new));
            }
            _ => (),
        }

        let entry = self.storage.entry(changeset.address).or_default();
        if changeset.storage.wiped {
            entry.storage.clear();
        }
        entry.wiped = changeset.storage.wiped;
        entry.storage.extend(changeset.storage.storage.iter().map(|(key, (_, new))| (key, new)));
    }
}

/// After transaction is executed this structure contain
/// transaction [Receipt] every change to state ([Account], Storage, [Bytecode])
/// that this transaction made and its old values
/// so that history account table can be updated.
#[derive(Debug, Default, Clone)]
pub struct TransactionChangeSet {
    /// Transaction receipt
    pub receipt: Receipt,
    /// State change that this transaction made on state.
    pub changeset: BTreeMap<Address, AccountChangeSet>,
    /// new bytecode created as result of transaction execution.
    pub new_bytecodes: BTreeMap<H256, Bytecode>,
}

/// Diff change set that is needed for creating history index and updating current world state.
#[derive(Debug, Default, Clone)]
pub struct AccountChangeSet {
    /// Old and New account account change.
    pub account: AccountInfoChangeSet,
    /// Storage containing key -> (OldValue,NewValue). in case that old value is not existing
    /// we can expect to have U256::ZERO, same with new value.
    pub storage: BTreeMap<U256, (U256, U256)>,
    /// Just to make sure that we are taking selfdestruct cleaning we have this field that wipes
    /// storage. There are instances where storage is changed but account is not touched, so we
    /// can't take into account that if new account is None that it is selfdestruct.
    pub wipe_storage: bool,
}

/// Contains old/new account changes
#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub enum AccountInfoChangeSet {
    /// The account is newly created.
    Created {
        /// The newly created account.
        new: Account,
    },
    /// An account was deleted (selfdestructed) or we have touched
    /// an empty account and we need to remove/destroy it.
    /// (Look at state clearing [EIP-158](https://eips.ethereum.org/EIPS/eip-158))
    Destroyed {
        /// The account that was destroyed.
        old: Account,
    },
    /// The account was changed.
    Changed {
        /// The account after the change.
        new: Account,
        /// The account prior to the change.
        old: Account,
    },
    /// Nothing was changed for the account (nonce/balance).
    #[default]
    NoChange,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reth_db::{
        database::Database,
        mdbx::{test_utils, Env, EnvKind, WriteMap},
        transaction::DbTx,
    };
    use reth_primitives::H160;

    use super::*;

    /*#[test]
    fn apply_account_info_changeset() {
        let db: Arc<Env<WriteMap>> = test_utils::create_test_db(EnvKind::RW);
        let address = H160::zero();
        let tx_num = 0;
        let acc1 = Account { balance: U256::from(1), nonce: 2, bytecode_hash: Some(H256::zero()) };
        let acc2 = Account { balance: U256::from(3), nonce: 4, bytecode_hash: Some(H256::zero()) };

        let tx = db.tx_mut().unwrap();

        // check Changed changeset
        AccountInfoChangeSet::Changed { new: acc1, old: acc2 }
            .apply_to_db(&tx, address, tx_num, true)
            .unwrap();
        assert_eq!(
            tx.get::<tables::AccountChangeSet>(tx_num),
            Ok(Some(AccountBeforeTx { address, info: Some(acc2) }))
        );
        assert_eq!(tx.get::<tables::PlainAccountState>(address), Ok(Some(acc1)));

        AccountInfoChangeSet::Created { new: acc1 }
            .apply_to_db(&tx, address, tx_num, true)
            .unwrap();
        assert_eq!(
            tx.get::<tables::AccountChangeSet>(tx_num),
            Ok(Some(AccountBeforeTx { address, info: None }))
        );
        assert_eq!(tx.get::<tables::PlainAccountState>(address), Ok(Some(acc1)));

        // delete old value, as it is dupsorted
        tx.delete::<tables::AccountChangeSet>(tx_num, None).unwrap();

        AccountInfoChangeSet::Destroyed { old: acc2 }
            .apply_to_db(&tx, address, tx_num, true)
            .unwrap();
        assert_eq!(tx.get::<tables::PlainAccountState>(address), Ok(None));
        assert_eq!(
            tx.get::<tables::AccountChangeSet>(tx_num),
            Ok(Some(AccountBeforeTx { address, info: Some(acc2) }))
        );
    }*/
}
