//! Tests for `SparseStateTrie` with `ParallelSparseTrie`.

use crate::ParallelSparseTrie;
use alloy_primitives::{
    b256,
    map::{HashMap, HashSet},
    B256, U256,
};
use alloy_trie::proof::DecodedProofNodes;
use arbitrary::Arbitrary;
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_primitives_traits::Account;
use reth_trie::{updates::StorageTrieUpdates, HashBuilder, MultiProof, EMPTY_ROOT_HASH};
use reth_trie_common::{
    proof::{ProofNodes, ProofRetainer},
    BranchNode, BranchNodeMasks, BranchNodeMasksMap, LeafNode, Nibbles, ProofTrieNode, RlpNode,
    StorageMultiProof, TrieAccount, TrieMask, TrieNode,
};
use reth_trie_sparse::{
    filter_map_revealed_nodes, provider::DefaultTrieNodeProviderFactory, FilterMappedProofNodes,
    ProofNodesMetricValues, SparseStateTrie, SparseTrie,
};

type TestSparseStateTrie = SparseStateTrie<ParallelSparseTrie, ParallelSparseTrie>;

#[test]
fn reveal_account_path_twice() {
    let provider_factory = DefaultTrieNodeProviderFactory;
    let mut sparse = TestSparseStateTrie::default();

    let leaf_value = alloy_rlp::encode(TrieAccount::default());
    let leaf_1 =
        alloy_rlp::encode(TrieNode::Leaf(LeafNode::new(Nibbles::default(), leaf_value.clone())));
    let leaf_2 =
        alloy_rlp::encode(TrieNode::Leaf(LeafNode::new(Nibbles::default(), leaf_value.clone())));

    let multiproof = MultiProof {
        account_subtree: ProofNodes::from_iter([
            (
                Nibbles::default(),
                alloy_rlp::encode(TrieNode::Branch(BranchNode {
                    stack: vec![RlpNode::from_rlp(&leaf_1), RlpNode::from_rlp(&leaf_2)],
                    state_mask: TrieMask::new(0b11),
                }))
                .into(),
            ),
            (Nibbles::from_nibbles([0x0]), leaf_1.clone().into()),
            (Nibbles::from_nibbles([0x1]), leaf_1.clone().into()),
        ]),
        ..Default::default()
    };

    // Reveal multiproof and check that the state trie contains the leaf value.
    sparse.reveal_decoded_multiproof(multiproof.clone().try_into().unwrap()).unwrap();
    assert_eq!(
        sparse.state_trie_ref().unwrap().get_leaf_value(&Nibbles::from_nibbles([0x0])),
        Some(&leaf_value)
    );

    // Remove the leaf node and check that the state trie does not contain the leaf value.
    sparse.remove_account_leaf(&Nibbles::from_nibbles([0x0]), &provider_factory).unwrap();
    assert!(sparse
        .state_trie_ref()
        .unwrap()
        .get_leaf_value(&Nibbles::from_nibbles([0x0]))
        .is_none());

    // Reveal multiproof again and check that the state trie still does not contain the leaf
    // value, because it was already revealed before.
    sparse.reveal_decoded_multiproof(multiproof.try_into().unwrap()).unwrap();
    assert!(sparse
        .state_trie_ref()
        .unwrap()
        .get_leaf_value(&Nibbles::from_nibbles([0x0]))
        .is_none());
}

