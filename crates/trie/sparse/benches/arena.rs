//! Benchmarks for [`ArenaParallelSparseTrie`] that mirror how the engine drives it: apply a batch
//! of leaf updates, learn which proofs are missing, reveal them, retry, and finally hash.
//!
//! The dataset sizes can be overridden to shorten a run, e.g.
//! `ARENA_BENCH_LEAVES=50000 ARENA_BENCH_UPDATES=1000,10000`.
#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::{map::B256Map, B256, U256};
use criterion::{
    criterion_group, criterion_main, BatchSize, BenchmarkGroup, BenchmarkId, Criterion,
};
use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use reth_trie::test_utils::TrieTestHarness;
use reth_trie_common::{ProofTrieNodeV2, ProofV2Target};
use reth_trie_sparse::{ArenaParallelSparseTrie, LeafUpdate, SparseTrie, TrieNodeEpoch};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// Leaf counts of the base storage tries.
const DEFAULT_LEAVES: &[usize] = &[50_000, 250_000];

/// Sizes of the update sets applied on top of a base trie.
const DEFAULT_UPDATES: &[usize] = &[1_000, 10_000, 50_000];

/// Update count for the serial root scenario.
const SMALL_DIRTY: usize = 64;

/// Epoch passed to [`SparseTrie::root`].
const ROOT_EPOCH: TrieNodeEpoch = TrieNodeEpoch::new(1);

const SEED: u64 = 0xA11CE;

fn arena_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("arena");
    group.warm_up_time(Duration::from_millis(300));

    for leaves in env_sizes("ARENA_BENCH_LEAVES", DEFAULT_LEAVES) {
        let started = Instant::now();
        let mut rng = StdRng::seed_from_u64(SEED);
        let storage = random_storage(leaves, &mut rng);
        let keys = storage.keys().copied().collect::<Vec<_>>();
        let harness = TrieTestHarness::new(storage);
        let cold = cold_trie(&harness);
        eprintln!("prepared {} leaf base trie in {:.1?}", label(leaves), started.elapsed());

        let small = Prepared::new(&harness, &cold, random_updates(&keys, SMALL_DIRTY, &mut rng));
        bench_root_small_dirty(&mut group, &small, &label(leaves));
        drop(small);

        for updates in env_sizes("ARENA_BENCH_UPDATES", DEFAULT_UPDATES) {
            let id = format!("{}/{}", label(leaves), label(updates));
            let started = Instant::now();
            let prepared = Prepared::new(&harness, &cold, random_updates(&keys, updates, &mut rng));
            eprintln!(
                "prepared {id} in {:.1?}: {} proof nodes, {} of {updates} updates still blocked on retry",
                started.elapsed(),
                prepared.cold_proofs.len(),
                prepared.retry_blocked,
            );

            bench_combination(&mut group, &prepared, &id);
        }
    }

    group.finish();
}

/// Everything a single (trie size, update count) combination needs, prepared once and cloned per
/// iteration.
struct Prepared {
    /// Trie with only the root node revealed, so every update blocks.
    cold: ArenaParallelSparseTrie,
    /// The update set: 80% new values for existing leaves, 10% insertions, 10% deletions.
    updates: B256Map<LeafUpdate>,
    /// Proof nodes answering a fully blocked pass over [`Self::cold`].
    cold_proofs: Vec<ProofTrieNodeV2>,
    /// [`Self::cold`] with proofs revealed for every other blocked target.
    half_revealed: ArenaParallelSparseTrie,
    /// Trie revealed far enough that one pass applies every update, with none applied yet.
    warm: ArenaParallelSparseTrie,
    /// [`Self::warm`] with all updates applied, root not yet computed.
    dirty: ArenaParallelSparseTrie,
    /// [`Self::dirty`] after hashing, with the recorded updates still in place.
    hashed: ArenaParallelSparseTrie,
    /// How many updates still block on [`Self::half_revealed`].
    retry_blocked: usize,
}

