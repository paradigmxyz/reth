#![allow(missing_docs, unreachable_pub)]
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256, U256,
};
use alloy_rlp::Decodable;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use itertools::Itertools;
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use reth_primitives_traits::Account;
use reth_trie::{
    hashed_cursor::{mock::MockHashedCursorFactory, HashedCursorFactory},
    proof::Proof,
    proof_v2::{ProofCalculator, SyncAccountValueEncoder},
    trie_cursor::{depth_first, mock::MockTrieCursorFactory, TrieCursorFactory},
};
use reth_trie_common::{
    HashedPostState, MultiProofTargets, Nibbles, ProofTrieNode, TrieMasks, TrieNode,
};
use std::collections::BTreeMap;

/// Benchmark for proof_v2 implementation.
///
/// This benchmark tests the performance of the new proof calculator that generates
/// merkle proofs using only leaf data, across multiple dataset sizes and target counts.
pub fn proof_v2_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Proof V2");
    group.sample_size(20);

    // Test across multiple dataset sizes and target counts
    for dataset_size in [100, 500, 1_000] {
        for num_targets in [1, 10, 50, 100] {
            // Skip combinations where targets > dataset (doesn't make sense)
            if num_targets > dataset_size {
                continue;
            }

            let (hashed_post_state, targets, _target_b256s) =
                generate_test_data(dataset_size, num_targets);

            // Create mock cursor factories from the hashed post state
            let (trie_cursor_factory, hashed_cursor_factory) =
                create_cursor_factories(&hashed_post_state);

            // Benchmark ID includes both dimensions: dataset_size/num_targets
            let bench_id = format!("v2/dataset_{}/targets_{}", dataset_size, num_targets);
            group.bench_function(BenchmarkId::new("account_proof", bench_id), |b| {
                b.iter(|| {
                    let trie_cursor = trie_cursor_factory
                        .account_trie_cursor()
                        .expect("Failed to create trie cursor");
                    let hashed_cursor = hashed_cursor_factory
                        .hashed_account_cursor()
                        .expect("Failed to create hashed cursor");

                    let value_encoder = SyncAccountValueEncoder::new(
                        trie_cursor_factory.clone(),
                        hashed_cursor_factory.clone(),
                    );

                    let mut proof_calculator = ProofCalculator::new(trie_cursor, hashed_cursor);
                    proof_calculator
                        .proof(&value_encoder, targets.clone())
                        .expect("Proof generation failed")
                })
            });
        }
    }
}

/// Benchmark for legacy proof implementation.
///
/// This benchmark tests the performance of the original proof calculator that uses
/// trie walking for comparison with the proof_v2 implementation, across multiple
/// dataset sizes and target counts.
pub fn proof_legacy_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Proof V2");
    group.sample_size(20);

    // Test across multiple dataset sizes and target counts
    for dataset_size in [100, 500, 1_000] {
        for num_targets in [1, 10, 50, 100] {
            // Skip combinations where targets > dataset (doesn't make sense)
            if num_targets > dataset_size {
                continue;
            }

            let (hashed_post_state, _targets, target_b256s) =
                generate_test_data(dataset_size, num_targets);

            // Create mock cursor factories from the hashed post state
            let (trie_cursor_factory, hashed_cursor_factory) =
                create_cursor_factories(&hashed_post_state);

            // Convert B256 targets to MultiProofTargets (account targets with empty storage sets)
            let legacy_targets: MultiProofTargets =
                target_b256s.iter().map(|addr| (*addr, B256Set::default())).collect();

            // Benchmark ID includes both dimensions: dataset_size/num_targets
            let bench_id = format!("legacy/dataset_{}/targets_{}", dataset_size, num_targets);
            // Benchmark account proof generation using legacy implementation
            // This includes decoding and sorting to match what proof_v2 returns
            group.bench_function(BenchmarkId::new("account_proof", bench_id), |b| {
                b.iter(|| {
                    let proof_result =
                        Proof::new(trie_cursor_factory.clone(), hashed_cursor_factory.clone())
                            .multiproof(legacy_targets.clone())
                            .expect("Legacy proof generation failed");

                    // Decode and sort legacy proof nodes (same as in proof_v2 tests)
                    let _proof_nodes: Vec<ProofTrieNode> = proof_result
                        .account_subtree
                        .iter()
                        .map(|(path, node_enc)| {
                            let mut buf = node_enc.as_ref();
                            let node = TrieNode::decode(&mut buf)
                                .expect("legacy implementation should produce valid proof nodes");

                            ProofTrieNode {
                                path: *path,
                                node,
                                masks: TrieMasks {
                                    hash_mask: proof_result
                                        .branch_node_hash_masks
                                        .get(path)
                                        .copied(),
                                    tree_mask: proof_result
                                        .branch_node_tree_masks
                                        .get(path)
                                        .copied(),
                                },
                            }
                        })
                        .sorted_by(|a, b| depth_first::cmp(&a.path, &b.path))
                        .collect();
                })
            });
        }
    }
}

