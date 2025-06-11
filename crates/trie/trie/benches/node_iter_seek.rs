#![allow(missing_docs, unreachable_pub)]
use alloy_primitives::{map::B256Map, Address, B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use reth_primitives_traits::Account;
use reth_trie::{
    hashed_cursor::{noop::NoopHashedAccountCursor, HashedPostStateAccountCursor},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::PrefixSetMut,
    trie_cursor::noop::NoopAccountTrieCursor,
    walker::TrieWalker,
    BranchNodeCompact, HashedPostState, Nibbles, TrieMask,
};
use std::collections::BTreeMap;

pub fn node_iter_seek_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Node Iterator Seek Operations");
    group.sample_size(20);

    for size in [100, 500, 1_000, 5_000] {
        // Too slow for large sizes in CI
        #[expect(unexpected_cfgs)]
        if cfg!(codspeed) && size > 1_000 {
            continue;
        }

        let (state, trie_nodes) = generate_test_data(size);

        group.bench_function(BenchmarkId::new("sequential iteration", size), |b| {
            b.iter(|| {
                let walker = TrieWalker::state_trie(
                    MockAccountTrieCursor::new(trie_nodes.clone()),
                    PrefixSetMut::default().freeze(),
                );
                let hashed_cursor = HashedPostStateAccountCursor::new(
                    NoopHashedAccountCursor::default(),
                    state.accounts(),
                );
                let mut iter = TrieNodeIter::state_trie(walker, hashed_cursor);

                let mut count = 0;
                while let Some(_) = iter.try_next().unwrap() {
                    count += 1;
                }
                count
            })
        });

        // Simulate a pattern where we have frequent seeks to nearby keys
        group.bench_function(BenchmarkId::new("frequent nearby seeks", size), |b| {
            b.iter(|| {
                let mut prefix_set = PrefixSetMut::default();
                // Mark every 10th key as changed to force more seeks
                for (i, (key, _)) in state.accounts().iter().enumerate() {
                    if i % 10 == 0 {
                        prefix_set.insert(Nibbles::unpack(*key));
                    }
                }

                let walker = TrieWalker::state_trie(
                    MockAccountTrieCursor::new(trie_nodes.clone()),
                    prefix_set.freeze(),
                );
                let hashed_cursor = HashedPostStateAccountCursor::new(
                    NoopHashedAccountCursor::default(),
                    state.accounts(),
                );
                let mut iter = TrieNodeIter::state_trie(walker, hashed_cursor);

                let mut count = 0;
                while let Some(_) = iter.try_next().unwrap() {
                    count += 1;
                }
                count
            })
        });
    }
}

fn generate_test_data(size: usize) -> (HashedPostState, B256Map<BranchNodeCompact>) {
    let mut runner = TestRunner::deterministic();

    // Generate random accounts
    let mut accounts = BTreeMap::new();
    for i in 0..size {
        let address = Address::from_low_u64_be(i as u64);
        let hashed_address = B256::from(address);
        let account =
            Account { nonce: i as u64, balance: U256::from(i * 1000), bytecode_hash: None };
        accounts.insert(hashed_address, Some(account));
    }

    let state = HashedPostState::default().with_accounts(accounts.clone());

    // Create mock trie nodes
    let mut trie_nodes = B256Map::default();

    // Add some branch nodes to simulate a real trie structure
    for i in 0..(size / 10).max(1) {
        let key = Nibbles::from_nibbles(vec![i as u8 % 16; 4]);
        let node = BranchNodeCompact::new(
            TrieMask::new(0b1111),
            TrieMask::new(0b0011),
            TrieMask::new(0b1100),
            vec![B256::random(), B256::random()],
            None,
        );
        let mut key_bytes = key.pack();
        key_bytes.resize(32, 0);
        trie_nodes.insert(B256::from_slice(&key_bytes), node);
    }

    (state, trie_nodes)
}

// Mock cursor for testing
#[derive(Debug, Clone)]
struct MockAccountTrieCursor {
    nodes: B256Map<BranchNodeCompact>,
    position: Option<B256>,
}

impl MockAccountTrieCursor {
    fn new(nodes: B256Map<BranchNodeCompact>) -> Self {
        Self { nodes, position: None }
    }
}

impl reth_trie::trie_cursor::TrieCursor for MockAccountTrieCursor {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, reth_storage_errors::db::DatabaseError> {
        let mut key_bytes = key.pack();
        key_bytes.resize(32, 0);
        let key_b256 = B256::from_slice(&key_bytes);

        self.position = Some(key_b256);
        Ok(self.nodes.get(&key_b256).map(|node| (key.clone(), node.clone())))
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, reth_storage_errors::db::DatabaseError> {
        let mut key_bytes = key.pack();
        key_bytes.resize(32, 0);
        let seek_key = B256::from_slice(&key_bytes);

        // Find the first key >= seek_key
        let result = self.nodes.iter().find(|(k, _)| **k >= seek_key).map(|(k, v)| {
            self.position = Some(*k);
            let nibbles = Nibbles::unpack(*k);
            (nibbles, v.clone())
        });

        Ok(result)
    }

    fn next(
        &mut self,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, reth_storage_errors::db::DatabaseError> {
        if let Some(current) = self.position {
            // Find next key after current
            let result = self.nodes.iter().find(|(k, _)| **k > current).map(|(k, v)| {
                self.position = Some(*k);
                let nibbles = Nibbles::unpack(*k);
                (nibbles, v.clone())
            });
            Ok(result)
        } else {
            // Return first entry
            self.nodes
                .iter()
                .next()
                .map(|(k, v)| {
                    self.position = Some(*k);
                    let nibbles = Nibbles::unpack(*k);
                    Ok((nibbles, v.clone()))
                })
                .transpose()
        }
    }

    fn current(&mut self) -> Result<Option<Nibbles>, reth_storage_errors::db::DatabaseError> {
        Ok(self.position.map(|k| Nibbles::unpack(k)))
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = node_iter_seek_benchmark
}
criterion_main!(benches);

