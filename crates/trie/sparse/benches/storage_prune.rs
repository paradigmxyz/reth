#![allow(missing_docs, unreachable_pub)]
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_trie_common::Nibbles;
use reth_trie_sparse::{
    provider::DefaultTrieNodeProviderFactory, ParallelSparseTrie, RevealableSparseTrie,
    SparseStateTrie,
};

fn storage_prune_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("sparse storage prune");

    for (storage_tries, keep, depth) in
        [(50usize, 10usize, 2usize), (50usize, 80usize, 2usize), (200, 40, 2), (200, 40, 4)]
    {
        let name = format!("tries={storage_tries} keep={keep} depth={depth}");
        group.bench_function(name, |b| {
            b.iter_batched(
                || build_trie_fixture(storage_tries),
                |mut trie| {
                    trie.prune(depth, keep);
                },
                BatchSize::LargeInput,
            )
        });
    }

    group.finish();
}

fn build_trie_fixture(
    storage_tries: usize,
) -> SparseStateTrie<ParallelSparseTrie, ParallelSparseTrie> {
    let mut trie = SparseStateTrie::default()
        .with_default_storage_trie(RevealableSparseTrie::revealed_empty());
    let provider_factory = DefaultTrieNodeProviderFactory;
    let mut rng = StdRng::seed_from_u64(0x5eed);

    for _ in 0..storage_tries {
        let address = random_b256(&mut rng);
        let storage_trie = RevealableSparseTrie::revealed_empty();
        trie.insert_storage_trie(address, storage_trie);

        for _ in 0..64 {
            let slot = random_b256(&mut rng);
            let value_len = rng.random_range(8..64);
            let value = vec![0u8; value_len];
            trie.update_storage_leaf(address, Nibbles::unpack(slot), value, &provider_factory).ok();
        }
    }

    // Compute hashes once so pruning does real work.
    let _ = trie.root(&provider_factory);
    trie
}

fn random_b256(rng: &mut StdRng) -> alloy_primitives::B256 {
    let mut bytes = [0u8; 32];
    rng.fill(&mut bytes);
    alloy_primitives::B256::from(bytes)
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = storage_prune_benchmark
}
criterion_main!(benches);
