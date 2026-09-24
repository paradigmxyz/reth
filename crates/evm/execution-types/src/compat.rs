//! State conversion at the evm2 execution boundary.

use alloc::vec::Vec;
use alloy_primitives::{
    map::{hash_map::Entry, AddressMap, AddressSet},
    Address, U256,
};
use revm::{
    database::{
        states::{bundle_state::BundleRetention, StorageSlot, TransitionAccount, TransitionState},
        AccountStatus, BundleState,
    },
    state::{Account, AccountInfo, Bytecode},
};

/// Transaction transitions retained using revm's bundle state model.
#[derive(Debug, Default)]
pub struct BlockState {
    transitions: TransitionState,
    contracts: alloy_primitives::map::B256Map<Bytecode>,
}

impl BlockState {
    /// Creates an empty block accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a converted transaction to the block's transitions.
    pub fn commit(&mut self, changes: &TransactionChanges) {
        self.contracts.extend(changes.contracts.iter().map(|(hash, code)| (*hash, code.clone())));
        for (address, account) in &changes.state {
            self.commit_account(
                *address,
                (!account.is_loaded_as_not_existing()).then(|| account.original_info()),
                (!account.is_selfdestructed()).then(|| account.info.clone()),
                account.is_created(),
                changes.wiped.contains(address),
                account.storage.iter().filter_map(|(key, slot)| {
                    slot.is_changed().then_some((
                        *key,
                        StorageSlot::new_changed(slot.original_value, slot.present_value),
                    ))
                }),
            );
        }
    }