#[test]
fn reveal_storage_path_twice() {
    let provider_factory = DefaultTrieNodeProviderFactory;
    let mut sparse = TestSparseStateTrie::default();

    let leaf_value = alloy_rlp::encode(TrieAccount::default());
    let leaf_1 =
        alloy_rlp::encode(TrieNode::Leaf(LeafNode::new(Nibbles::default(), leaf_value.clone())));
    let leaf_2 =
        alloy_rlp::encode(TrieNode::Leaf(LeafNode::new(Nibbles::default(), leaf_value.clone())));

    let multiproof = MultiProof {
        storages: HashMap::from_iter([(
            B256::ZERO,
            StorageMultiProof {
                root: B256::ZERO,
                subtree: ProofNodes::from_iter([
                    (
                        Nibbles::default(),
                        alloy_rlp::encode(TrieNode::Branch(BranchNode {
                            stack: vec![RlpNode::from_rlp(&leaf_1), RlpNode::from_rlp(&leaf_2)],
                            state_mask: TrieMask::new(0b11),
                        }))
                        .into(),
                    ),
                    (Nibbles::from_nibbles([0x0]), leaf_1.clone().into()),
                    (Nibbles::from_nibbles([0x1]), leaf_1.clone().into()),
                ]),
                branch_node_masks: Default::default(),
            },
        )]),
        ..Default::default()
    };

    // Reveal multiproof and check that the storage trie contains the leaf value.
    sparse.reveal_decoded_multiproof(multiproof.clone().try_into().unwrap()).unwrap();
    assert_eq!(
        sparse.storage_trie_ref(&B256::ZERO).unwrap().get_leaf_value(&Nibbles::from_nibbles([0x0])),
        Some(&leaf_value)
    );

    // Remove the leaf node and check that the storage trie does not contain the leaf value.
    sparse
        .remove_storage_leaf(B256::ZERO, &Nibbles::from_nibbles([0x0]), &provider_factory)
        .unwrap();
    assert!(sparse
        .storage_trie_ref(&B256::ZERO)
        .unwrap()
        .get_leaf_value(&Nibbles::from_nibbles([0x0]))
        .is_none());

    // Reveal multiproof again and check that the storage trie still does not contain the leaf
    // value, because it was already revealed before.
    sparse.reveal_decoded_multiproof(multiproof.try_into().unwrap()).unwrap();
    assert!(sparse
        .storage_trie_ref(&B256::ZERO)
        .unwrap()
        .get_leaf_value(&Nibbles::from_nibbles([0x0]))
        .is_none());
}

#[test]
fn reveal_v2_proof_nodes() {
    let provider_factory = DefaultTrieNodeProviderFactory;
    let mut sparse = TestSparseStateTrie::default();

    let leaf_value = alloy_rlp::encode(TrieAccount::default());
    let leaf_1_node = TrieNode::Leaf(LeafNode::new(Nibbles::default(), leaf_value.clone()));
    let leaf_2_node = TrieNode::Leaf(LeafNode::new(Nibbles::default(), leaf_value.clone()));

    let branch_node = TrieNode::Branch(BranchNode {
        stack: vec![
            RlpNode::from_rlp(&alloy_rlp::encode(&leaf_1_node)),
            RlpNode::from_rlp(&alloy_rlp::encode(&leaf_2_node)),
        ],
        state_mask: TrieMask::new(0b11),
    });

    // Create V2 proof nodes with masks already included.
    let v2_proof_nodes = vec![
        ProofTrieNode {
            path: Nibbles::default(),
            node: branch_node,
            masks: Some(BranchNodeMasks {
                hash_mask: TrieMask::default(),
                tree_mask: TrieMask::default(),
            }),
        },
        ProofTrieNode { path: Nibbles::from_nibbles([0x0]), node: leaf_1_node, masks: None },
        ProofTrieNode { path: Nibbles::from_nibbles([0x1]), node: leaf_2_node, masks: None },
    ];

    // Reveal V2 proof nodes.
    sparse.reveal_account_v2_proof_nodes(v2_proof_nodes.clone()).unwrap();

    // Check that the state trie contains the leaf value.
    assert_eq!(
        sparse.state_trie_ref().unwrap().get_leaf_value(&Nibbles::from_nibbles([0x0])),
        Some(&leaf_value)
    );

    // Remove the leaf node.
    sparse.remove_account_leaf(&Nibbles::from_nibbles([0x0]), &provider_factory).unwrap();
    assert!(sparse
        .state_trie_ref()
        .unwrap()
        .get_leaf_value(&Nibbles::from_nibbles([0x0]))
        .is_none());

    // Reveal again - should skip already revealed paths.
    sparse.reveal_account_v2_proof_nodes(v2_proof_nodes).unwrap();
    assert!(sparse
        .state_trie_ref()
        .unwrap()
        .get_leaf_value(&Nibbles::from_nibbles([0x0]))
        .is_none());
}

