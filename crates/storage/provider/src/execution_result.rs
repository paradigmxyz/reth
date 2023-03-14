//! Output of execution.

use reth_db::{models::AccountBeforeTx, tables, transaction::DbTxMut, Error as DbError};
use reth_primitives::{Account, Address, Receipt, H256, U256};
use revm_primitives::Bytecode;
use std::collections::BTreeMap;

/// Execution Result containing vector of transaction changesets
/// and block reward if present
#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub struct ExecutionResult {
    /// Transaction changeset containing [Receipt], changed [Accounts][Account] and Storages.
    pub tx_changesets: Vec<TransactionChangeSet>,
    /// Post block account changesets. This might include block reward, uncle rewards, withdrawals
    /// or irregular state changes (DAO fork).
    pub block_changesets: BTreeMap<Address, AccountInfoChangeSet>,
}

/// After transaction is executed this structure contain
/// transaction [Receipt] every change to state ([Account], Storage, [Bytecode])
/// that this transaction made and its old values
/// so that history account table can be updated.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct TransactionChangeSet {
    /// Transaction receipt
    pub receipt: Receipt,
    /// State change that this transaction made on state.
    pub changeset: BTreeMap<Address, AccountChangeSet>,
    /// new bytecode created as result of transaction execution.
    pub new_bytecodes: BTreeMap<H256, Bytecode>,
}

/// Contains old/new account changes
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AccountInfoChangeSet {
    /// The account is newly created. Account can be created by just by sending balance,
    ///
    /// Revert of this changeset is empty account,
    Created {
        /// The newly created account.
        new: Account,
    },
    /// An account was deleted (selfdestructed) or we have touched
    /// an empty account and we need to remove/destroy it.
    /// (Look at state clearing [EIP-158](https://eips.ethereum.org/EIPS/eip-158))
    ///
    /// Revert of this changeset is old account
    Destroyed {
        /// The account that was destroyed.
        old: Account,
    },
    /// The account was changed.
    ///
    /// revert of this changeset is old account
    Changed {
        /// The account after the change.
        new: Account,
        /// The account prior to the change.
        old: Account,
    },
    /// Nothing was changed for the account (nonce/balance).
    NoChange {
        /// Used to clear existing empty accounts pre-EIP-161.
        is_empty: bool,
    },
}

impl Default for AccountInfoChangeSet {
    fn default() -> Self {
        AccountInfoChangeSet::NoChange { is_empty: false }
    }
}

impl AccountInfoChangeSet {
    /// Create new account info changeset
    pub fn new(old: Option<Account>, new: Option<Account>) -> Self {
        match (old, new) {
            (Some(old), Some(new)) => {
                if new != old {
                    Self::Changed { new, old }
                } else {
                    if new.is_empty() {}
                    Self::NoChange { is_empty: true }
                }
            }
            (None, Some(new)) => Self::Created { new },
            (Some(old), None) => Self::Destroyed { old },
            (None, None) => Self::NoChange { is_empty: false },
        }
    }
    /// Apply the changes from the changeset to a database transaction.
    pub fn apply_to_db<'a, TX: DbTxMut<'a>>(
        self,
        tx: &TX,
        address: Address,
        tx_index: u64,
        has_state_clear_eip: bool,
    ) -> Result<(), DbError> {
        match self {
            AccountInfoChangeSet::Changed { old, new } => {
                // insert old account in AccountChangeSet
                // check for old != new was already done
                tx.put::<tables::AccountChangeSet>(
                    tx_index,
                    AccountBeforeTx { address, info: Some(old) },
                )?;
                tx.put::<tables::PlainAccountState>(address, new)?;
            }
            AccountInfoChangeSet::Created { new } => {
                // Ignore account that are created empty and state clear (SpuriousDragon) hardfork
                // is activated.
                if has_state_clear_eip && new.is_empty() {
                    return Ok(())
                }
                tx.put::<tables::AccountChangeSet>(
                    tx_index,
                    AccountBeforeTx { address, info: None },
                )?;
                tx.put::<tables::PlainAccountState>(address, new)?;
            }
            AccountInfoChangeSet::Destroyed { old } => {
                tx.delete::<tables::PlainAccountState>(address, None)?;
                tx.put::<tables::AccountChangeSet>(
                    tx_index,
                    AccountBeforeTx { address, info: Some(old) },
                )?;
            }
            AccountInfoChangeSet::NoChange { is_empty } => {
                if has_state_clear_eip && is_empty {
                    tx.delete::<tables::PlainAccountState>(address, None)?;
                }
            }
        }
        Ok(())
    }
}

/// Diff change set that is needed for creating history index and updating current world state.
#[derive(Debug, Default, Eq, PartialEq, Clone)]
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

    #[test]
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
    }
}
