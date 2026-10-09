//! BAL builder module

#[cfg(feature = "account-ext")]
use crate::AccountExtension;
use crate::{
    bal::{writes::BalWrites, BalError, BlockAccessIndex},
    Account, AccountInfo, EvmStorage,
};
use alloy_eip7928::{
    AccountChanges as AlloyAccountChanges, BalanceChange as AlloyBalanceChange,
    CodeChange as AlloyCodeChange, NonceChange as AlloyNonceChange,
    SlotChanges as AlloySlotChanges, StorageChange as AlloyStorageChange,
};
use bytecode::{Bytecode, BytecodeDecodeError};
use core::ops::{Deref, DerefMut};
use primitives::{Address, StorageKey, StorageValue, B256, U256};
use std::{
    collections::{btree_map::Entry, BTreeMap},
    vec::Vec,
};

/// Account BAL structure.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccountBal {
    /// Account info bal.
    pub account_info: AccountInfoBal,
    /// Storage bal.
    pub storage: StorageBal,
}

impl Deref for AccountBal {
    type Target = AccountInfoBal;

    fn deref(&self) -> &Self::Target {
        &self.account_info
    }
}

impl DerefMut for AccountBal {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.account_info
    }
}

impl AccountBal {
    /// Populate account from BAL. Return true if account info got changed
    pub fn populate_account_info(
        &self,
        bal_index: BlockAccessIndex,
        account: &mut AccountInfo,
    ) -> bool {
        self.account_info.populate_account_info(bal_index, account)
    }

    /// Extend account from another account.
    #[inline]
    pub fn update(&mut self, bal_index: BlockAccessIndex, account: &Account) {
        if account.is_selfdestructed_locally() {
            let empty_info = AccountInfo::default();
            self.account_info
                .update(bal_index, &account.original_info(), &empty_info);
            // Selfdestruct wipes all storage to zero, record writes accordingly.
            self.storage
                .update_selfdestruct(bal_index, &account.storage);
            return;
        }

        self.account_info
            .update(bal_index, &account.original_info(), &account.info);

        self.storage.update(bal_index, &account.storage);
    }

    /// Create an account BAL from EIP-7928 [`AlloyAccountChanges`].
    ///
    /// # Errors
    ///
    /// Returns [`BytecodeDecodeError`] if any code change contains bytecode rejected by
    /// [`Bytecode::new_raw_checked`]. This currently happens for malformed EIP-7702
    /// bytecode, such as bytes with the EIP-7702 magic prefix but an invalid length or
    /// unsupported version.
    #[inline]
    pub fn try_from_alloy(
        alloy_account: AlloyAccountChanges,
    ) -> Result<(Address, Self), BytecodeDecodeError> {
        Ok((
            alloy_account.address,
            AccountBal {
                account_info: AccountInfoBal {
                    nonce: BalWrites::from(alloy_account.nonce_changes),
                    balance: BalWrites::from(alloy_account.balance_changes),
                    code: BalWrites::try_from(alloy_account.code_changes)?,
                    #[cfg(feature = "account-ext")]
                    extension: BalWrites::default(),
                },
                storage: StorageBal::from_iter(
                    alloy_account
                        .storage_changes
                        .into_iter()
                        .chain(
                            alloy_account
                                .storage_reads
                                .into_iter()
                                .map(|key| AlloySlotChanges::new(key, Default::default())),
                        )
                        .map(|slot| (slot.slot, BalWrites::from(slot.changes))),
                ),
            },
        ))
    }

    /// Clone an account BAL from EIP-7928 [`AlloyAccountChanges`] without consuming the source.
    ///
    /// # Errors
    ///
    /// Returns [`BytecodeDecodeError`] if any code change contains bytecode rejected by
    /// [`Bytecode::new_raw_checked`]. This currently happens for malformed EIP-7702
    /// bytecode, such as bytes with the EIP-7702 magic prefix but an invalid length or
    /// unsupported version.
    #[inline]
    pub fn clone_from_alloy(
        alloy_account: &AlloyAccountChanges,
    ) -> Result<(Address, Self), BytecodeDecodeError> {
        Ok((
            alloy_account.address,
            AccountBal {
                account_info: AccountInfoBal {
                    nonce: BalWrites::from(alloy_account.nonce_changes.as_slice()),
                    balance: BalWrites::from(alloy_account.balance_changes.as_slice()),
                    code: BalWrites::try_from(alloy_account.code_changes.as_slice())?,
                    #[cfg(feature = "account-ext")]
                    extension: BalWrites::default(),
                },
                storage: StorageBal::from_iter(
                    alloy_account
                        .storage_changes
                        .iter()
                        .map(|slot| (slot.slot, BalWrites::from(slot.changes.as_slice())))
                        .chain(
                            alloy_account
                                .storage_reads
                                .iter()
                                .map(|key| (*key, BalWrites::default())),
                        ),
                ),
            },
        ))
    }