impl Prepared {
    fn new(
        harness: &TrieTestHarness,
        cold: &ArenaParallelSparseTrie,
        updates: B256Map<LeafUpdate>,
    ) -> Self {
        let mut cold_targets = required_targets(cold, &updates);
        assert!(!cold_targets.is_empty(), "cold trie should block every update");
        let (cold_proofs, _) = harness.proof_v2(&mut cold_targets);

        let mut warm = cold.clone();
        reveal(&mut warm, &cold_proofs);
        loop {
            // Deletions can collapse a branch and need a sibling that the first round did not
            // include.
            let mut targets = required_targets(&warm, &updates);
            if targets.is_empty() {
                break
            }
            let (proofs, _) = harness.proof_v2(&mut targets);
            reveal(&mut warm, &proofs);
        }

        let mut half_targets = cold_targets.iter().copied().step_by(2).collect::<Vec<_>>();
        let (half_proofs, _) = harness.proof_v2(&mut half_targets);
        let mut half_revealed = cold.clone();
        reveal(&mut half_revealed, &half_proofs);

        let mut dirty = warm.clone();
        let mut pending = updates.clone();
        dirty
            .update_leaves(&mut pending, |_, _| panic!("warm trie must not block"))
            .expect("update_leaves");

        let mut hashed = dirty.clone();
        hashed.root(ROOT_EPOCH);

        let retry_blocked = required_targets(&half_revealed, &updates).len();
        Self {
            cold: cold.clone(),
            updates,
            cold_proofs,
            half_revealed,
            warm,
            dirty,
            hashed,
            retry_blocked,
        }
    }
}

fn bench_combination(group: &mut BenchGroup<'_>, prepared: &Prepared, id: &str) {
    group.sample_size(20).measurement_time(Duration::from_millis(1500));

    group.bench_function(BenchmarkId::new("update_leaves/cold", id), |b| {
        b.iter_batched(
            || (prepared.cold.clone(), prepared.updates.clone()),
            |(mut trie, mut updates)| apply(&mut trie, &mut updates),
            BatchSize::LargeInput,
        )
    });

    group.bench_function(BenchmarkId::new("update_leaves/retry", id), |b| {
        b.iter_batched(
            || (prepared.half_revealed.clone(), prepared.updates.clone()),
            |(mut trie, mut updates)| apply(&mut trie, &mut updates),
            BatchSize::LargeInput,
        )
    });

    group.bench_function(BenchmarkId::new("update_leaves/warm", id), |b| {
        b.iter_batched(
            || (prepared.warm.clone(), prepared.updates.clone()),
            |(mut trie, mut updates)| apply(&mut trie, &mut updates),
            BatchSize::LargeInput,
        )
    });

    group.bench_function(BenchmarkId::new("reveal_nodes", id), |b| {
        b.iter_batched(
            || (prepared.cold.clone(), prepared.cold_proofs.clone()),
            |(mut trie, mut nodes)| trie.reveal_nodes(&mut nodes).expect("reveal_nodes"),
            BatchSize::LargeInput,
        )
    });

    group.bench_function(BenchmarkId::new("root/after_updates", id), |b| {
        b.iter_batched(
            || prepared.dirty.clone(),
            |mut trie| trie.root(ROOT_EPOCH),
            BatchSize::LargeInput,
        )
    });

    // `take_updates` is orders of magnitude cheaper than cloning its input, and criterion sizes
    // the run from the measured time alone. A tiny measurement time keeps it at criterion's
    // floor of `sample_size * (sample_size + 1) / 2` iterations.
    group.sample_size(10).measurement_time(Duration::from_millis(10));
    group.bench_function(BenchmarkId::new("take_updates", id), |b| {
        b.iter_batched(
            || prepared.hashed.clone(),
            |mut trie| trie.take_updates(),
            BatchSize::LargeInput,
        )
    });
}