#[test]
fn reveal_storage_v2_proof_nodes() {
    let provider_factory = DefaultTrieNodeProviderFactory;
    let mut sparse = TestSparseStateTrie::default();

    let storage_value: Vec<u8> = alloy_rlp::encode_fixed_size(&U256::from(42)).to_vec();
    let leaf_1_node = TrieNode::Leaf(LeafNode::new(Nibbles::default(), storage_value.clone()));
    let leaf_2_node = TrieNode::Leaf(LeafNode::new(Nibbles::default(), storage_value.clone()));

    let branch_node = TrieNode::Branch(BranchNode {
        stack: vec![
            RlpNode::from_rlp(&alloy_rlp::encode(&leaf_1_node)),
            RlpNode::from_rlp(&alloy_rlp::encode(&leaf_2_node)),
        ],
        state_mask: TrieMask::new(0b11),
    });

    let v2_proof_nodes = vec![
        ProofTrieNode { path: Nibbles::default(), node: branch_node, masks: None },
        ProofTrieNode { path: Nibbles::from_nibbles([0x0]), node: leaf_1_node, masks: None },
        ProofTrieNode { path: Nibbles::from_nibbles([0x1]), node: leaf_2_node, masks: None },
    ];

    // Reveal V2 storage proof nodes for account.
    sparse.reveal_storage_v2_proof_nodes(B256::ZERO, v2_proof_nodes.clone()).unwrap();

    // Check that the storage trie contains the leaf value.
    assert_eq!(
        sparse.storage_trie_ref(&B256::ZERO).unwrap().get_leaf_value(&Nibbles::from_nibbles([0x0])),
        Some(&storage_value)
    );

    // Remove the leaf node.
    sparse
        .remove_storage_leaf(B256::ZERO, &Nibbles::from_nibbles([0x0]), &provider_factory)
        .unwrap();
    assert!(sparse
        .storage_trie_ref(&B256::ZERO)
        .unwrap()
        .get_leaf_value(&Nibbles::from_nibbles([0x0]))
        .is_none());

    // Reveal again - should skip already revealed paths.
    sparse.reveal_storage_v2_proof_nodes(B256::ZERO, v2_proof_nodes).unwrap();
    assert!(sparse
        .storage_trie_ref(&B256::ZERO)
        .unwrap()
        .get_leaf_value(&Nibbles::from_nibbles([0x0]))
        .is_none());
}

#[test]
fn take_trie_updates() {
    reth_tracing::init_test_tracing();

    let mut rng = StdRng::seed_from_u64(1);

    let mut bytes = [0u8; 1024];
    rng.fill(bytes.as_mut_slice());

    let slot_1 = b256!("0x1000000000000000000000000000000000000000000000000000000000000000");
    let slot_path_1 = Nibbles::unpack(slot_1);
    let value_1 = U256::from(rng.random::<u64>());
    let slot_2 = b256!("0x1100000000000000000000000000000000000000000000000000000000000000");
    let slot_path_2 = Nibbles::unpack(slot_2);
    let value_2 = U256::from(rng.random::<u64>());
    let slot_3 = b256!("0x2000000000000000000000000000000000000000000000000000000000000000");
    let slot_path_3 = Nibbles::unpack(slot_3);
    let value_3 = U256::from(rng.random::<u64>());

    let mut storage_hash_builder = HashBuilder::default()
        .with_proof_retainer(ProofRetainer::from_iter([slot_path_1, slot_path_2]));
    storage_hash_builder.add_leaf(slot_path_1, &alloy_rlp::encode_fixed_size(&value_1));
    storage_hash_builder.add_leaf(slot_path_2, &alloy_rlp::encode_fixed_size(&value_2));

    let storage_root = storage_hash_builder.root();
    let storage_proof_nodes = storage_hash_builder.take_proof_nodes();
    let storage_branch_node_masks = BranchNodeMasksMap::from_iter([
        (
            Nibbles::default(),
            BranchNodeMasks { hash_mask: TrieMask::new(0b010), tree_mask: TrieMask::default() },
        ),
        (
            Nibbles::from_nibbles([0x1]),
            BranchNodeMasks { hash_mask: TrieMask::new(0b11), tree_mask: TrieMask::default() },
        ),
    ]);

    let address_1 = b256!("0x1000000000000000000000000000000000000000000000000000000000000000");
    let address_path_1 = Nibbles::unpack(address_1);
    let account_1 = Account::arbitrary(&mut arbitrary::Unstructured::new(&bytes)).unwrap();
    let mut trie_account_1 = account_1.into_trie_account(storage_root);
    let address_2 = b256!("0x1100000000000000000000000000000000000000000000000000000000000000");
    let address_path_2 = Nibbles::unpack(address_2);
    let account_2 = Account::arbitrary(&mut arbitrary::Unstructured::new(&bytes)).unwrap();
    let mut trie_account_2 = account_2.into_trie_account(EMPTY_ROOT_HASH);

    let mut hash_builder = HashBuilder::default()
        .with_proof_retainer(ProofRetainer::from_iter([address_path_1, address_path_2]));
    hash_builder.add_leaf(address_path_1, &alloy_rlp::encode(trie_account_1));
    hash_builder.add_leaf(address_path_2, &alloy_rlp::encode(trie_account_2));

    let root = hash_builder.root();
    let proof_nodes = hash_builder.take_proof_nodes();

    let provider_factory = DefaultTrieNodeProviderFactory;
    let mut sparse = TestSparseStateTrie::default().with_updates(true);
    sparse
        .reveal_decoded_multiproof(
            MultiProof {
                account_subtree: proof_nodes,
                branch_node_masks: BranchNodeMasksMap::from_iter([(
                    Nibbles::from_nibbles([0x1]),
                    BranchNodeMasks {
                        hash_mask: TrieMask::new(0b00),
                        tree_mask: TrieMask::default(),
                    },
                )]),
                storages: HashMap::from_iter([
                    (
                        address_1,
                        StorageMultiProof {
                            root,
                            subtree: storage_proof_nodes.clone(),
                            branch_node_masks: storage_branch_node_masks.clone(),
                        },
                    ),
                    (
                        address_2,
                        StorageMultiProof {
                            root,
                            subtree: storage_proof_nodes,
                            branch_node_masks: storage_branch_node_masks,
                        },
                    ),
                ]),
            }
            .try_into()
            .unwrap(),
        )
        .unwrap();

    assert_eq!(sparse.root(&provider_factory).unwrap(), root);

    let address_3 = b256!("0x2000000000000000000000000000000000000000000000000000000000000000");
    let address_path_3 = Nibbles::unpack(address_3);
    let account_3 = Account { nonce: account_1.nonce + 1, ..account_1 };
    let trie_account_3 = account_3.into_trie_account(EMPTY_ROOT_HASH);

    sparse
        .update_account_leaf(address_path_3, alloy_rlp::encode(trie_account_3), &provider_factory)
        .unwrap();

    sparse
        .update_storage_leaf(address_1, slot_path_3, alloy_rlp::encode(value_3), &provider_factory)
        .unwrap();
    trie_account_1.storage_root = sparse.storage_root(&address_1).unwrap();
    sparse
        .update_account_leaf(address_path_1, alloy_rlp::encode(trie_account_1), &provider_factory)
        .unwrap();

    sparse.wipe_storage(address_2).unwrap();
    trie_account_2.storage_root = sparse.storage_root(&address_2).unwrap();
    sparse
        .update_account_leaf(address_path_2, alloy_rlp::encode(trie_account_2), &provider_factory)
        .unwrap();

    sparse.root(&provider_factory).unwrap();

    let sparse_updates = sparse.take_trie_updates().unwrap();
    // TODO(alexey): assert against real state root calculation updates
    pretty_assertions::assert_eq!(
        sparse_updates,
        reth_trie::updates::TrieUpdates {
            account_nodes: HashMap::default(),
            storage_tries: HashMap::from_iter([(
                b256!("0x1100000000000000000000000000000000000000000000000000000000000000"),
                StorageTrieUpdates {
                    is_deleted: true,
                    storage_nodes: HashMap::default(),
                    removed_nodes: HashSet::default()
                }
            )]),
            removed_nodes: HashSet::default()
        }
    );
}

