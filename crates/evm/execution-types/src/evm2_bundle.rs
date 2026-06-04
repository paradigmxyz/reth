//! Evm2 state aggregation types.

use alloc::{collections::BTreeMap, vec::Vec};
use alloy_primitives::{
    map::{AddressMap, B256Map},
    Address, BlockNumber, B256, U256,
};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, StateChanges, StorageChangeSet, Tracked},
};
use reth_primitives_traits::{Account, Bytecode as RethBytecode};
use reth_trie_common::{HashedPostState, HashedStorage, KeyHasher};

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

    /// Creates a bundle from Reth account, storage, revert, and bytecode initialization data.
    pub fn new_init(
        first_block: BlockNumber,
        accounts: impl IntoIterator<
            Item = (Address, (Option<Account>, Option<Account>, BTreeMap<U256, (U256, U256)>)),
        >,
        block_reverts: impl IntoIterator<Item = Evm2BlockReverts>,
        contracts: impl IntoIterator<Item = (B256, RethBytecode)>,
    ) -> Self {
        let mut bundle = Self::new(first_block);
        bundle.accounts = accounts
            .into_iter()
            .map(|(address, (original, current, storage))| {
                for (slot, (original, current)) in storage {
                    bundle
                        .storage
                        .entry(address)
                        .or_default()
                        .slots
                        .insert(slot, Tracked { original, current, _non_exhaustive: () });
                }
                (
                    address,
                    Tracked {
                        original: original.map(account_to_info),
                        current: current.map(account_to_info),
                        _non_exhaustive: (),
                    },
                )
            })
            .collect();
        bundle.block_reverts = block_reverts.into_iter().collect();
        bundle.contracts = contracts
            .into_iter()
            .map(|(hash, bytecode)| (hash, Bytecode::new_raw(bytecode.original_bytes())))
            .collect();
        bundle
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

    /// Return bytecode if known.
    pub fn bytecode(&self, code_hash: &B256) -> Option<RethBytecode> {
        self.contracts
            .get(code_hash)
            .map(|bytecode| RethBytecode::new_raw(bytecode.original_bytes()))
    }

    /// Get account if account is known.
    pub fn account(&self, address: &Address) -> Option<Option<Account>> {
        self.accounts.get(address).map(|account| account.current.as_ref().map(account_info_to_reth))
    }

    /// Get storage if value is known.
    pub fn storage_slot(&self, address: &Address, storage_key: U256) -> Option<U256> {
        self.storage.get(address)?.slots.get(&storage_key).map(|slot| slot.current)
    }

    /// Returns the hashed post-state represented by this bundle.
    pub fn hashed_post_state<KH: KeyHasher>(&self) -> HashedPostState {
        let mut hashed_state = HashedPostState::with_capacity(self.accounts.len());

        for (address, account) in &self.accounts {
            hashed_state
                .accounts
                .insert(KH::hash_key(address), account.current.as_ref().map(account_info_to_reth));
        }

        for (address, storage) in &self.storage {
            let hashed_storage = HashedStorage::from_iter(
                storage.wipe,
                storage.slots.iter().map(|(slot, value)| {
                    (KH::hash_key(B256::new(slot.to_be_bytes())), value.current)
                }),
            );

            if !hashed_storage.is_empty() {
                hashed_state.storages.insert(KH::hash_key(address), hashed_storage);
            }
        }

        hashed_state
    }

    /// Return iterator over all accounts.
    pub fn accounts_iter(&self) -> impl Iterator<Item = (Address, Option<&AccountInfo>)> {
        self.accounts.iter().map(|(address, account)| (*address, account.current.as_ref()))
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

    /// Extends this bundle with another bundle built on top of it.
    pub fn extend(&mut self, other: Self) {
        self.accounts.extend(other.accounts);
        self.storage.extend(other.storage);
        self.contracts.extend(other.contracts);
        self.block_reverts.extend(other.block_reverts);
    }

    /// Drops the first `n` block revert entries.
    pub fn drop_first_reverts(&mut self, n: usize) {
        self.block_reverts.drain(..n.min(self.block_reverts.len()));
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
            let block_revert =
                block_reverts.storage.entry(address).or_insert_with(|| Evm2StorageReverts {
                    previous_wipe: self
                        .storage
                        .get(&address)
                        .map(|storage| storage.wipe)
                        .unwrap_or_default(),
                    ..Default::default()
                });
            let bundle_storage = self.storage.entry(address).or_default();
            block_revert.wiped |= change_set.wipe;
            bundle_storage.wipe |= change_set.wipe;

            for (key, change) in change_set.slots {
                block_revert.slots.entry(key).or_insert(change.original);
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
                storage.wipe = storage_reverts.previous_wipe;

                for (key, original) in storage_reverts.slots {
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
    /// Original storage before the block changed it.
    pub storage: AddressMap<Evm2StorageReverts>,
}

/// Storage reverts for one account in one block.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evm2StorageReverts {
    /// Whether the block wiped the account storage.
    pub wiped: bool,
    /// Whether earlier bundle state had already marked this account storage as wiped.
    pub previous_wipe: bool,
    /// Original storage slots before the block changed them.
    pub slots: BTreeMap<U256, U256>,
}

impl From<StorageChangeSet> for Evm2StorageChangeSet {
    fn from(value: StorageChangeSet) -> Self {
        Self { wipe: value.wipe, slots: value.slots }
    }
}

fn account_info_to_reth(info: &AccountInfo) -> Account {
    Account {
        nonce: info.nonce,
        balance: info.balance,
        bytecode_hash: (!info.code_hash.is_zero()).then_some(info.code_hash),
    }
}

pub(crate) fn account_to_info(account: Account) -> AccountInfo {
    AccountInfo {
        balance: account.balance,
        nonce: account.nonce,
        code_hash: account.get_bytecode_hash(),
        code: None,
        _non_exhaustive: (),
    }
}

#[cfg(feature = "serde")]
mod serde_impl {
    use super::*;
    use alloy_primitives::Bytes;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct BundleStateSerde {
        accounts: AddressMap<TrackedSerde<Option<AccountInfoSerde>>>,
        storage: AddressMap<StorageChangeSetSerde>,
        contracts: B256Map<Bytes>,
        block_reverts: Vec<BlockRevertsSerde>,
        first_block: BlockNumber,
    }

    #[derive(Serialize, Deserialize)]
    struct StorageChangeSetSerde {
        wipe: bool,
        slots: BTreeMap<U256, TrackedSerde<U256>>,
    }

    #[derive(Serialize, Deserialize)]
    struct BlockRevertsSerde {
        accounts: AddressMap<Option<AccountInfoSerde>>,
        storage: AddressMap<StorageRevertsSerde>,
    }

    #[derive(Serialize, Deserialize)]
    struct StorageRevertsSerde {
        wiped: bool,
        previous_wipe: bool,
        slots: BTreeMap<U256, U256>,
    }

    #[derive(Serialize, Deserialize)]
    struct TrackedSerde<T> {
        original: T,
        current: T,
    }

    #[derive(Serialize, Deserialize)]
    struct AccountInfoSerde {
        balance: U256,
        nonce: u64,
        code_hash: B256,
        code: Option<Bytes>,
    }

    impl Serialize for Evm2BundleState {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            BundleStateSerde::from(self).serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Evm2BundleState {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            BundleStateSerde::deserialize(deserializer).map(Into::into)
        }
    }

    impl Serialize for Evm2StorageChangeSet {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            StorageChangeSetSerde::from(self).serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Evm2StorageChangeSet {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            StorageChangeSetSerde::deserialize(deserializer).map(Into::into)
        }
    }

    impl Serialize for Evm2BlockReverts {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            BlockRevertsSerde::from(self).serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Evm2BlockReverts {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            BlockRevertsSerde::deserialize(deserializer).map(Into::into)
        }
    }

    impl From<&Evm2BundleState> for BundleStateSerde {
        fn from(value: &Evm2BundleState) -> Self {
            Self {
                accounts: value
                    .accounts
                    .iter()
                    .map(|(address, account)| (*address, tracked_account_to_serde(account)))
                    .collect(),
                storage: value
                    .storage
                    .iter()
                    .map(|(address, storage)| (*address, StorageChangeSetSerde::from(storage)))
                    .collect(),
                contracts: value
                    .contracts
                    .iter()
                    .map(|(hash, bytecode)| (*hash, bytecode.original_bytes()))
                    .collect(),
                block_reverts: value.block_reverts.iter().map(BlockRevertsSerde::from).collect(),
                first_block: value.first_block,
            }
        }
    }

    impl From<BundleStateSerde> for Evm2BundleState {
        fn from(value: BundleStateSerde) -> Self {
            Self {
                accounts: value
                    .accounts
                    .into_iter()
                    .map(|(address, account)| (address, tracked_account_from_serde(account)))
                    .collect(),
                storage: value
                    .storage
                    .into_iter()
                    .map(|(address, storage)| (address, storage.into()))
                    .collect(),
                contracts: value
                    .contracts
                    .into_iter()
                    .map(|(hash, bytecode)| (hash, Bytecode::new_raw(bytecode)))
                    .collect(),
                block_reverts: value.block_reverts.into_iter().map(Into::into).collect(),
                first_block: value.first_block,
            }
        }
    }

    impl From<&Evm2StorageChangeSet> for StorageChangeSetSerde {
        fn from(value: &Evm2StorageChangeSet) -> Self {
            Self {
                wipe: value.wipe,
                slots: value
                    .slots
                    .iter()
                    .map(|(key, slot)| (*key, TrackedSerde::from(slot)))
                    .collect(),
            }
        }
    }

    impl From<StorageChangeSetSerde> for Evm2StorageChangeSet {
        fn from(value: StorageChangeSetSerde) -> Self {
            Self {
                wipe: value.wipe,
                slots: value.slots.into_iter().map(|(key, slot)| (key, slot.into())).collect(),
            }
        }
    }

    impl From<&Evm2BlockReverts> for BlockRevertsSerde {
        fn from(value: &Evm2BlockReverts) -> Self {
            Self {
                accounts: value
                    .accounts
                    .iter()
                    .map(|(address, account)| {
                        (*address, account.as_ref().map(AccountInfoSerde::from))
                    })
                    .collect(),
                storage: value
                    .storage
                    .iter()
                    .map(|(address, storage)| (*address, StorageRevertsSerde::from(storage)))
                    .collect(),
            }
        }
    }

    impl From<BlockRevertsSerde> for Evm2BlockReverts {
        fn from(value: BlockRevertsSerde) -> Self {
            Self {
                accounts: value
                    .accounts
                    .into_iter()
                    .map(|(address, account)| (address, account.map(Into::into)))
                    .collect(),
                storage: value
                    .storage
                    .into_iter()
                    .map(|(address, storage)| (address, storage.into()))
                    .collect(),
            }
        }
    }

    impl From<&Evm2StorageReverts> for StorageRevertsSerde {
        fn from(value: &Evm2StorageReverts) -> Self {
            Self {
                wiped: value.wiped,
                previous_wipe: value.previous_wipe,
                slots: value.slots.clone(),
            }
        }
    }

    impl From<StorageRevertsSerde> for Evm2StorageReverts {
        fn from(value: StorageRevertsSerde) -> Self {
            Self { wiped: value.wiped, previous_wipe: value.previous_wipe, slots: value.slots }
        }
    }

    impl<T: Clone> From<&Tracked<T>> for TrackedSerde<T> {
        fn from(value: &Tracked<T>) -> Self {
            Self { original: value.original.clone(), current: value.current.clone() }
        }
    }

    impl<T> From<TrackedSerde<T>> for Tracked<T> {
        fn from(value: TrackedSerde<T>) -> Self {
            Self { original: value.original, current: value.current, _non_exhaustive: () }
        }
    }

    fn tracked_account_to_serde(
        value: &Tracked<Option<AccountInfo>>,
    ) -> TrackedSerde<Option<AccountInfoSerde>> {
        TrackedSerde {
            original: value.original.as_ref().map(AccountInfoSerde::from),
            current: value.current.as_ref().map(AccountInfoSerde::from),
        }
    }

    fn tracked_account_from_serde(
        value: TrackedSerde<Option<AccountInfoSerde>>,
    ) -> Tracked<Option<AccountInfo>> {
        Tracked {
            original: value.original.map(Into::into),
            current: value.current.map(Into::into),
            _non_exhaustive: (),
        }
    }

    impl From<&AccountInfo> for AccountInfoSerde {
        fn from(value: &AccountInfo) -> Self {
            Self {
                balance: value.balance,
                nonce: value.nonce,
                code_hash: value.code_hash,
                code: value.code.as_ref().map(Bytecode::original_bytes),
            }
        }
    }

    impl From<AccountInfoSerde> for AccountInfo {
        fn from(value: AccountInfoSerde) -> Self {
            Self {
                balance: value.balance,
                nonce: value.nonce,
                code_hash: value.code_hash,
                code: value.code.map(Bytecode::new_raw),
                _non_exhaustive: (),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};
    use reth_trie_common::KeccakKeyHasher;

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
            bundle.block_reverts()[0].storage.get(&address).unwrap().slots.get(&U256::from(1)),
            Some(&U256::ZERO)
        );
        assert!(bundle.block_reverts()[0].storage.get(&address).unwrap().wiped);
    }

    #[test]
    fn block_reverts_preserve_previous_storage_wipe_state() {
        let address = address!("0000000000000000000000000000000000000001");
        let slot = U256::from(1);
        let mut first_block = StateChanges::default();
        first_block.storage.insert(
            address,
            StorageChangeSet {
                wipe: true,
                slots: BTreeMap::from([(
                    slot,
                    Tracked { original: U256::ZERO, current: U256::from(2), _non_exhaustive: () },
                )]),
                _non_exhaustive: (),
            },
        );

        let mut second_block = StateChanges::default();
        second_block.storage.insert(
            address,
            StorageChangeSet {
                wipe: true,
                slots: BTreeMap::from([(
                    slot,
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
        bundle.append_block([first_block]);
        bundle.append_block([second_block]);

        let second_block_reverts = bundle.block_reverts()[1].storage.get(&address).unwrap();
        assert!(second_block_reverts.wiped);
        assert!(second_block_reverts.previous_wipe);

        bundle.revert_blocks(1);
        let storage = bundle.storage().get(&address).unwrap();
        assert!(storage.wipe);
        assert_eq!(storage.slots.get(&slot).unwrap().current, U256::from(2));

        bundle.revert_blocks(1);
        let storage = bundle.storage().get(&address).unwrap();
        assert!(!storage.wipe);
        assert_eq!(storage.slots.get(&slot).unwrap().current, U256::ZERO);
    }

    #[test]
    fn hashed_post_state_hashes_accounts_and_storage() {
        let address = address!("0000000000000000000000000000000000000001");
        let slot = U256::from(1);
        let value = U256::from(2);
        let mut bundle = Evm2BundleState::new(1);

        let mut tx = StateChanges::default();
        tx.accounts.insert(
            address,
            Tracked { original: None, current: Some(account(1)), _non_exhaustive: () },
        );
        tx.storage.insert(
            address,
            StorageChangeSet {
                wipe: true,
                slots: BTreeMap::from([(
                    slot,
                    Tracked { original: U256::ZERO, current: value, _non_exhaustive: () },
                )]),
                _non_exhaustive: (),
            },
        );
        bundle.append_block([tx]);

        let hashed = bundle.hashed_post_state::<KeccakKeyHasher>();
        let hashed_address = KeccakKeyHasher::hash_key(address);
        let hashed_slot = KeccakKeyHasher::hash_key(B256::new(slot.to_be_bytes()));

        assert_eq!(
            hashed.accounts.get(&hashed_address),
            Some(&Some(account_info_to_reth(&account(1))))
        );
        let storage = hashed.storages.get(&hashed_address).unwrap();
        assert!(storage.wiped);
        assert_eq!(storage.storage.get(&hashed_slot), Some(&value));
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
