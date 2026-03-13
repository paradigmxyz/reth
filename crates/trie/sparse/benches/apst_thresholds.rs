#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::{map::B256Map, B256, U256};
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use rand::{seq::SliceRandom, Rng, SeedableRng};
use reth_trie::test_utils::TrieTestHarness;
use reth_trie_common::{Nibbles, ProofTrieNodeV2, ProofV2Target};
use reth_trie_sparse::{
    ArenaParallelSparseTrie, ArenaParallelismThresholds, LeafUpdate, SparseTrie,
};
use std::{collections::BTreeMap, time::Duration};

/// Pre-computed data for a single benchmark configuration.
struct BenchData {
    harness: TrieTestHarness,
    leaf_updates: B256Map<LeafUpdate>,
    proof_nodes: Vec<ProofTrieNodeV2>,
    retained_leaves: Vec<Nibbles>,
}

/// Generates deterministic benchmark data.
///
/// 1. Creates `dataset_size` random storage slots.
/// 2. Builds a `TrieTestHarness` from that dataset.
/// 3. Generates a changeset of `changeset_size` entries (~70% updates, ~20% new, ~10% deletes).
/// 4. Pre-computes proof nodes via the reveal-update loop.
/// 5. Picks ~50% of existing keys as retained leaves for prune benchmarks.
fn generate_bench_data(dataset_size: usize, changeset_size: usize, seed: u64) -> BenchData {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

    // 1. Generate base dataset of random storage slots with non-zero values.
    let mut storage = BTreeMap::new();
    while storage.len() < dataset_size {
        let key = B256::from(rng.random::<[u8; 32]>());
        let value = U256::from(rng.random::<u64>() | 1); // ensure non-zero
        storage.insert(key, value);
    }

    // 2. Build harness.
    let harness = TrieTestHarness::new(storage.clone());

    // 3. Build changeset: ~70% updates to existing, ~20% new, ~10% deletes.
    let existing_keys: Vec<B256> = storage.keys().copied().collect();
    let num_updates = changeset_size * 70 / 100;
    let num_new = changeset_size * 20 / 100;
    let num_deletes = changeset_size - num_updates - num_new;

    let mut shuffled = existing_keys.clone();
    shuffled.shuffle(&mut rng);

    let mut changeset = BTreeMap::new();

    // Updates to existing keys
    for &key in shuffled.iter().take(num_updates) {
        changeset.insert(key, U256::from(rng.random::<u64>() | 1));
    }

    // Deletions of existing keys
    for &key in shuffled.iter().skip(num_updates).take(num_deletes) {
        changeset.insert(key, U256::ZERO);
    }

    // New insertions
    for _ in 0..num_new {
        let key = B256::from(rng.random::<[u8; 32]>());
        changeset.insert(key, U256::from(rng.random::<u64>() | 1));
    }

    // 4. Build leaf updates.
    let leaf_updates: B256Map<LeafUpdate> = changeset
        .iter()
        .map(|(&slot, &value)| {
            let rlp_value = if value == U256::ZERO {
                Vec::new()
            } else {
                alloy_rlp::encode_fixed_size(&value).to_vec()
            };
            (slot, LeafUpdate::Changed(rlp_value))
        })
        .collect();

    // 5. Pre-compute proof nodes via the reveal-update loop on a temporary APST.
    let root_node = harness.root_node();
    let mut temp_apst = ArenaParallelSparseTrie::default();
    temp_apst
        .set_root(root_node.node.clone(), root_node.masks.clone(), false)
        .expect("set_root should succeed");

    let mut all_proof_nodes: Vec<ProofTrieNodeV2> = Vec::new();
    let mut temp_updates = leaf_updates.clone();

    loop {
        let mut targets: Vec<ProofV2Target> = Vec::new();
        temp_apst
            .update_leaves(&mut temp_updates, |key, min_len| {
                targets.push(ProofV2Target::new(key).with_min_len(min_len));
            })
            .expect("update_leaves should succeed");

        if targets.is_empty() {
            break;
        }

        let (mut proof_nodes, _) = harness.proof_v2(&mut targets);
        temp_apst.reveal_nodes(&mut proof_nodes).expect("reveal_nodes should succeed");
        all_proof_nodes.append(&mut proof_nodes);
    }

    // 6. Retained leaves: ~50% of existing keys.
    let mut retained_keys = existing_keys;
    retained_keys.shuffle(&mut rng);
    let num_retain = retained_keys.len() / 2;
    let retained_leaves: Vec<Nibbles> =
        retained_keys[..num_retain].iter().map(|k| Nibbles::unpack(*k)).collect();

    BenchData { harness, leaf_updates, proof_nodes: all_proof_nodes, retained_leaves }
}

