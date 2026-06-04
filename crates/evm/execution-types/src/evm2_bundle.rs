//! Evm2 state aggregation types.

use alloc::{collections::BTreeMap, vec::Vec};
use alloy_primitives::{
    map::{AddressMap, B256Map},
    BlockNumber, U256,
};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, StateChanges, StorageChangeSet, Tracked},
};

/// Bundle state built from evm2 per-transaction state changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evm2BundleState {
    accounts: AddressMap<Tracked<Option<AccountInfo>>>,
    storage: AddressMap<Evm2StorageChangeSet>,
    contracts: B256Map<Bytecode>,
    block_reverts: Vec<Evm2BlockReverts>,
    first_block: BlockNumber,
}

impl Evm2BundleState {
    /// Creates an empty bundle state beginning at `first_block`.
    pub fn new(first_block: BlockNumber) -> Self {
        Self { first_block, ..Default::default() }
    }

    /// Returns the first block covered by this bundle.
    pub const fn first_block(&self) -> BlockNumber {
        self.first_block
    }

    /// Returns the account changes in this bundle.
    pub const fn accounts(&self) -> &AddressMap<Tracked<Option<AccountInfo>>> {
        &self.accounts
    }

    /// Returns the storage changes in this bundle.
    pub const fn storage(&self) -> &AddressMap<Evm2StorageChangeSet> {
        &self.storage
    }

    /// Returns the bytecodes changed in this bundle.
    pub const fn contracts(&self) -> &B256Map<Bytecode> {
        &self.contracts
    }

    /// Returns the per-block reverts captured by this bundle.
    pub const fn block_reverts(&self) -> &Vec<Evm2BlockReverts> {
        &self.block_reverts
    }

    /// Appends a block worth of evm2 transaction state changes.
    pub fn append_block(&mut self, txs: impl IntoIterator<Item = StateChanges>) {
        let mut block_reverts = Evm2BlockReverts::default();
        for tx in txs {
            self.append_transaction(tx, &mut block_reverts);
        }
        self.block_reverts.push(block_reverts);
    }

    /// Removes state and revert data for the last `n` blocks.
    ///
    /// This only rewinds changes recorded in this bundle. Callers remain responsible for
    /// truncating receipts and other block-scoped execution outputs.
    pub fn revert_blocks(&mut self, n: usize) {
        for reverts in self
            .block_reverts
            .split_off(self.block_reverts.len().saturating_sub(n))
            .into_iter()
            .rev()
        {
            self.apply_reverts(reverts);
        }
    }

    fn append_transaction(&mut self, changes: StateChanges, block_reverts: &mut Evm2BlockReverts) {
        let StateChanges { accounts, storage, code, logs: _, _non_exhaustive: () } = changes;

        for (address, change) in accounts {
            block_reverts.accounts.entry(address).or_insert_with(|| change.original.clone());
            self.accounts
                .entry(address)
                .and_modify(|existing| existing.current = change.current.clone())
                .or_insert(change);
        }

        for (address, change_set) in storage {
            let block_revert = block_reverts.storage.entry(address).or_default();
            let bundle_storage = self.storage.entry(address).or_default();
            bundle_storage.wipe |= change_set.wipe;

            for (key, change) in change_set.slots {
                block_revert.entry(key).or_insert(change.original);
                bundle_storage
                    .slots
                    .entry(key)
                    .and_modify(|existing| existing.current = change.current)
                    .or_insert(change);
            }
        }

        self.contracts.extend(code);
    }

