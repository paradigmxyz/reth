use alloy_primitives::{Address, B256};
use derive_more::{Deref, DerefMut};
use revm::{
    database::State,
    primitives::{AddressMap, StorageKey, StorageValue},
    state::{Account, AccountId, AccountInfo, Bytecode},
    Database, DatabaseCommit,
};

/// Canonical BAL state that records and applies each account before visiting the next one.
#[derive(Debug, Deref, DerefMut)]
pub(super) struct CanonicalState<DB: Database>(State<DB>);

impl<DB: Database> CanonicalState<DB> {
    pub(super) fn new(database: DB) -> Self {
        Self(
            State::builder()
                .with_database(database)
                .with_bundle_update()
                .with_bal_builder()
                .build(),
        )
    }
}

impl<DB: Database> DatabaseCommit for CanonicalState<DB> {
    fn commit(&mut self, changes: AddressMap<Account>) {
        // The iterator path interleaves BAL recording with cache and transition updates instead
        // of traversing the entire transaction state once for BALs and again for state changes.
        self.0.commit_iter(&mut changes.into_iter());
    }

    fn commit_iter(&mut self, changes: &mut dyn Iterator<Item = (Address, Account)>) {
        self.0.commit_iter(changes);
    }
}

impl<DB: Database> Database for CanonicalState<DB> {
    type Error = <State<DB> as Database>::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.0.basic(address)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.0.code_by_hash(code_hash)
    }

    fn storage(
        &mut self,
        address: Address,
        index: StorageKey,
    ) -> Result<StorageValue, Self::Error> {
        self.0.storage(address, index)
    }

    fn storage_by_account_id(
        &mut self,
        address: Address,
        account_id: AccountId,
        storage_key: StorageKey,
    ) -> Result<StorageValue, Self::Error> {
        self.0.storage_by_account_id(address, account_id, storage_key)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.0.block_hash(number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use revm::{
        database::{states::bundle_state::BundleRetention, EmptyDB},
        state::EvmStorageSlot,
    };

    #[test]
    fn fused_commit_preserves_bal_and_bundle() {
        let mut fused = CanonicalState::new(EmptyDB::default());
        let mut regular = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .with_bal_builder()
            .build();

        for index in 1..=8 {
            let mut changes = AddressMap::default();
            for n in 1..=4 {
                let address = Address::repeat_byte(n);
                let mut account = Account::from(AccountInfo::default());
                account.mark_touch();
                account.info.balance = U256::from(index * u64::from(n));
                account.info.nonce = index;
                account.storage.insert(
                    U256::ZERO,
                    EvmStorageSlot::new_changed(
                        U256::from(index - 1),
                        U256::from(index),
                        Default::default(),
                    ),
                );
                account
                    .storage
                    .insert(U256::from(1), EvmStorageSlot::new(U256::from(7), Default::default()));
                changes.insert(address, account);
            }
            fused.bump_bal_index();
            regular.bump_bal_index();
            regular.commit(changes.clone());
            fused.commit(changes);
        }

        assert_eq!(fused.take_built_alloy_bal(), regular.take_built_alloy_bal());
        fused.merge_transitions(BundleRetention::Reverts);
        regular.merge_transitions(BundleRetention::Reverts);
        assert_eq!(fused.take_bundle(), regular.take_bundle());
    }
}
