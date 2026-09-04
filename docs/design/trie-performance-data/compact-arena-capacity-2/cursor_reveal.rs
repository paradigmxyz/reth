//! Temporary benchmark for revealing fresh storage proofs into a root-only sparse trie.
use alloy_primitives::{keccak256, B256, U256};
use reth_trie::test_utils::TrieTestHarness;
use reth_trie_common::ProofV2Target;
use reth_trie_sparse::{ArenaParallelSparseTrie, SparseTrie, TrieNodeEpoch};
use std::{collections::BTreeMap, hint::black_box, time::Instant};

fn main() {
    let samples: usize =
        std::env::var("MPT_BENCH_SAMPLES").map(|value| value.parse().unwrap()).unwrap_or(100);
    rayon::ThreadPoolBuilder::new().build_global().unwrap();
    println!("case,samples,median_ns,p25_ns,p75_ns,root");
    for size in [1000usize, 32768] {
        let keys: Vec<B256> = (0..size as u64).map(|i| keccak256(i.to_be_bytes())).collect();
        let storage: BTreeMap<_, _> = keys.iter().map(|key| (*key, U256::from(42))).collect();
        let harness = TrieTestHarness::new(storage);
        let root = harness.root_node();
        let mut base = ArenaParallelSparseTrie::default();
        base.set_root(root.node, root.masks, true).unwrap();
        for count in [1, size / 10, size] {
            let mut targets: Vec<_> =
                keys[..count].iter().copied().map(ProofV2Target::new).collect();
            let (nodes, _) = harness.proof_v2(&mut targets);
            let mut durations = Vec::with_capacity(samples + 5);
            for _ in 0..samples + 5 {
                let mut trie = base.clone();
                let mut nodes = nodes.clone();
                let start = Instant::now();
                black_box(trie.reveal_nodes(black_box(&mut nodes))).unwrap();
                durations.push(start.elapsed().as_nanos());
                assert_eq!(trie.root(TrieNodeEpoch::new(1)), harness.original_root());
                for key in &keys[..count] {
                    assert_eq!(
                        trie.get_leaf_value(&reth_trie_common::Nibbles::unpack(key)),
                        Some(&vec![42])
                    );
                }
            }
            durations.drain(..5);
            durations.sort_unstable();
            println!(
                "reveal/{size}/{count},{samples},{},{},{},{}",
                durations[samples / 2],
                durations[samples / 4],
                durations[samples * 3 / 4],
                harness.original_root()
            );
        }
    }
}