    fn apply_reverts(&mut self, reverts: Evm2BlockReverts) {
        for (address, original) in reverts.accounts {
            match self.accounts.get_mut(&address) {
                Some(account) => account.current = original,
                None => {
                    self.accounts.insert(
                        address,
                        Tracked {
                            original: original.clone(),
                            current: original,
                            _non_exhaustive: (),
                        },
                    );
                }
            }
        }

        for (address, storage_reverts) in reverts.storage {
            if let Some(storage) = self.storage.get_mut(&address) {
                for (key, original) in storage_reverts {
                    match storage.slots.get_mut(&key) {
                        Some(slot) => slot.current = original,
                        None => {
                            storage.slots.insert(
                                key,
                                Tracked { original, current: original, _non_exhaustive: () },
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Storage changes for one account aggregated across evm2 transactions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evm2StorageChangeSet {
    /// Whether all pre-existing account storage must be cleared before applying changed slots.
    pub wipe: bool,
    /// Changed slots keyed by EVM storage slot index.
    pub slots: BTreeMap<U256, Tracked<U256>>,
}

/// Reverts for one block of evm2 state changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evm2BlockReverts {
    /// Original accounts before the block changed them.
    pub accounts: AddressMap<Option<AccountInfo>>,
    /// Original storage slots before the block changed them.
    pub storage: AddressMap<BTreeMap<U256, U256>>,
}

impl From<StorageChangeSet> for Evm2StorageChangeSet {
    fn from(value: StorageChangeSet) -> Self {
        Self { wipe: value.wipe, slots: value.slots }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256, B256};

    #[test]
    fn bundle_keeps_first_original_and_latest_current() {
        let address = address!("0000000000000000000000000000000000000001");
        let mut first_tx = StateChanges::default();
        first_tx.accounts.insert(
            address,
            Tracked { original: Some(account(1)), current: Some(account(2)), _non_exhaustive: () },
        );

        let mut second_tx = StateChanges::default();
        second_tx.accounts.insert(
            address,
            Tracked { original: Some(account(2)), current: Some(account(3)), _non_exhaustive: () },
        );
        second_tx.code.insert(
            b256!("0x2222222222222222222222222222222222222222222222222222222222222222"),
            Bytecode::new_raw([0x00].as_slice().into()),
        );

        let mut bundle = Evm2BundleState::new(10);
        bundle.append_block([first_tx, second_tx]);

        let account_change = bundle.accounts().get(&address).unwrap();
        assert_eq!(account_change.original, Some(account(1)));
        assert_eq!(account_change.current, Some(account(3)));
        assert_eq!(bundle.block_reverts()[0].accounts.get(&address), Some(&Some(account(1))));
        assert_eq!(bundle.contracts().len(), 1);

        bundle.revert_blocks(1);
        assert_eq!(bundle.accounts().get(&address).unwrap().current, Some(account(1)));
    }

    #[test]
    fn bundle_merges_storage_changes() {
        let address = address!("0000000000000000000000000000000000000001");
        let mut first_tx = StateChanges::default();
        first_tx.storage.insert(
            address,
            StorageChangeSet {
                wipe: false,
                slots: BTreeMap::from([(
                    U256::from(1),
                    Tracked { original: U256::ZERO, current: U256::from(2), _non_exhaustive: () },
                )]),
                _non_exhaustive: (),
            },
        );

        let mut second_tx = StateChanges::default();
        second_tx.storage.insert(
            address,
            StorageChangeSet {
                wipe: true,
                slots: BTreeMap::from([(
                    U256::from(1),
                    Tracked {
                        original: U256::from(2),
                        current: U256::from(3),
                        _non_exhaustive: (),
                    },
                )]),
                _non_exhaustive: (),
            },
        );

        let mut bundle = Evm2BundleState::new(1);
        bundle.append_block([first_tx, second_tx]);

        let storage = bundle.storage().get(&address).unwrap();
        assert!(storage.wipe);
        assert_eq!(storage.slots.get(&U256::from(1)).unwrap().original, U256::ZERO);
        assert_eq!(storage.slots.get(&U256::from(1)).unwrap().current, U256::from(3));
        assert_eq!(
            bundle.block_reverts()[0].storage.get(&address).unwrap().get(&U256::from(1)),
            Some(&U256::ZERO)
        );
    }

    fn account(nonce: u64) -> AccountInfo {
        AccountInfo {
            nonce,
            balance: U256::from(nonce),
            code_hash: B256::ZERO,
            code: None,
            _non_exhaustive: (),
        }
    }
}