#[test]
fn test_filter_map_revealed_nodes() {
    let mut revealed_nodes = HashSet::from_iter([Nibbles::from_nibbles([0x0])]);
    let leaf = TrieNode::Leaf(LeafNode::new(Nibbles::default(), alloy_rlp::encode([])));
    let leaf_encoded = alloy_rlp::encode(&leaf);
    let branch = TrieNode::Branch(BranchNode::new(
        vec![RlpNode::from_rlp(&leaf_encoded), RlpNode::from_rlp(&leaf_encoded)],
        TrieMask::new(0b11),
    ));
    let proof_nodes = DecodedProofNodes::from_iter([
        (Nibbles::default(), branch.clone()),
        (Nibbles::from_nibbles([0x0]), leaf.clone()),
        (Nibbles::from_nibbles([0x1]), leaf.clone()),
    ]);

    let branch_node_masks = BranchNodeMasksMap::default();

    let decoded =
        filter_map_revealed_nodes(proof_nodes, &mut revealed_nodes, &branch_node_masks).unwrap();

    assert_eq!(
        decoded,
        FilterMappedProofNodes {
            root_node: Some(ProofTrieNode { path: Nibbles::default(), node: branch, masks: None }),
            nodes: vec![ProofTrieNode {
                path: Nibbles::from_nibbles([0x1]),
                node: leaf,
                masks: None
            }],
            // Branch, two of its children, one leaf
            new_nodes: 4,
            // Metric values
            metric_values: ProofNodesMetricValues {
                // Branch, leaf, leaf
                total_nodes: 3,
                // Revealed leaf node with path 0x1
                skipped_nodes: 1,
            },
        }
    );
}
