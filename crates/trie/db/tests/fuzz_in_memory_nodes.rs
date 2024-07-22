use proptest::prelude::*;
use reth_db::{cursor::DbCursorRW, tables, transaction::DbTxMut};
use reth_primitives::{Account, B256, U256};
use reth_provider::test_utils::create_test_provider_factory;
use reth_trie::{
    prefix_set::{PrefixSetMut, TriePrefixSets},
    test_utils::state_root_prehashed,
    trie_cursor::InMemoryTrieCursorFactory,
    StateRoot,
};
use reth_trie_common::Nibbles;
use reth_trie_db::DatabaseStateRoot;
use std::collections::BTreeMap;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128, ..ProptestConfig::default()
    })]

    #[test]
    fn fuzz_in_memory_nodes(mut init_state: BTreeMap<B256, U256>, mut updated_state: BTreeMap<B256, U256>) {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut hashed_account_cursor = provider.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();

        // Insert init state into database
        for (hashed_address, balance) in init_state.clone() {
            hashed_account_cursor.upsert(hashed_address, Account { balance, ..Default::default() }).unwrap();
        }

        // Compute initial root and updates
        let (_, trie_updates) = StateRoot::from_tx(provider.tx_ref())
        .root_with_updates()
        .unwrap();

        // Insert state updates into database
        let mut changes = PrefixSetMut::default();
        for (hashed_address, balance) in updated_state.clone() {
            hashed_account_cursor.upsert(hashed_address, Account { balance, ..Default::default() }).unwrap();
            changes.insert(Nibbles::unpack(hashed_address));
        }

        // Compute root with in-memory trie nodes overlay
        let (state_root, _) = StateRoot::from_tx(provider.tx_ref())
            .with_prefix_sets(TriePrefixSets { account_prefix_set: changes.freeze(), ..Default::default() })
            .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(provider.tx_ref(), &trie_updates.into_sorted()))
            .root_with_updates()
            .unwrap();

        // Verify the result
        let mut state = BTreeMap::default();
        state.append(&mut init_state);
        state.append(&mut updated_state);
        let expected_root = state_root_prehashed(
            state.iter().map(|(&key, &balance)| (key, (Account { balance, ..Default::default() }, std::iter::empty())))
        );
        assert_eq!(expected_root, state_root);
    }
}