    /// Consumes `AccountBal` and converts it into canonical EIP-7928
    /// [`AlloyAccountChanges`].
    ///
    /// The returned account changes are ordered deterministically: storage reads
    /// and storage changes are sorted lexicographically by slot key, changes
    /// within each storage slot are sorted by block access index, and balance,
    /// nonce, and code changes are sorted by block access index.
    ///
    /// This matches the EIP-7928 ordering requirements:
    /// <https://eips.ethereum.org/EIPS/eip-7928#ordering-uniqueness-and-determinism>.
    #[inline]
    pub fn into_alloy_account(self, address: Address) -> AlloyAccountChanges {
        let storage_len = self.storage.storage.len();
        let mut storage_reads = Vec::with_capacity(storage_len);
        let mut storage_changes = Vec::with_capacity(storage_len);
        for (key, value) in self.storage.storage {
            if value.writes.is_empty() {
                storage_reads.push(key);
            } else {
                let mut changes = value
                    .writes
                    .into_iter()
                    .map(|(index, value)| AlloyStorageChange::new(index, value))
                    .collect::<Vec<_>>();
                changes.sort_unstable_by_key(|change| change.block_access_index);

                storage_changes.push(AlloySlotChanges::new(key, changes));
            }
        }

        let mut balance_changes = self
            .account_info
            .balance
            .writes
            .into_iter()
            .map(|(index, value)| AlloyBalanceChange::new(index, value))
            .collect::<Vec<_>>();
        balance_changes.sort_unstable_by_key(|change| change.block_access_index);

        let mut nonce_changes = self
            .account_info
            .nonce
            .writes
            .into_iter()
            .map(|(index, value)| AlloyNonceChange::new(index, value))
            .collect::<Vec<_>>();
        nonce_changes.sort_unstable_by_key(|change| change.block_access_index);

        let mut code_changes = self
            .account_info
            .code
            .writes
            .into_iter()
            .map(|(index, (_, value))| AlloyCodeChange::new(index, value.original_bytes()))
            .collect::<Vec<_>>();
        code_changes.sort_unstable_by_key(|change| change.block_access_index);

        AlloyAccountChanges {
            address,
            storage_changes,
            storage_reads,
            balance_changes,
            nonce_changes,
            code_changes,
        }
    }
}

/// Account info bal structure.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccountInfoBal {
    /// Nonce builder.
    pub nonce: BalWrites<u64>,
    /// Balance builder.
    pub balance: BalWrites<U256>,
    /// Code builder.
    pub code: BalWrites<(B256, Bytecode)>,
    /// Chain-specific account extension builder.
    #[cfg(feature = "account-ext")]
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "BalWrites::is_empty")
    )]
    pub extension: BalWrites<AccountExtension>,
}

impl AccountInfoBal {
    /// Populate account info from BAL. Return true if account info got changed
    pub fn populate_account_info(
        &self,
        bal_index: BlockAccessIndex,
        account: &mut AccountInfo,
    ) -> bool {
        let mut changed = false;
        if let Some(nonce) = self.nonce.get(bal_index) {
            account.nonce = nonce;
            changed = true;
        }
        if let Some(balance) = self.balance.get(bal_index) {
            account.balance = balance;
            changed = true;
        }
        if let Some(code) = self.code.get(bal_index) {
            account.code_hash = code.0;
            account.code = Some(code.1);
            changed = true;
        }
        #[cfg(feature = "account-ext")]
        if let Some(extension) = self.extension.get(bal_index) {
            account.extension = extension;
            changed = true;
        }
        changed
    }

    /// Extend account info from another account info.
    #[inline]
    pub fn update(
        &mut self,
        index: BlockAccessIndex,
        original: &AccountInfo,
        present: &AccountInfo,
    ) {
        self.nonce.update(index, &original.nonce, present.nonce);
        self.balance
            .update(index, &original.balance, present.balance);
        if original.code_hash != present.code_hash {
            self.code.update_with_key(
                index,
                &original.code_hash,
                (present.code_hash, present.code.clone().unwrap_or_default()),
                |i| &i.0,
            );
        }
        #[cfg(feature = "account-ext")]
        self.extension
            .update(index, &original.extension, present.extension.clone());
    }

