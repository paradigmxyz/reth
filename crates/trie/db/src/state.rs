//! Database implementation for extracting state-root from in-memory hashed state.

use crate::{proof::ProofDb, trie::StateRootDb, TxRefWrapper};
use reth_db_api::transaction::DbTx;
use reth_execution_errors::StateRootError;
use reth_primitives::{Address, B256};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof::Proof, updates::TrieUpdates,
    HashedPostState, StateRoot,
};
use reth_trie_common::AccountProof;

/// Calculate the state root for this [`HashedPostState`].
/// Internally, this method retrieves prefixsets and uses them
/// to calculate incremental state root.
///
/// # Example
///
/// ```
/// use reth_db::test_utils::create_test_rw_db;
/// use reth_db_api::database::Database;
/// use reth_primitives::{Account, U256};
/// use reth_trie::HashedPostState;
/// use reth_trie_db::state::state_root;
///
/// // Initialize the database
/// let db = create_test_rw_db();
///
/// // Initialize hashed post state
/// let mut hashed_state = HashedPostState::default();
/// hashed_state.accounts.insert(
///     [0x11; 32].into(),
///     Some(Account { nonce: 1, balance: U256::from(10), bytecode_hash: None }),
/// );
///
/// // Calculate the state root
/// let tx = db.tx().expect("failed to create transaction");
/// let state_root = state_root(hashed_state, &tx);
/// ```
///
/// # Returns
///
/// The state root for this [`HashedPostState`].
pub fn state_root<TX: DbTx>(state: HashedPostState, tx: &TX) -> Result<B256, StateRootError> {
    let prefix_sets = state.construct_prefix_sets().freeze();
    let sorted = state.into_sorted();
    StateRoot::from_tx(tx)
        .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
            TxRefWrapper::from(tx),
            &sorted,
        ))
        .with_prefix_sets(prefix_sets)
        .root()
}

/// Calculates the state root for this [`HashedPostState`] and returns it alongside trie
/// updates. See [`state_root`] for more info.
pub fn state_root_with_updates<TX: DbTx>(
    state: HashedPostState,
    tx: &TX,
) -> Result<(B256, TrieUpdates), StateRootError> {
    let prefix_sets = state.construct_prefix_sets().freeze();
    let sorted = state.into_sorted();
    StateRoot::from_tx(tx)
        .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
            TxRefWrapper::from(tx),
            &sorted,
        ))
        .with_prefix_sets(prefix_sets)
        .root_with_updates()
}

/// Generates the state proof for target account and slots on top of this [`HashedPostState`].
pub fn account_proof<TX: DbTx>(
    state: HashedPostState,
    tx: &TX,
    address: Address,
    slots: &[B256],
) -> Result<AccountProof, StateRootError> {
    let prefix_sets = state.construct_prefix_sets();
    let sorted = state.into_sorted();
    Proof::from_tx(tx)
        .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
            TxRefWrapper::from(tx),
            &sorted,
        ))
        .with_prefix_sets_mut(prefix_sets)
        .account_proof(address, slots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db::test_utils::create_test_rw_db;
    use reth_db_api::database::Database;
    use reth_primitives::{hex, Address, B256, U256};
    use reth_trie::{HashedPostState, HashedStorage};
    use revm::{db::states::BundleState, primitives::AccountInfo};
    use std::collections::HashMap;

    #[test]
    fn hashed_state_wiped_extension() {
        let hashed_address = B256::default();
        let hashed_slot = B256::with_last_byte(64);
        let hashed_slot2 = B256::with_last_byte(65);

        // Initialize post state storage
        let original_slot_value = U256::from(123);
        let mut hashed_state = HashedPostState::default().with_storages([(
            hashed_address,
            HashedStorage::from_iter(
                false,
                [(hashed_slot, original_slot_value), (hashed_slot2, original_slot_value)],
            ),
        )]);

        // Update single slot value
        let updated_slot_value = U256::from(321);
        let extension = HashedPostState::default().with_storages([(
            hashed_address,
            HashedStorage::from_iter(false, [(hashed_slot, updated_slot_value)]),
        )]);
        hashed_state.extend(extension);

        let account_storage = hashed_state.storages.get(&hashed_address);
        assert_eq!(
            account_storage.and_then(|st| st.storage.get(&hashed_slot)),
            Some(&updated_slot_value)
        );
        assert_eq!(
            account_storage.and_then(|st| st.storage.get(&hashed_slot2)),
            Some(&original_slot_value)
        );
        assert_eq!(account_storage.map(|st| st.wiped), Some(false));

        // Wipe account storage
        let wiped_extension =
            HashedPostState::default().with_storages([(hashed_address, HashedStorage::new(true))]);
        hashed_state.extend(wiped_extension);

        let account_storage = hashed_state.storages.get(&hashed_address);
        assert_eq!(account_storage.map(|st| st.storage.is_empty()), Some(true));
        assert_eq!(account_storage.map(|st| st.wiped), Some(true));

        // Reinitialize single slot value
        hashed_state.extend(HashedPostState::default().with_storages([(
            hashed_address,
            HashedStorage::from_iter(false, [(hashed_slot, original_slot_value)]),
        )]));
        let account_storage = hashed_state.storages.get(&hashed_address);
        assert_eq!(
            account_storage.and_then(|st| st.storage.get(&hashed_slot)),
            Some(&original_slot_value)
        );
        assert_eq!(account_storage.and_then(|st| st.storage.get(&hashed_slot2)), None);
        assert_eq!(account_storage.map(|st| st.wiped), Some(true));

        // Reinitialize single slot value
        hashed_state.extend(HashedPostState::default().with_storages([(
            hashed_address,
            HashedStorage::from_iter(false, [(hashed_slot2, updated_slot_value)]),
        )]));
        let account_storage = hashed_state.storages.get(&hashed_address);
        assert_eq!(
            account_storage.and_then(|st| st.storage.get(&hashed_slot)),
            Some(&original_slot_value)
        );
        assert_eq!(
            account_storage.and_then(|st| st.storage.get(&hashed_slot2)),
            Some(&updated_slot_value)
        );
        assert_eq!(account_storage.map(|st| st.wiped), Some(true));
    }

    #[test]
    fn from_bundle_state_with_rayon() {
        let address1 = Address::with_last_byte(1);
        let address2 = Address::with_last_byte(2);
        let slot1 = U256::from(1015);
        let slot2 = U256::from(2015);

        let account1 = AccountInfo { nonce: 1, ..Default::default() };
        let account2 = AccountInfo { nonce: 2, ..Default::default() };

        let bundle_state = BundleState::builder(2..=2)
            .state_present_account_info(address1, account1)
            .state_present_account_info(address2, account2)
            .state_storage(address1, HashMap::from([(slot1, (U256::ZERO, U256::from(10)))]))
            .state_storage(address2, HashMap::from([(slot2, (U256::ZERO, U256::from(20)))]))
            .build();
        assert_eq!(bundle_state.reverts.len(), 1);

        let post_state = HashedPostState::from_bundle_state(&bundle_state.state);
        assert_eq!(post_state.accounts.len(), 2);
        assert_eq!(post_state.storages.len(), 2);

        let db = create_test_rw_db();
        let tx = db.tx().expect("failed to create transaction");
        assert_eq!(
            state_root(post_state, &tx).unwrap(),
            hex!("b464525710cafcf5d4044ac85b72c08b1e76231b8d91f288fe438cc41d8eaafd")
        );
    }
}