    /// Streams one finalized native transaction into the block without materializing EVM state.
    /// Storage callbacks must precede the corresponding account callback, as in evm2 transaction
    /// streams. Use a fresh sink for each transaction.
    pub fn transaction_sink(
        &mut self,
    ) -> impl evm2::evm::StateChangeSink<Error = core::convert::Infallible> + '_ {
        BlockStateSink { block: self, storage: AddressMap::default() }
    }

    fn commit_account(
        &mut self,
        address: Address,
        original: Option<AccountInfo>,
        current: Option<AccountInfo>,
        created: bool,
        wiped: bool,
        storage: impl Iterator<Item = (U256, StorageSlot)>,
    ) {
        let entry = self.transitions.transitions.entry(address);
        let previous_status = match &entry {
            Entry::Occupied(entry) => entry.get().status,
            Entry::Vacant(_) => loaded_status(original.as_ref()),
        };
        let status =
            if current.is_none() {
                previous_status.on_selfdestructed()
            } else if wiped && !created {
                AccountStatus::DestroyedChanged
            } else if created {
                previous_status.on_created()
            } else {
                previous_status.on_changed(original.as_ref().is_none_or(|i| {
                    i.nonce == 0 && i.code_hash == alloy_primitives::KECCAK256_EMPTY
                }))
            };
        match entry {
            Entry::Vacant(entry) => {
                entry.insert(TransitionAccount {
                    info: current,
                    status,
                    previous_info: original,
                    previous_status,
                    storage: storage.collect(),
                    storage_was_destroyed: wiped,
                });
            }
            Entry::Occupied(mut entry) => {
                let account = entry.get_mut();
                account.info = current;
                account.status = status;
                // Match TransitionAccount::update: deletion starts a new storage lifetime.
                if matches!(status, AccountStatus::Destroyed | AccountStatus::DestroyedAgain) {
                    account.storage.clear();
                    account.storage.extend(storage);
                    account.storage_was_destroyed = true;
                } else {
                    for (key, slot) in storage {
                        match account.storage.entry(key) {
                            Entry::Vacant(entry) => {
                                entry.insert(slot);
                            }
                            Entry::Occupied(mut entry) => {
                                if entry.get().original_value() == slot.present_value() {
                                    entry.remove();
                                } else {
                                    entry.get_mut().present_value = slot.present_value();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Finalizes one block's transitions and reverts.
    pub fn into_bundle(self) -> BundleState {
        let mut bundle = BundleState::default();
        bundle.apply_transitions_and_create_reverts(self.transitions, BundleRetention::Reverts);
        bundle.contracts.extend(self.contracts);
        bundle
    }
}

/// Materialized transaction changes in revm's EVM state representation.
#[derive(Debug, Default)]
pub struct TransactionChanges {
    /// State passed to the block accumulator and state-root hooks.
    pub state: revm::state::EvmState,
    wiped: AddressSet,
    contracts: alloy_primitives::map::B256Map<Bytecode>,
}

impl evm2::evm::StateChangeSink for TransactionChanges {
    type Error = core::convert::Infallible;
    fn bytecode(
        &mut self,
        hash: alloy_primitives::B256,
        code: &evm2::bytecode::Bytecode,
    ) -> Result<(), Self::Error> {
        self.contracts.entry(hash).or_insert_with(|| revm_bytecode(code));
        Ok(())
    }
    fn storage_wipe(&mut self, address: alloy_primitives::Address) -> Result<(), Self::Error> {
        self.wiped.insert(address);
        self.state.entry(address).or_insert_with(empty_account).storage.clear();
        Ok(())
    }
    fn storage(&mut self, change: evm2::evm::StorageChange) -> Result<(), Self::Error> {
        self.state.entry(change.address).or_insert_with(empty_account).storage.insert(
            change.key,
            revm::state::EvmStorageSlot::new_changed(
                change.original,
                change.current,
                revm::state::TransactionId::ZERO,
            ),
        );
        Ok(())
    }
    fn account(&mut self, change: evm2::evm::AccountChangeRef<'_>) -> Result<(), Self::Error> {
        let account = self.state.entry(change.address).or_insert_with(empty_account);
        update_account(account, change, &self.contracts);
        Ok(())
    }
    fn account_read(
        &mut self,
        address: alloy_primitives::Address,
        info: Option<&evm2::evm::AccountInfo>,
    ) -> Result<(), Self::Error> {
        if let Some(account) = self.state.get_mut(&address) {
            update_account(
                account,
                evm2::evm::AccountChangeRef {
                    address,
                    original: info,
                    current: info,
                    created: false,
                    selfdestructed: false,
                },
                &self.contracts,
            );
        }
        Ok(())
    }
}

/// Storage precedes account metadata in native transaction streams. Buffer slots contiguously
/// until the account's lifecycle is known, avoiding temporary per-account storage hash maps.
struct BlockStateSink<'a> {
    block: &'a mut BlockState,
    storage: AddressMap<(bool, Vec<(U256, StorageSlot)>)>,
}

impl evm2::evm::StateChangeSink for BlockStateSink<'_> {
    type Error = core::convert::Infallible;

    fn bytecode(
        &mut self,
        hash: alloy_primitives::B256,
        code: &evm2::bytecode::Bytecode,
    ) -> Result<(), Self::Error> {
        self.block.contracts.entry(hash).or_insert_with(|| revm_bytecode(code));
        Ok(())
    }

    fn storage_wipe(&mut self, address: Address) -> Result<(), Self::Error> {
        let (wiped, slots) = self.storage.entry(address).or_default();
        *wiped = true;
        slots.clear();
        Ok(())
    }

    fn storage(&mut self, change: evm2::evm::StorageChange) -> Result<(), Self::Error> {
        let (_, slots) = self.storage.entry(change.address).or_default();
        if change.original != change.current {
            slots.push((change.key, StorageSlot::new_changed(change.original, change.current)));
        }
        Ok(())
    }

    fn account(&mut self, change: evm2::evm::AccountChangeRef<'_>) -> Result<(), Self::Error> {
        let (wiped, slots) = self.storage.remove(&change.address).unwrap_or_default();
        let mut current = change.current.map(revm_account);
        if let Some(info) = &mut current {
            info.code = self.block.contracts.get(&info.code_hash).cloned();
        }
        self.block.commit_account(
            change.address,
            change.original.map(revm_account),
            current,
            change.created,
            wiped,
            slots.into_iter(),
        );
        Ok(())
    }

    fn account_read(
        &mut self,
        address: Address,
        info: Option<&evm2::evm::AccountInfo>,
    ) -> Result<(), Self::Error> {
        if self.storage.contains_key(&address) {
            self.account(evm2::evm::AccountChangeRef {
                address,
                original: info,
                current: info,
                created: false,
                selfdestructed: false,
            })?;
        }
        Ok(())
    }
}

fn empty_account() -> Account {
    // Account::from initializes the original-info box directly, without constructing and
    // immediately dropping the default empty bytecode in both account-info values.
    Account::from(AccountInfo {
        balance: alloy_primitives::U256::ZERO,
        nonce: 0,
        code_hash: alloy_primitives::KECCAK256_EMPTY,
        code: None,
        account_id: None,
    })
}

fn update_account(
    account: &mut Account,
    change: evm2::evm::AccountChangeRef<'_>,
    contracts: &alloy_primitives::map::B256Map<Bytecode>,
) {
    let original = change.original.map(revm_account);
    account.status.set(revm::state::AccountStatus::LoadedAsNotExisting, original.is_none());
    *account.original_info_mut() = original.unwrap_or_default();
    account.info = change.current.map(revm_account).unwrap_or_default();
    if let Some(code) = contracts.get(&account.info.code_hash) {
        account.info.code = Some(code.clone());
    }
    account.mark_touch();
    if change.created {
        account.mark_created();
    }
    if change.current.is_none() {
        account.mark_selfdestruct();
    }
}

/// Visits a persisted bundle as native changes when seeding an execution cache.
#[derive(Debug)]
pub struct BundleSource<'a>(pub &'a BundleState);

impl evm2::evm::StateChangeSource for BundleSource<'_> {
    fn visit<S: evm2::evm::StateChangeSink>(&self, sink: &mut S) -> Result<(), S::Error> {
        for (hash, code) in &self.0.contracts {
            sink.bytecode(*hash, &native_bytecode(code))?;
        }
        for (address, account) in &self.0.state {
            let original = account.original_info.as_ref().map(native_account);
            let current = account.info.as_ref().map(native_account);
            if account.status.was_destroyed() {
                sink.storage_wipe(*address)?;
            }
            sink.account(evm2::evm::AccountChangeRef {
                address: *address,
                original: original.as_ref(),
                current: current.as_ref(),
                created: account.status.is_storage_known() && current.is_some(),
                selfdestructed: current.is_none(),
            })?;
            for (key, value) in &account.storage {
                sink.storage(evm2::evm::StorageChange {
                    address: *address,
                    key: *key,
                    original: value.original_value(),
                    current: value.present_value(),
                })?;
            }
        }
        Ok(())
    }
}

/// Converts persistent account information into evm2's execution representation.
pub fn native_account(info: &AccountInfo) -> evm2::evm::AccountInfo {
    evm2::evm::AccountInfo::new(
        info.balance,
        info.nonce,
        info.code_hash,
        info.code.as_ref().map(native_bytecode),
    )
}

/// Converts persistent bytecode while retaining its analyzed jump destinations and padding.
pub fn native_bytecode(code: &Bytecode) -> evm2::bytecode::Bytecode {
    if code.is_empty() {
        return evm2::bytecode::Bytecode::default()
    }
    if let Some(jumps) = code.legacy_jump_table() {
        let jumps = evm2::bytecode::JumpTable::from_slice(jumps.as_slice(), code.len());
        // SAFETY: revm and evm2 use the same legacy analysis and padding rules for PUSH and
        // DUPN/SWAPN/EXCHANGE. The bytecode and jump map come from an already analyzed value;
        // retaining its padded bytes preserves the interpreter's bounds invariants.
        unsafe { evm2::bytecode::Bytecode::new_analyzed(code.bytes(), code.len(), jumps) }
    } else {
        evm2::bytecode::Bytecode::new_raw(code.original_bytes())
    }
}

/// Converts native bytecode into the persistent state representation.
pub fn revm_bytecode(code: &evm2::bytecode::Bytecode) -> Bytecode {
    Bytecode::new_raw(code.original_bytes())
}

/// Converts native account information into the persistent state representation.
/// Bytecode is carried separately by the change stream; account updates only need its hash.
pub const fn revm_account(info: &evm2::evm::AccountInfo) -> AccountInfo {
    AccountInfo {
        balance: info.balance,
        nonce: info.nonce,
        code_hash: info.code_hash,
        code: None,
        account_id: None,
    }
}

fn loaded_status(info: Option<&AccountInfo>) -> AccountStatus {
    match info {
        None => AccountStatus::LoadedNotExisting,
        Some(info) if info.is_empty() => AccountStatus::LoadedEmptyEIP161,
        Some(_) => AccountStatus::Loaded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};
    use evm2::evm::{
        AccountChangeRef, AccountInfo as NativeAccount, StateChangeSink, StorageChange,
    };

    #[test]
    fn native_sink_preserves_storage_lifetimes_and_reverts() {
        let address = Address::with_last_byte(1);
        let code = evm2::bytecode::Bytecode::new_raw(alloy_primitives::bytes!("60015b00"));
        let info = NativeAccount::default().with_nonce(1).with_code(code.clone());
        let mut converted = BlockState::new();
        let mut native = BlockState::new();
        // Storage-only writes, restoration, deletion, recreation, a surviving storage wipe,
        // and another deletion exercise both occupied and vacant transition entries.
        for step in 0..7 {
            let visit = |sink: &mut dyn StateChangeSink<Error = core::convert::Infallible>| {
                if step == 3 {
                    sink.bytecode(code.hash_slow(), &code).unwrap();
                }
                if step >= 2 {
                    sink.storage_wipe(address).unwrap();
                }
                if step < 2 || step == 3 || step == 4 {
                    sink.storage(StorageChange {
                        address,
                        key: U256::from(3),
                        original: U256::from(if step == 1 { 8 } else { 7 }),
                        current: U256::from(if step == 1 { 7 } else { 8 }),
                    })
                    .unwrap();
                }
                if step < 2 {
                    sink.account_read(address, Some(&info)).unwrap();
                } else {
                    sink.account(AccountChangeRef {
                        address,
                        original: (step != 3 && step != 6).then_some(&info),
                        current: (step == 3 || step == 4).then_some(&info),
                        created: step == 3,
                        selfdestructed: step != 3,
                    })
                    .unwrap();
                }
                // Reads alone must never introduce an account transition.
                sink.account_read(Address::with_last_byte(2), Some(&info)).unwrap();
            };
            let mut changes = TransactionChanges::default();
            visit(&mut changes);
            converted.commit(&changes);
            visit(&mut native.transaction_sink());
            assert_eq!(native.transitions, converted.transitions, "step {step}");
            assert_eq!(native.contracts, converted.contracts);
        }
        let mut native = native.into_bundle();
        let mut converted = converted.into_bundle();
        assert_eq!(native, converted);
        assert!(native.revert_latest());
        assert!(converted.revert_latest());
        assert_eq!(native, converted);
    }

    #[test]
    fn bytecode_conversion_preserves_analysis_and_padding() {
        let mut cases = alloc::vec![
            alloc::vec![],
            alloc::vec![0x00],
            alloc::vec![0x5b, 0x60, 0x5b, 0x00],
            alloc::vec![0x5b; 24_576],
        ];
        for opcode in 0x60..=0x7f {
            // Every truncated PUSH width, including a JUMPDEST inside its immediate data.
            for available in 0..=usize::from(opcode - 0x60) {
                let mut bytes = alloc::vec![opcode];
                bytes.extend(core::iter::repeat_n(0x5b, available));
                cases.push(bytes);
            }
        }
        for opcode in [
            revm::bytecode::opcode::DUPN,
            revm::bytecode::opcode::SWAPN,
            revm::bytecode::opcode::EXCHANGE,
        ] {
            cases.push(alloc::vec![opcode]);
            cases.push(alloc::vec![opcode, 0]);
        }
        for bytes in cases {
            let persistent = Bytecode::new_raw(bytes.into());
            let native = native_bytecode(&persistent);
            let analyzed = evm2::bytecode::Bytecode::new_raw(persistent.original_bytes());
            assert_eq!(native, analyzed);
            assert_eq!(native.bytes(), analyzed.bytes());
            assert_eq!(native.legacy_jump_table(), analyzed.legacy_jump_table());
            if !persistent.is_empty() {
                assert_eq!(native.bytes().as_ptr(), persistent.bytes_ref().as_ptr());
            }
        }
        let delegated = Bytecode::new_eip7702(Address::with_last_byte(7));
        let native = native_bytecode(&delegated);
        assert_eq!(native.eip7702_address(), Some(Address::with_last_byte(7)));
        assert_eq!(native.original_bytes(), delegated.original_bytes());
    }

    #[test]
    fn cached_bytecode_does_not_change_bundle_output() {
        let address = Address::with_last_byte(1);
        let code = evm2::bytecode::Bytecode::new_raw(alloy_primitives::bytes!("60015b00"));
        let info = NativeAccount {
            balance: U256::from(5),
            code_hash: code.hash_slow(),
            ..Default::default()
        };
        let bundle = |info: &NativeAccount| {
            let mut changes = TransactionChanges::default();
            changes
                .account(AccountChangeRef {
                    address,
                    original: None,
                    current: Some(info),
                    created: false,
                    selfdestructed: false,
                })
                .unwrap();
            let mut block = BlockState::new();
            block.commit(&changes);
            block.into_bundle()
        };
        let uncached = bundle(&info);
        let cached = bundle(&NativeAccount { code: Some(code), ..info });
        assert_eq!(uncached, cached);
        assert!(cached.contracts.is_empty());
    }

    #[test]
    fn deployed_bytecode_is_retained_separately_from_account_metadata() {
        let address = Address::with_last_byte(1);
        let code = evm2::bytecode::Bytecode::new_raw(alloy_primitives::bytes!("60015b00"));
        let hash = code.hash_slow();
        let info = NativeAccount {
            nonce: 1,
            code_hash: hash,
            code: Some(code.clone()),
            ..Default::default()
        };
        let mut changes = TransactionChanges::default();
        changes.bytecode(hash, &code).unwrap();
        changes
            .account(AccountChangeRef {
                address,
                original: None,
                current: Some(&info),
                created: true,
                selfdestructed: false,
            })
            .unwrap();
        let mut block = BlockState::new();
        block.commit(&changes);
        let bundle = block.into_bundle();
        assert_eq!(bundle.contracts[&hash].original_bytes(), code.original_bytes());
        assert_eq!(bundle.account(&address).unwrap().info.as_ref().unwrap().code_hash, hash);
    }

    #[test]
    fn storage_only_change_retains_account_and_reverts() {
        let address = Address::with_last_byte(1);
        let info = NativeAccount { nonce: 1, ..Default::default() };
        let mut changes = TransactionChanges::default();
        changes
            .storage(StorageChange {
                address,
                key: U256::from(3),
                original: U256::from(7),
                current: U256::from(8),
            })
            .unwrap();
        changes.account_read(address, Some(&info)).unwrap();
        let mut block = BlockState::new();
        block.commit(&changes);
        let mut bundle = block.into_bundle();
        assert_eq!(bundle.account(&address).unwrap().info.as_ref().unwrap().nonce, 1);
        assert_eq!(bundle.storage(&address, U256::from(3)), Some(U256::from(8)));
        assert_eq!(bundle.reverts.len(), 1);
        assert!(bundle.revert_latest());
        assert_eq!(bundle.storage(&address, U256::from(3)), Some(U256::from(7)));
    }

    #[test]
    fn destroy_then_recreate_retains_parent_storage_wipe() {
        let address = Address::with_last_byte(1);
        let original = NativeAccount { nonce: 1, balance: U256::from(7), ..Default::default() };
        let recreated = NativeAccount { nonce: 1, balance: U256::from(9), ..Default::default() };
        let mut block = BlockState::new();
        let mut destroyed = TransactionChanges::default();
        destroyed
            .account(AccountChangeRef {
                address,
                original: Some(&original),
                current: None,
                created: false,
                selfdestructed: true,
            })
            .unwrap();
        block.commit(&destroyed);
        let mut created = TransactionChanges::default();
        created.storage_wipe(address).unwrap();
        created
            .storage(StorageChange {
                address,
                key: U256::from(3),
                original: U256::ZERO,
                current: U256::from(8),
            })
            .unwrap();
        created
            .account(AccountChangeRef {
                address,
                original: None,
                current: Some(&recreated),
                created: true,
                selfdestructed: false,
            })
            .unwrap();
        block.commit(&created);
        let bundle = block.into_bundle();
        let account = bundle.account(&address).unwrap();
        assert!(account.was_destroyed());
        assert_eq!(account.original_info.as_ref().unwrap().balance, U256::from(7));
        assert_eq!(account.info.as_ref().unwrap().balance, U256::from(9));
        assert_eq!(account.storage_slot(U256::from(4)), Some(U256::ZERO));
        assert_eq!(account.storage_slot(U256::from(3)), Some(U256::from(8)));
        assert!(bundle.reverts[0][0].1.wipe_storage);
    }
}