/// Generate test data for benchmarking.
///
/// Returns a tuple of:
/// - `HashedPostState` with random accounts
/// - Proof targets (Nibbles) that are 80% from existing accounts, 20% random
/// - Proof targets (B256) for legacy implementation
fn generate_test_data(
    dataset_size: usize,
    num_targets: usize,
) -> (HashedPostState, Vec<Nibbles>, Vec<B256>) {
    let mut runner = TestRunner::deterministic();

    // Generate random accounts
    let accounts_strategy =
        proptest::collection::vec((any::<[u8; 32]>(), account_strategy()), dataset_size);

    let accounts = accounts_strategy.new_tree(&mut runner).unwrap().current();

    // Convert to HashedPostState
    let account_map: B256Map<_> = accounts
        .iter()
        .map(|(addr_bytes, account)| (B256::from(*addr_bytes), Some(*account)))
        .collect();

    // All accounts have empty storages
    let storages =
        account_map.keys().copied().map(|addr| (addr, Default::default())).collect::<B256Map<_>>();

    let hashed_post_state = HashedPostState { accounts: account_map.clone(), storages };

    // Generate proof targets: 80% from existing accounts, 20% random
    let account_keys: Vec<B256> = account_map.keys().copied().collect();

    let targets_strategy = proptest::collection::vec(
        prop::bool::weighted(0.8).prop_flat_map(move |from_accounts| {
            if from_accounts && !account_keys.is_empty() {
                prop::sample::select(account_keys.clone()).boxed()
            } else {
                any::<[u8; 32]>().prop_map(B256::from).boxed()
            }
        }),
        num_targets,
    );

    let target_b256s = targets_strategy.new_tree(&mut runner).unwrap().current();

    // Convert B256 targets to sorted Nibbles
    let mut targets: Vec<Nibbles> = target_b256s
        .iter()
        .map(|b256| {
            // SAFETY: B256 is exactly 32 bytes
            unsafe { Nibbles::unpack_unchecked(b256.as_slice()) }
        })
        .collect();
    targets.sort();

    (hashed_post_state, targets, target_b256s)
}

/// Generate a strategy for Account values
fn account_strategy() -> impl Strategy<Value = Account> {
    (any::<u64>(), any::<u64>(), any::<[u8; 32]>()).prop_map(|(nonce, balance, code_hash)| {
        Account { nonce, balance: U256::from(balance), bytecode_hash: Some(B256::from(code_hash)) }
    })
}

/// Create cursor factories from a `HashedPostState`.
///
/// This mimics the test harness pattern from the proof_v2 tests.
fn create_cursor_factories(
    post_state: &HashedPostState,
) -> (MockTrieCursorFactory, MockHashedCursorFactory) {
    // Extract accounts from post state, filtering out None (deleted accounts)
    let hashed_accounts: BTreeMap<B256, _> = post_state
        .accounts
        .iter()
        .filter_map(|(addr, account)| account.map(|acc| (*addr, acc)))
        .collect();

    // Extract storage tries from post state
    let hashed_storage_tries: B256Map<BTreeMap<B256, U256>> = post_state
        .storages
        .iter()
        .map(|(addr, hashed_storage)| {
            // Convert HashedStorage to BTreeMap, filtering out zero values (deletions)
            let storage_map: BTreeMap<B256, U256> = hashed_storage
                .storage
                .iter()
                .filter_map(|(slot, value)| (*value != U256::ZERO).then_some((*slot, *value)))
                .collect();
            (*addr, storage_map)
        })
        .collect();

    // Ensure that there's a storage trie dataset for every storage trie, even if empty
    let storage_trie_nodes: B256Map<BTreeMap<_, _>> =
        hashed_storage_tries.keys().copied().map(|addr| (addr, Default::default())).collect();

    // Create mock hashed cursor factory populated with the post state data
    let hashed_cursor_factory = MockHashedCursorFactory::new(hashed_accounts, hashed_storage_tries);

    // Create empty trie cursor factory (leaf-only calculator doesn't need trie nodes)
    let trie_cursor_factory = MockTrieCursorFactory::new(BTreeMap::new(), storage_trie_nodes);

    (trie_cursor_factory, hashed_cursor_factory)
}

criterion_group!(proof_comparison, proof_v2_benchmark, proof_legacy_benchmark);
criterion_main!(proof_comparison);