/// Creates a fresh APST with one threshold set to `threshold_value` and all others forced
/// parallel (set to 0).
fn make_thresholds_for_reveal(threshold_value: usize) -> ArenaParallelismThresholds {
    ArenaParallelismThresholds {
        min_revealed_nodes: threshold_value,
        min_updates: 0,
        min_dirty_leaves: 0,
        min_leaves_for_prune: 0,
        ..Default::default()
    }
}

fn make_thresholds_for_update(threshold_value: usize, global: bool) -> ArenaParallelismThresholds {
    ArenaParallelismThresholds {
        min_revealed_nodes: 0,
        min_updates: threshold_value,
        min_dirty_leaves: 0,
        min_leaves_for_prune: 0,
        global_updates_threshold: global,
    }
}

fn make_thresholds_for_hash(threshold_value: u64) -> ArenaParallelismThresholds {
    ArenaParallelismThresholds {
        min_revealed_nodes: 0,
        min_updates: 0,
        min_dirty_leaves: threshold_value,
        min_leaves_for_prune: 0,
        ..Default::default()
    }
}

fn make_thresholds_for_prune(threshold_value: u64) -> ArenaParallelismThresholds {
    ArenaParallelismThresholds {
        min_revealed_nodes: 0,
        min_updates: 0,
        min_dirty_leaves: 0,
        min_leaves_for_prune: threshold_value,
        ..Default::default()
    }
}

const DATASET_SIZES: &[usize] = &[100, 1_000, 10_000, 50_000];
const CHANGESET_SIZES: &[usize] = &[50, 500, 5_000, 25_000];
const THRESHOLD_VALUES: &[usize] = &[1, 4, 16, 32, 64, 128, 256, 512, 1024];

const UPDATE_DATASET_SIZES: &[usize] = &[10_000, 50_000, 100_000, 200_000, 500_000];
const UPDATE_CHANGESET_SIZES: &[usize] = &[5_000, 25_000, 50_000, 100_000];

/// Runs the full pipeline up to (but not including) reveal_nodes, then times reveal_nodes.
fn bench_reveal_nodes(c: &mut Criterion) {
    let mut group = c.benchmark_group("reveal_nodes");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(200);

    for &ds in DATASET_SIZES {
        for &cs in CHANGESET_SIZES {
            if cs > ds {
                continue;
            }
            let seed = (ds as u64) ^ ((cs as u64) << 32);
            let data = generate_bench_data(ds, cs, seed);

            for &t in THRESHOLD_VALUES {
                let id = BenchmarkId::new("reveal_nodes", format!("{ds}/{cs}/t={t}"));
                let thresholds = make_thresholds_for_reveal(t);

                group.bench_function(id, |b| {
                    b.iter_batched(
                        || {
                            let root_node = data.harness.root_node();
                            let mut apst = ArenaParallelSparseTrie::default()
                                .with_parallelism_thresholds(thresholds);
                            apst.set_root(root_node.node, root_node.masks, false)
                                .expect("set_root should succeed");
                            let proof_nodes = data.proof_nodes.clone();
                            (apst, proof_nodes)
                        },
                        |(mut apst, mut proof_nodes): (
                            ArenaParallelSparseTrie,
                            Vec<ProofTrieNodeV2>,
                        )| {
                            apst.reveal_nodes(&mut proof_nodes)
                                .expect("reveal_nodes should succeed");
                        },
                        BatchSize::SmallInput,
                    );
                });
            }
        }
    }

    group.finish();
}

