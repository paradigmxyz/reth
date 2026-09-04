//! Reproducible sparse trie benchmarks. Fixture setup, input allocation, and trie cloning are
//! not timed. `root_dirty` times hashing already-applied changes; `update_root` includes applying
//! changes, hashing, and extracting branch updates for persistence (but excludes database I/O).
//!
//! Run with `cargo bench -p reth-trie-sparse --bench state_root`. Set `MPT_BENCH_SAMPLES`
//! to change the number of samples (default: 50), or `MPT_BENCH_FILTER` to select cases.
//! Set `RAYON_NUM_THREADS` explicitly when comparing runs. Roots are checked against the
//! independent `alloy_trie::HashBuilder` implementation outside the timed region.

use alloy_primitives::{keccak256, map::B256Map, B256, U256};
use reth_trie_sparse::{ArenaParallelSparseTrie, LeafUpdate, SparseTrie, TrieNodeEpoch};
use std::{hint::black_box, time::Instant};

fn updates(keys: &[B256], value: u64) -> B256Map<LeafUpdate> {
    let value = alloy_rlp::encode(U256::from(value));
    keys.iter().map(|key| (*key, LeafUpdate::Changed(value.clone()))).collect()
}

fn benchmark(
    name: &str,
    fixture: &ArenaParallelSparseTrie,
    expected_root: B256,
    samples: usize,
    filter: &str,
    leaf_updates: Option<&B256Map<LeafUpdate>>,
) {
    if !name.contains(filter) {
        return;
    }
    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples + 5 {
        let mut trie = fixture.clone();
        let mut leaf_updates = leaf_updates.cloned();
        let start = Instant::now();
        if let Some(ref mut updates) = leaf_updates {
            trie.update_leaves(black_box(updates), |_, _| unreachable!()).unwrap();
        }
        let root = black_box(trie.root(black_box(TrieNodeEpoch::new(2))));
        let branch_updates = leaf_updates.is_some().then(|| black_box(trie.take_updates()));
        let elapsed = start.elapsed().as_nanos();
        black_box(branch_updates);
        assert_eq!(root, expected_root);
        durations.push(elapsed);
    }
    durations.drain(..5);
    durations.sort_unstable();
    let median = durations[durations.len() / 2];
    let lower = durations[durations.len() / 4];
    let upper = durations[durations.len() * 3 / 4];
    println!("{name},{samples},{median},{lower},{upper},{expected_root}");
}

fn reference_root(keys: &[B256], changed: usize) -> B256 {
    let mut leaves = keys
        .iter()
        .enumerate()
        .map(|(i, key)| (*key, alloy_rlp::encode(U256::from(if i < changed { 2 } else { 1 }))))
        .collect::<Vec<_>>();
    leaves.sort_unstable_by_key(|(key, _)| *key);
    let mut builder = alloy_trie::HashBuilder::default();
    for (key, value) in leaves {
        builder.add_leaf(alloy_trie::Nibbles::unpack(key), &value);
    }
    builder.root()
}

fn main() {
    let samples = std::env::var("MPT_BENCH_SAMPLES")
        .map(|value| value.parse::<usize>().expect("MPT_BENCH_SAMPLES must be a positive integer"))
        .unwrap_or(50);
    assert!(samples > 0);
    let filter = std::env::var("MPT_BENCH_FILTER").unwrap_or_default();
    println!("case,samples,median_ns,p25_ns,p75_ns,root");

    for size in [1_000, 32_768] {
        let keys: Vec<B256> = (0..size as u64).map(|i| keccak256(i.to_be_bytes())).collect();
        for retain_updates in [false, true] {
            let mut base = ArenaParallelSparseTrie::default();
            base.set_updates(retain_updates);
            base.update_leaves(&mut updates(&keys, 1), |_, _| unreachable!()).unwrap();
            base.root(TrieNodeEpoch::new(1));
            base.take_updates();
            let mode = if retain_updates { "retain" } else { "discard" };
            benchmark(
                &format!("root_cached/{size}/{mode}"),
                &base,
                reference_root(&keys, 0),
                samples,
                &filter,
                None,
            );

            for changed in [1, size / 10, size] {
                let expected_root = reference_root(&keys, changed);
                let leaf_updates = updates(&keys[..changed], 2);
                benchmark(
                    &format!("update_root/{size}/{changed}/{mode}"),
                    &base,
                    expected_root,
                    samples,
                    &filter,
                    Some(&leaf_updates),
                );
                let mut dirty = base.clone();
                dirty
                    .update_leaves(&mut updates(&keys[..changed], 2), |_, _| unreachable!())
                    .unwrap();
                benchmark(
                    &format!("root_dirty/{size}/{changed}/{mode}"),
                    &dirty,
                    expected_root,
                    samples,
                    &filter,
                    None,
                );
            }
        }
    }
}
