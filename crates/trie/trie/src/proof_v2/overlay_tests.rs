use super::*;
use crate::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    test_utils::TrieTestHarness,
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
};
use alloy_primitives::map::B256Map;
use proptest::prelude::*;
use reth_trie_common::{
    prefix_set::PrefixSetMut, updates::TrieUpdates, HashedPostStateSorted, HashedStorage,
    ProofV2TargetParent,
};
use std::collections::BTreeMap;

fn slot(key: u16) -> B256 {
    // A small alphabet produces shared paths and cached branches in small test cases.
    B256::right_padding_from(&(key & 0x3333).to_be_bytes())
}

#[test]
fn insertions_before_and_after_cached_root_extension() {
    let entry = |prefix: [u8; 2]| (B256::right_padding_from(&prefix), U256::from(1));
    let initial = [[0xe1, 0xd0], [0xe1, 0xd1], [0xe1, 0xe0]]
        .map(entry)
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let changes = [[0x00, 0x00], [0xe0, 0x00], [0xe0, 0x10], [0xea, 0x00], [0xf0, 0x00]].map(entry);
    let expected_root = crate::test_utils::storage_root_prehashed(
        initial.iter().map(|(&key, &value)| (key, value)).chain(changes),
    );
    let harness = TrieTestHarness::new(initial);
    let address = harness.hashed_address();
    let mut prefixes = PrefixSetMut::default();
    for (key, _) in changes {
        prefixes.insert(Nibbles::unpack(key));
    }
    let overlay = HashedPostStateSorted::new(
        Vec::new(),
        B256Map::from_iter([(address, HashedStorage::from_iter(changes).into_sorted())]),
    );
    let hashed_factory =
        HashedPostStateCursorFactory::new(harness.hashed_cursor_factory(), &overlay);
    let mut calculator = StorageProofCalculator::new_storage(
        harness.trie_cursor_factory().storage_trie_cursor(address).unwrap(),
        hashed_factory.hashed_storage_cursor(address).unwrap(),
    )
    .with_prefix_set(prefixes.freeze());

    // The e0 branch is built before the cached e1 branch arrives. Its absolute path is needed
    // when both branches become children of the new branch at e.
    let proof = calculator
        .storage_proof(address, &mut [ProofV2Target::new(B256::repeat_byte(0x40))])
        .unwrap();
    assert_eq!(calculator.compute_root_hash(&proof).unwrap(), Some(expected_root));
    let root = calculator.storage_root_node(address).unwrap();
    assert_eq!(calculator.compute_root_hash(&[root]).unwrap(), Some(expected_root));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn proofs_match_rebuilt_state_with_overlays(
        initial in prop::collection::vec((any::<u16>(), 1u64..1000), 0..60),
        changes in prop::collection::vec((any::<u16>(), 0u64..4), 0..40),
        target in any::<u16>(),
        parent_depth in 0usize..64,
        all_changed in any::<bool>(),
    ) {
        let initial = initial.into_iter()
            .map(|(key, value)| (slot(key), U256::from(value)))
            .collect::<BTreeMap<_, _>>();
        let harness = TrieTestHarness::new(initial.clone());
        let address = harness.hashed_address();
        let changes = changes.into_iter()
            .map(|(key, value)| (slot(key), U256::from(value)))
            .collect::<Vec<_>>();
        let intermediate = changes[..changes.len() / 2].iter().copied().collect();
        let (_, branch_updates) = harness.get_root_with_updates(&intermediate);
        let trie_updates = TrieUpdates {
            storage_tries: B256Map::from_iter([(address, branch_updates)]),
            ..Default::default()
        }.into_sorted();
        let trie_factory = InMemoryTrieCursorFactory::new(harness.trie_cursor_factory(), &trie_updates);

        let mut current = initial;
        let mut prefixes = PrefixSetMut::default();
        for &(key, value) in &changes {
            prefixes.insert(Nibbles::unpack(key));
            if value.is_zero() {
                current.remove(&key);
            } else {
                current.insert(key, value);
            }
        }
        let overlay = HashedPostStateSorted::new(
            Vec::new(),
            B256Map::from_iter([(address, HashedStorage::from_iter(changes).into_sorted())]),
        );
        let hashed_factory = HashedPostStateCursorFactory::new(harness.hashed_cursor_factory(), &overlay);
        let mut calculator = StorageProofCalculator::new_storage(
            trie_factory.storage_trie_cursor(address).unwrap(),
            hashed_factory.hashed_storage_cursor(address).unwrap(),
        ).with_prefix_set(if all_changed { PrefixSet::all_paths() } else { prefixes.freeze() });

        let fresh = TrieTestHarness::new(current);
        let key = slot(target);
        let existing = fresh.storage().keys().next().copied().unwrap_or(key);
        let mut targets = [
            ProofV2Target::new(key),
            ProofV2Target::new(key).with_parent(ProofV2TargetParent::new(parent_depth)),
            ProofV2Target::new(existing).with_parent(ProofV2TargetParent::new(0)),
        ];
        let actual = calculator.storage_proof(address, &mut targets).unwrap();
        prop_assert_eq!(calculator.compute_root_hash(&actual).unwrap(), Some(fresh.original_root()));
        let (expected, _) = fresh.proof_v2(&mut targets);

        // Compare paths and node contents because masks describe the chosen backing store.
        let nodes = |proof: Vec<ProofTrieNodeV2>| {
            proof.into_iter().map(|node| (node.path, node.node)).collect::<Vec<_>>()
        };
        prop_assert_eq!(nodes(actual), nodes(expected));

        let mut partial = targets.into_iter()
            .filter(|target| target.parent.is_known())
            .collect::<Vec<_>>();
        let actual = calculator.storage_proof(address, &mut partial).unwrap();
        let (expected, _) = fresh.proof_v2(&mut partial);
        prop_assert_eq!(nodes(actual), nodes(expected));
    }
}
