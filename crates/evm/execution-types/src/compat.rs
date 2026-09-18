//! State conversion at the evm2 execution boundary.

use alloy_primitives::map::AddressSet;
use revm::{
    database::{
        states::{bundle_state::BundleRetention, TransitionAccount, TransitionState},
        AccountStatus, BundleState,
    },
    state::{AccountInfo, Bytecode},
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
            let original = changes.originals.get(address).expect("state has account metadata");
            let previous_status = self
                .transitions
                .transitions
                .get(address)
                .map_or_else(|| loaded_status(original.as_ref()), |account| account.status);
            let destroyed = account.is_selfdestructed();
            let wiped = changes.wiped.contains(address);
            let status = if destroyed {
                previous_status.on_selfdestructed()
            } else if wiped && !account.is_created() {
                AccountStatus::DestroyedChanged
            } else if account.is_created() {
                previous_status.on_created()
            } else {
                previous_status.on_changed(original.as_ref().is_none_or(|i| {
                    i.nonce == 0 && i.code_hash == alloy_primitives::KECCAK256_EMPTY
                }))
            };
            self.transitions.add_transition(
                *address,
                TransitionAccount {
                    info: (!destroyed).then(|| account.info.clone()),
                    status,
                    previous_info: original.clone(),
                    previous_status,
                    storage: Some(alloc::borrow::Cow::Borrowed(&account.storage)),
                    storage_was_destroyed: wiped,
                },
            );
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
    originals: alloy_primitives::map::AddressMap<Option<AccountInfo>>,
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
        self.state.entry(address).or_default().storage.clear();
        Ok(())
    }
    fn storage(&mut self, change: evm2::evm::StorageChange) -> Result<(), Self::Error> {
        self.state.entry(change.address).or_default().storage.insert(
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
        let original = change.original.map(revm_account);
        let account = self.state.entry(change.address).or_default();
        *account.original_info_mut() = original.clone().unwrap_or_default();
        self.originals.insert(change.address, original);
        account.info = change.current.map(revm_account).unwrap_or_default();
        if let Some(code) = self.contracts.get(&account.info.code_hash) {
            account.info.code = Some(code.clone());
        }
        account.mark_touch();
        if change.created {
            account.mark_created();
        }
        if change.current.is_none() {
            account.mark_selfdestruct();
        }
        Ok(())
    }
    fn account_read(
        &mut self,
        address: alloy_primitives::Address,
        info: Option<&evm2::evm::AccountInfo>,
    ) -> Result<(), Self::Error> {
        if self.state.contains_key(&address) {
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

/// Visits a persisted bundle as native changes when seeding an execution cache.
#[derive(Debug)]
pub struct BundleSource<'a>(pub &'a BundleState);

impl evm2::evm::StateChangeSource for BundleSource<'_> {
    fn visit<S: evm2::evm::StateChangeSink>(&self, sink: &mut S) -> Result<(), S::Error> {
        for (hash, code) in &self.0.contracts {
            sink.bytecode(*hash, &evm2::bytecode::Bytecode::new_raw(code.original_bytes()))?;
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
    evm2::evm::AccountInfo {
        balance: info.balance,
        nonce: info.nonce,
        code_hash: info.code_hash,
        code: info
            .code
            .as_ref()
            .map(|code| evm2::bytecode::Bytecode::new_raw(code.original_bytes())),
        _non_exhaustive: (),
    }
}

/// Converts native bytecode into the persistent state representation.
pub fn revm_bytecode(code: &evm2::bytecode::Bytecode) -> Bytecode {
    Bytecode::new_raw(code.original_bytes())
}

/// Converts native account information into the persistent state representation.
/// Empty bytecode is omitted because its presence depends on database cache state.
pub fn revm_account(info: &evm2::evm::AccountInfo) -> AccountInfo {
    AccountInfo {
        balance: info.balance,
        nonce: info.nonce,
        code_hash: info.code_hash,
        code: info.code.as_ref().filter(|code| !code.is_empty()).map(revm_bytecode),
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
    fn cached_empty_bytecode_does_not_change_bundle_output() {
        let address = Address::with_last_byte(1);
        let info = NativeAccount { balance: U256::from(5), ..Default::default() };
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
        let cached = bundle(&NativeAccount { code: Some(Default::default()), ..info });
        assert_eq!(uncached, cached);
        assert!(cached.contracts.is_empty());
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
