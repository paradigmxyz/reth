//! Contains the `AccountChanges` struct, which represents storage, balance, nonce, code changes and
//! read for the account. All changes for a single account, grouped by field type.
//! This eliminates address redundancy across different change types.

use alloy_primitives::{Address, StorageKey};

use crate::{
    balance_change::BalanceChange, code_change::CodeChange, nonce_change::NonceChange,
    StorageChange, MAX_SLOTS, MAX_TXS,
};

/// This struct is used to track the changes across accounts in a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountChanges {
    /// The address of the account whoose changes are stored.
    pub address: Address,
    /// List of storage changes for this account.
    pub storage_changes: Vec<StorageChange>,
    /// List of storage reads for this account.
    pub storage_reads: Vec<StorageKey>,
    /// List of balance changes for this account.
    pub balance_changes: Vec<BalanceChange>,
    /// List of nonce changes for this account.
    pub nonce_changes: Vec<NonceChange>,
    /// List of code changes for this account.
    pub code_changes: Vec<CodeChange>,
}

impl AccountChanges {
    /// Creates a new `AccountChanges` instance for the given address.
    pub fn new(address: Address) -> Self {
        Self {
            address,
            storage_changes: Vec::with_capacity(MAX_SLOTS),
            storage_reads: Vec::with_capacity(MAX_SLOTS),
            balance_changes: Vec::with_capacity(MAX_TXS),
            nonce_changes: Vec::with_capacity(MAX_TXS),
            code_changes: Vec::with_capacity(MAX_TXS),
        }
    }

    /// Returns the address of the account.
    #[inline]
    pub const fn address(&self) -> Address {
        self.address
    }

    /// Returns the storage changes for this account.
    #[inline]
    pub fn storage_changes(&self) -> &[StorageChange] {
        &self.storage_changes
    }

    /// Returns the storage reads for this account.
    #[inline]
    pub fn storage_reads(&self) -> &[StorageKey] {
        &self.storage_reads
    }

    /// Returns the balance changes for this account.
    #[inline]
    pub fn balance_changes(&self) -> &[BalanceChange] {
        &self.balance_changes
    }

    /// Returns the nonce changes for this account.
    #[inline]
    pub fn nonce_changes(&self) -> &[NonceChange] {
        &self.nonce_changes
    }

    /// Returns the code changes for this account.
    #[inline]
    pub fn code_changes(&self) -> &[CodeChange] {
        &self.code_changes
    }
}
