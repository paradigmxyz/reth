//! Profiling binary for prune() - run with `samply record`
//!
//! cargo build --release -p reth-trie-sparse-parallel --example profile_prune
//! samply record ./target/release/examples/profile_prune serial
//! samply record ./target/release/examples/profile_prune parallel

use alloy_primitives::U256;
use alloy_rlp::Encodable;
use alloy_trie::EMPTY_ROOT_HASH;
use reth_primitives_traits::Account;
use reth_trie_common::Nibbles;
use reth_trie_sparse::{provider::DefaultTrieNodeProvider, SparseTrie, SparseTrieExt};
use reth_trie_sparse_parallel::{ParallelSparseTrie, ParallelismThresholds};
use std::hint::black_box;

fn large_account_value() -> Vec<u8> {
    let account = Account {
        nonce: 0x123456789abcdef,
        balance: U256::from(0x123456789abcdef0123456789abcdef_u128),
        ..Default::default()
    };
    let mut buf = Vec::new();
    account.into_trie_account(EMPTY_ROOT_HASH).encode(&mut buf);
    buf
}

fn build_trie(num_lower_subtries: usize, min_prune_subtries: usize) -> ParallelSparseTrie {
    let provider = DefaultTrieNodeProvider;
    let value = large_account_value();

    let thresholds =
        ParallelismThresholds { min_revealed_nodes: 0, min_updated_nodes: 0, min_prune_subtries };

    let mut trie = ParallelSparseTrie::default().with_parallelism_thresholds(thresholds);

    for subtrie_idx in 0..num_lower_subtries {
        let first_nibble = (subtrie_idx / 16) as u8;
        let second_nibble = (subtrie_idx % 16) as u8;

        for leaf_idx in 0..4u8 {
            let path = Nibbles::from_nibbles([
                first_nibble,
                second_nibble,
                leaf_idx,
                0x3,
                0x4,
                0x5,
                0x6,
                0x7,
            ]);
            trie.update_leaf(path, value.clone(), &provider).unwrap();
        }
    }

    trie.root();
    trie
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("serial");

    let num_subtries = 64;
    let iterations = 100_000;

    let min_prune_subtries = match mode {
        "serial" => usize::MAX,
        "parallel" => 1,
        _ => {
            eprintln!("Usage: {} [serial|parallel]", args[0]);
            std::process::exit(1);
        }
    };

    eprintln!("Building base trie with {} subtries...", num_subtries);
    let base_trie = build_trie(num_subtries, min_prune_subtries);

    eprintln!("Pre-cloning {} tries for {} mode...", iterations, mode);
    let mut tries: Vec<_> = (0..iterations).map(|_| base_trie.clone()).collect();

    eprintln!("Running {} prune iterations ({} mode)...", iterations, mode);
    let start = std::time::Instant::now();

    let mut total_pruned = 0;
    for trie in &mut tries {
        total_pruned += trie.prune(2);
        black_box(total_pruned);
    }

    let elapsed = start.elapsed();
    eprintln!(
        "Done: {} iterations in {:?} ({:?}/iter), pruned {} total nodes",
        iterations,
        elapsed,
        elapsed / iterations as u32,
        total_pruned
    );
}