fn bench_root_small_dirty(group: &mut BenchGroup<'_>, prepared: &Prepared, id: &str) {
    group.sample_size(20).measurement_time(Duration::from_millis(1500));
    group.bench_function(BenchmarkId::new("root/small_dirty", id), |b| {
        b.iter_batched(
            || prepared.dirty.clone(),
            |mut trie| trie.root(ROOT_EPOCH),
            BatchSize::LargeInput,
        )
    });
}

type BenchGroup<'a> = BenchmarkGroup<'a, criterion::measurement::WallTime>;

/// Applies `updates`, collecting the proof targets the trie asks for, like the engine's proof
/// fetch loop does.
fn apply(
    trie: &mut ArenaParallelSparseTrie,
    updates: &mut B256Map<LeafUpdate>,
) -> Vec<ProofV2Target> {
    let mut targets = Vec::new();
    trie.update_leaves(updates, |key, parent| {
        targets.push(ProofV2Target::new(key).with_parent(parent))
    })
    .expect("update_leaves");
    targets
}

/// Returns the proof targets one `update_leaves` pass over a copy of `trie` requests.
fn required_targets(
    trie: &ArenaParallelSparseTrie,
    updates: &B256Map<LeafUpdate>,
) -> Vec<ProofV2Target> {
    apply(&mut trie.clone(), &mut updates.clone())
}

fn reveal(trie: &mut ArenaParallelSparseTrie, nodes: &[ProofTrieNodeV2]) {
    trie.reveal_nodes(&mut nodes.to_vec()).expect("reveal_nodes");
}

fn cold_trie(harness: &TrieTestHarness) -> ArenaParallelSparseTrie {
    let root = harness.root_node();
    let mut trie = ArenaParallelSparseTrie::default();
    trie.set_root(root.node, root.masks, true).expect("set_root");
    trie
}

fn random_storage(leaves: usize, rng: &mut StdRng) -> BTreeMap<B256, U256> {
    (0..leaves).map(|_| (B256::from(rng.random::<[u8; 32]>()), random_value(rng))).collect()
}

/// Builds an update set of `count` entries: 80% new values for existing leaves, 10% insertions of
/// fresh keys, 10% deletions.
fn random_updates(keys: &[B256], count: usize, rng: &mut StdRng) -> B256Map<LeafUpdate> {
    let changed = count * 8 / 10;
    let removed = count / 10;
    assert!(keys.len() >= changed + removed, "not enough leaves for {count} updates");

    let mut existing = keys.to_vec();
    existing.shuffle(rng);
    let mut updates = B256Map::default();
    for &key in &existing[..changed] {
        updates.insert(key, LeafUpdate::Changed(encode(random_value(rng))));
    }
    for &key in &existing[changed..changed + removed] {
        updates.insert(key, LeafUpdate::Changed(Vec::new()));
    }
    while updates.len() < count {
        updates.insert(
            B256::from(rng.random::<[u8; 32]>()),
            LeafUpdate::Changed(encode(random_value(rng))),
        );
    }
    updates
}

fn random_value(rng: &mut StdRng) -> U256 {
    U256::from(rng.random::<u64>() | 1)
}

fn encode(value: U256) -> Vec<u8> {
    alloy_rlp::encode_fixed_size(&value).to_vec()
}

fn env_sizes(var: &str, default: &[usize]) -> Vec<usize> {
    let Ok(value) = std::env::var(var) else { return default.to_vec() };
    value
        .split(',')
        .map(|part| part.trim().parse().unwrap_or_else(|_| panic!("invalid {var}: {value}")))
        .collect()
}

fn label(count: usize) -> String {
    if count >= 1_000 && count.is_multiple_of(1_000) {
        format!("{}k", count / 1_000)
    } else {
        count.to_string()
    }
}

criterion_group!(arena, arena_benches);
criterion_main!(arena);