/// Times update_leaves after performing reveal_nodes as setup.
/// Benchmarks both parallelism modes ("global" = all-or-nothing, "per_sub" = per-subtrie)
/// across larger dataset/changeset sizes to find where parallelism matters.
fn bench_update_leaves(c: &mut Criterion) {
    let mut group = c.benchmark_group("update_leaves");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(100);

    for &ds in UPDATE_DATASET_SIZES {
        for &cs in UPDATE_CHANGESET_SIZES {
            if cs > ds {
                continue;
            }
            let seed = (ds as u64) ^ ((cs as u64) << 32);
            let data = generate_bench_data(ds, cs, seed);

            for &t in THRESHOLD_VALUES {
                for &(mode_name, global) in &[("global", true), ("per_sub", false)] {
                    let id =
                        BenchmarkId::new("update_leaves", format!("{ds}/{cs}/t={t}/{mode_name}"));
                    let thresholds = make_thresholds_for_update(t, global);

                    group.bench_function(id, |b| {
                        b.iter_batched(
                            || {
                                let root_node = data.harness.root_node();
                                let mut apst = ArenaParallelSparseTrie::default()
                                    .with_parallelism_thresholds(thresholds);
                                apst.set_root(root_node.node, root_node.masks, false)
                                    .expect("set_root should succeed");
                                let mut proof_nodes = data.proof_nodes.clone();
                                apst.reveal_nodes(&mut proof_nodes)
                                    .expect("reveal_nodes should succeed");
                                let leaf_updates = data.leaf_updates.clone();
                                (apst, leaf_updates)
                            },
                            |(mut apst, mut leaf_updates): (
                                ArenaParallelSparseTrie,
                                B256Map<LeafUpdate>,
                            )| {
                                apst.update_leaves(&mut leaf_updates, |_, _| {})
                                    .expect("update_leaves should succeed");
                            },
                            BatchSize::SmallInput,
                        );
                    });
                }
            }
        }
    }

    group.finish();
}

/// Times root() (hash computation) after reveal + update as setup.
fn bench_root(c: &mut Criterion) {
    let mut group = c.benchmark_group("root");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(200);

    for &ds in DATASET_SIZES {
        for &cs in CHANGESET_SIZES {
            if cs > ds {
                continue;
            }
            let seed = (ds as u64) ^ ((cs as u64) << 32);
            let data = generate_bench_data(ds, cs, seed);

            for &t in THRESHOLD_VALUES {
                let id = BenchmarkId::new("root", format!("{ds}/{cs}/t={t}"));
                let thresholds = make_thresholds_for_hash(t as u64);

                group.bench_function(id, |b| {
                    b.iter_batched(
                        || {
                            let root_node = data.harness.root_node();
                            let mut apst = ArenaParallelSparseTrie::default()
                                .with_parallelism_thresholds(thresholds);
                            apst.set_root(root_node.node, root_node.masks, false)
                                .expect("set_root should succeed");
                            let mut proof_nodes = data.proof_nodes.clone();
                            apst.reveal_nodes(&mut proof_nodes)
                                .expect("reveal_nodes should succeed");
                            let mut leaf_updates = data.leaf_updates.clone();
                            apst.update_leaves(&mut leaf_updates, |_, _| {})
                                .expect("update_leaves should succeed");
                            apst
                        },
                        |mut apst: ArenaParallelSparseTrie| {
                            let _ = apst.root();
                        },
                        BatchSize::SmallInput,
                    );
                });
            }
        }
    }

    group.finish();
}

/// Times prune() after the full reveal → update → hash pipeline as setup.
fn bench_prune(c: &mut Criterion) {
    let mut group = c.benchmark_group("prune");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(200);

    for &ds in DATASET_SIZES {
        for &cs in CHANGESET_SIZES {
            if cs > ds {
                continue;
            }
            let seed = (ds as u64) ^ ((cs as u64) << 32);
            let data = generate_bench_data(ds, cs, seed);

            for &t in THRESHOLD_VALUES {
                let id = BenchmarkId::new("prune", format!("{ds}/{cs}/t={t}"));
                let thresholds = make_thresholds_for_prune(t as u64);

                group.bench_function(id, |b| {
                    b.iter_batched(
                        || {
                            let root_node = data.harness.root_node();
                            let mut apst = ArenaParallelSparseTrie::default()
                                .with_parallelism_thresholds(thresholds);
                            apst.set_root(root_node.node, root_node.masks, false)
                                .expect("set_root should succeed");
                            let mut proof_nodes = data.proof_nodes.clone();
                            apst.reveal_nodes(&mut proof_nodes)
                                .expect("reveal_nodes should succeed");
                            let mut leaf_updates = data.leaf_updates.clone();
                            apst.update_leaves(&mut leaf_updates, |_, _| {})
                                .expect("update_leaves should succeed");
                            let _ = apst.root();
                            let retained = data.retained_leaves.clone();
                            (apst, retained)
                        },
                        |(mut apst, retained): (ArenaParallelSparseTrie, Vec<Nibbles>)| {
                            apst.prune(&retained);
                        },
                        BatchSize::SmallInput,
                    );
                });
            }
        }
    }

    group.finish();
}

criterion_group!(apst_benches, bench_reveal_nodes, bench_update_leaves, bench_root, bench_prune);
criterion_main!(apst_benches);