    /// Extend account info from another account info.
    #[inline]
    pub fn extend(&mut self, bal_account: AccountInfoBal) {
        self.nonce.extend(bal_account.nonce);
        self.balance.extend(bal_account.balance);
        self.code.extend(bal_account.code);
        #[cfg(feature = "account-ext")]
        self.extension.extend(bal_account.extension);
    }

    /// Update account balance in BAL.
    #[inline]
    pub fn balance_update(
        &mut self,
        bal_index: BlockAccessIndex,
        original_balance: &U256,
        balance: U256,
    ) {
        self.balance.update(bal_index, original_balance, balance);
    }

    /// Update account nonce in BAL.
    #[inline]
    pub fn nonce_update(&mut self, bal_index: BlockAccessIndex, original_nonce: &u64, nonce: u64) {
        self.nonce.update(bal_index, original_nonce, nonce);
    }

    /// Update account code in BAL.
    #[inline]
    pub fn code_update(
        &mut self,
        bal_index: BlockAccessIndex,
        original_code_hash: &B256,
        code_hash: B256,
        code: Bytecode,
    ) {
        self.code
            .update_with_key(bal_index, original_code_hash, (code_hash, code), |i| &i.0);
    }
}

/// Storage BAL
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StorageBal {
    /// Storage with writes and reads.
    pub storage: BTreeMap<StorageKey, BalWrites<StorageValue>>,
}

impl StorageBal {
    /// Get storage from the builder.
    #[inline]
    pub fn get(
        &self,
        address: &Address,
        key: StorageKey,
        bal_index: BlockAccessIndex,
    ) -> Result<Option<StorageValue>, BalError> {
        Ok(self.get_bal_writes(address, key)?.get(bal_index))
    }

    /// Get storage writes from the builder.
    ///
    /// `address` is only needed in case of an error to propagate the address.
    #[inline]
    pub fn get_bal_writes(
        &self,
        address: &Address,
        key: StorageKey,
    ) -> Result<&BalWrites<StorageValue>, BalError> {
        self.storage.get(&key).ok_or(BalError::SlotNotFound {
            address: *address,
            slot: key,
        })
    }

    /// Extend storage from another storage.
    #[inline]
    pub fn extend(&mut self, storage: StorageBal) {
        for (key, value) in storage.storage {
            match self.storage.entry(key) {
                Entry::Occupied(mut entry) => {
                    entry.get_mut().extend(value);
                }
                Entry::Vacant(entry) => {
                    entry.insert(value);
                }
            }
        }
    }

    /// Update storage from [`EvmStorage`].
    #[inline]
    pub fn update(&mut self, bal_index: BlockAccessIndex, storage: &EvmStorage) {
        for (key, value) in storage {
            self.storage.entry(*key).or_default().update(
                bal_index,
                &value.original_value,
                value.present_value,
            );
        }
    }

    /// Update storage for a selfdestructed account.
    ///
    /// All accessed slots are recorded as written to zero since selfdestruct wipes storage.
    #[inline]
    pub fn update_selfdestruct(&mut self, bal_index: BlockAccessIndex, storage: &EvmStorage) {
        for (key, value) in storage {
            self.storage.entry(*key).or_default().update(
                bal_index,
                &value.original_value,
                StorageValue::ZERO,
            );
        }
    }

    /// Update reads from [`EvmStorage`].
    ///
    /// It will expend inner map with new reads.
    #[inline]
    pub fn update_reads(&mut self, storage: impl Iterator<Item = StorageKey>) {
        for key in storage {
            self.storage.entry(key).or_default();
        }
    }

    /// Insert storage into the builder.
    pub fn extend_iter(
        &mut self,
        storage: impl Iterator<Item = (StorageKey, BalWrites<StorageValue>)>,
    ) {
        for (key, value) in storage {
            self.storage.insert(key, value);
        }
    }

    /// Convert the storage into a vector of reads and writes
    pub fn into_vecs(self) -> (Vec<StorageKey>, Vec<(StorageKey, BalWrites<StorageValue>)>) {
        let mut reads = Vec::new();
        let mut writes = Vec::new();

        for (key, value) in self.storage {
            if value.writes.is_empty() {
                reads.push(key);
            } else {
                writes.push((key, value));
            }
        }

        (reads, writes)
    }
}

impl FromIterator<(StorageKey, BalWrites<StorageValue>)> for StorageBal {
    fn from_iter<I: IntoIterator<Item = (StorageKey, BalWrites<StorageValue>)>>(iter: I) -> Self {
        Self {
            storage: iter.into_iter().collect(),
        }
    }
}
