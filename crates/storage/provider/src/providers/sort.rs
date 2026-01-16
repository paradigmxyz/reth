//! K-way merge utilities for sorted streams.

use alloy_primitives::B256;
use std::{cmp::Ordering, collections::BinaryHeap};

/// Entry in the k-way merge heap.
///
/// Orders by key (ascending) then `block_idx` (descending) so that for duplicate keys,
/// the value from the latest block is selected.
///
/// Stores key and value by reference, and owns the iterator to avoid external Vec storage.
struct KMergeEntry<'a, V, I> {
    key: &'a B256,
    value: &'a V,
    block_idx: usize,
    iter: I,
}

impl<V, I> Ord for KMergeEntry<'_, V, I> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Min-heap by key, then max by block_idx (latest block wins on ties)
        other.key.cmp(self.key).then_with(|| self.block_idx.cmp(&other.block_idx))
    }
}

impl<V, I> PartialOrd for KMergeEntry<'_, V, I> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<V, I> PartialEq for KMergeEntry<'_, V, I> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<V, I> Eq for KMergeEntry<'_, V, I> {}

/// K-way merge iterator yielding deduplicated `(&key, &value)` pairs in sorted order.
/// For duplicate keys, the value from the highest block index wins.
pub(crate) struct KMergeIter<'a, V, I> {
    heap: BinaryHeap<KMergeEntry<'a, V, I>>,
}

impl<'a, V, I: Iterator<Item = (&'a B256, &'a V)>> KMergeIter<'a, V, I> {
    pub(crate) fn new(sources: impl IntoIterator<Item = (usize, I)>) -> Self {
        let mut heap = BinaryHeap::new();
        for (block_idx, mut source) in sources {
            if let Some((key, value)) = source.next() {
                heap.push(KMergeEntry { key, value, block_idx, iter: source });
            }
        }
        Self { heap }
    }
}

impl<'a, V, I: Iterator<Item = (&'a B256, &'a V)>> Iterator for KMergeIter<'a, V, I> {
    type Item = (&'a B256, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        let mut entry = self.heap.pop()?;
        let key = entry.key;
        let value = entry.value;

        // Advance this source's iterator
        if let Some((next_key, next_value)) = entry.iter.next() {
            self.heap.push(KMergeEntry {
                key: next_key,
                value: next_value,
                block_idx: entry.block_idx,
                iter: entry.iter,
            });
        }

        // Drain duplicate keys (heap ordering guarantees first pop is the winner)
        while self.heap.peek().is_some_and(|e| e.key == key) {
            let mut next = self.heap.pop().unwrap();
            if let Some((next_key, next_value)) = next.iter.next() {
                self.heap.push(KMergeEntry {
                    key: next_key,
                    value: next_value,
                    block_idx: next.block_idx,
                    iter: next.iter,
                });
            }
        }

        Some((key, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{map::HashMap, U256};
    use reth_primitives_traits::Account;

    fn b256(n: u8) -> B256 {
        let mut bytes = [0u8; 32];
        bytes[31] = n;
        B256::from(bytes)
    }

    fn account(nonce: u64, balance: u64) -> Option<Account> {
        Some(Account { nonce, balance: U256::from(balance), bytecode_hash: None })
    }

    fn sequential_apply<V: Clone>(blocks: &[Vec<(B256, V)>]) -> Vec<(B256, V)> {
        let mut state: HashMap<B256, V> = HashMap::default();
        for block in blocks {
            for (k, v) in block {
                state.insert(*k, v.clone());
            }
        }
        let mut result: Vec<_> = state.into_iter().collect();
        result.sort_by_key(|(k, _)| *k);
        result
    }

    fn kmerge_apply<V: Clone>(blocks: &[Vec<(B256, V)>]) -> Vec<(B256, V)> {
        let mut sorted_blocks: Vec<Vec<(B256, V)>> = blocks.to_vec();
        for block in &mut sorted_blocks {
            block.sort_by_key(|(k, _)| *k);
        }

        let sources = sorted_blocks
            .iter()
            .enumerate()
            .map(|(idx, block)| (idx, block.iter().map(|(k, v)| (k, v))));

        KMergeIter::new(sources).map(|(k, v)| (*k, v.clone())).collect()
    }

    #[test]
    fn accounts_single_block() {
        let blocks = vec![vec![
            (b256(3), account(1, 300)),
            (b256(1), account(1, 100)),
            (b256(2), account(1, 200)),
        ]];

        assert_eq!(sequential_apply(&blocks), kmerge_apply(&blocks));
    }

    #[test]
    fn accounts_multiple_blocks_no_overlap() {
        let blocks = vec![
            vec![(b256(1), account(1, 100)), (b256(3), account(1, 300))],
            vec![(b256(2), account(1, 200)), (b256(4), account(1, 400))],
            vec![(b256(5), account(1, 500))],
        ];

        assert_eq!(sequential_apply(&blocks), kmerge_apply(&blocks));
    }

    #[test]
    fn accounts_latest_block_wins() {
        let blocks = vec![
            vec![(b256(1), account(1, 100)), (b256(2), account(1, 200))],
            vec![(b256(1), account(2, 110)), (b256(3), account(1, 300))],
            vec![(b256(1), account(3, 120)), (b256(2), account(2, 220))],
        ];

        let merged = kmerge_apply(&blocks);
        assert_eq!(sequential_apply(&blocks), merged);
        assert_eq!(merged[0], (b256(1), account(3, 120)));
        assert_eq!(merged[1], (b256(2), account(2, 220)));
        assert_eq!(merged[2], (b256(3), account(1, 300)));
    }

    #[test]
    fn accounts_with_deletions() {
        let blocks = vec![
            vec![(b256(1), account(1, 100)), (b256(2), account(1, 200))],
            vec![(b256(1), None)], // delete account 1
            vec![(b256(2), None), (b256(3), account(1, 300))],
        ];

        let merged = kmerge_apply(&blocks);
        assert_eq!(sequential_apply(&blocks), merged);
        assert_eq!(merged[0], (b256(1), None));
        assert_eq!(merged[1], (b256(2), None));
        assert_eq!(merged[2], (b256(3), account(1, 300)));
    }

    #[test]
    fn storage_slots_latest_wins() {
        let blocks = vec![
            vec![(b256(1), U256::from(100)), (b256(2), U256::from(200))],
            vec![(b256(1), U256::from(110)), (b256(3), U256::from(300))],
            vec![(b256(1), U256::from(120)), (b256(2), U256::from(220))],
        ];

        let merged = kmerge_apply(&blocks);
        assert_eq!(sequential_apply(&blocks), merged);
        assert_eq!(merged[0], (b256(1), U256::from(120)));
        assert_eq!(merged[1], (b256(2), U256::from(220)));
        assert_eq!(merged[2], (b256(3), U256::from(300)));
    }

    #[test]
    fn storage_with_zero_values() {
        let blocks = vec![
            vec![(b256(1), U256::from(100)), (b256(2), U256::from(200))],
            vec![(b256(1), U256::ZERO)], // clear slot 1
            vec![(b256(2), U256::ZERO), (b256(3), U256::from(300))],
        ];

        let merged = kmerge_apply(&blocks);
        assert_eq!(sequential_apply(&blocks), merged);
        assert_eq!(merged[0], (b256(1), U256::ZERO));
        assert_eq!(merged[1], (b256(2), U256::ZERO));
        assert_eq!(merged[2], (b256(3), U256::from(300)));
    }

    #[test]
    fn empty_blocks() {
        let blocks: Vec<Vec<(B256, Option<Account>)>> = vec![vec![], vec![], vec![]];
        let merged = kmerge_apply(&blocks);
        assert!(merged.is_empty());
    }

    #[test]
    fn mixed_empty_and_nonempty() {
        let blocks = vec![
            vec![],
            vec![(b256(1), account(1, 100))],
            vec![],
            vec![(b256(2), account(1, 200)), (b256(1), account(2, 110))],
            vec![],
        ];

        assert_eq!(sequential_apply(&blocks), kmerge_apply(&blocks));
    }

    #[test]
    fn all_updates_same_key() {
        let blocks = vec![
            vec![(b256(1), account(1, 100))],
            vec![(b256(1), account(2, 200))],
            vec![(b256(1), account(3, 300))],
            vec![(b256(1), account(4, 400))],
        ];

        let merged = kmerge_apply(&blocks);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0], (b256(1), account(4, 400)));
    }

    #[test]
    fn output_is_sorted() {
        let blocks = vec![
            vec![(b256(5), account(1, 500)), (b256(1), account(1, 100))],
            vec![(b256(3), account(1, 300)), (b256(7), account(1, 700))],
            vec![(b256(2), account(1, 200)), (b256(6), account(1, 600))],
        ];

        let merged = kmerge_apply(&blocks);
        for window in merged.windows(2) {
            assert!(window[0].0 < window[1].0);
        }
    }

    #[test]
    fn three_sources_same_key_latest_wins() {
        // Block 2 should win over blocks 0 and 1
        let blocks = vec![
            vec![(b256(1), account(1, 100))],
            vec![(b256(1), account(2, 200))],
            vec![(b256(1), account(3, 300))],
        ];

        let merged = kmerge_apply(&blocks);
        assert_eq!(sequential_apply(&blocks), merged);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0], (b256(1), account(3, 300)));
    }

    #[test]
    fn non_contiguous_blocks_same_key() {
        // Blocks 0 and 2 have key 1, block 1 doesn't
        let blocks = vec![
            vec![(b256(1), account(1, 100)), (b256(2), account(1, 200))],
            vec![(b256(3), account(1, 300))], // no key 1
            vec![(b256(1), account(2, 150)), (b256(4), account(1, 400))],
        ];

        let merged = kmerge_apply(&blocks);
        assert_eq!(sequential_apply(&blocks), merged);
        assert_eq!(merged[0], (b256(1), account(2, 150))); // block 2 wins
    }

    #[test]
    fn many_sources_interleaved() {
        // 5 blocks with various overlapping keys
        let blocks = vec![
            vec![(b256(1), U256::from(10)), (b256(5), U256::from(50))],
            vec![(b256(2), U256::from(20)), (b256(5), U256::from(51))],
            vec![(b256(3), U256::from(30)), (b256(1), U256::from(11))],
            vec![(b256(4), U256::from(40)), (b256(2), U256::from(21))],
            vec![(b256(5), U256::from(52)), (b256(3), U256::from(31))],
        ];

        let merged = kmerge_apply(&blocks);
        assert_eq!(sequential_apply(&blocks), merged);
        // Verify specific winners
        assert_eq!(merged[0], (b256(1), U256::from(11))); // block 2
        assert_eq!(merged[1], (b256(2), U256::from(21))); // block 3
        assert_eq!(merged[2], (b256(3), U256::from(31))); // block 4
        assert_eq!(merged[3], (b256(4), U256::from(40))); // block 3 (only source)
        assert_eq!(merged[4], (b256(5), U256::from(52))); // block 4
    }

    #[test]
    fn middle_block_wins_not_last() {
        // Block 1 has key, blocks 0 and 2 don't - block 1 should win
        let blocks = vec![
            vec![(b256(1), account(1, 100))],
            vec![(b256(1), account(2, 200)), (b256(2), account(1, 250))],
            vec![(b256(2), account(2, 260))], // no key 1
        ];

        let merged = kmerge_apply(&blocks);
        assert_eq!(sequential_apply(&blocks), merged);
        assert_eq!(merged[0], (b256(1), account(2, 200))); // block 1 wins for key 1
        assert_eq!(merged[1], (b256(2), account(2, 260))); // block 2 wins for key 2
    }

    #[test]
    fn single_entry_per_block_many_blocks() {
        // Each block has exactly one unique key
        let blocks: Vec<Vec<(B256, U256)>> =
            (0..10).map(|i| vec![(b256(i), U256::from(i as u64 * 100))]).collect();

        let merged = kmerge_apply(&blocks);
        assert_eq!(sequential_apply(&blocks), merged);
        assert_eq!(merged.len(), 10);
        for (i, (key, value)) in merged.iter().enumerate() {
            assert_eq!(*key, b256(i as u8));
            assert_eq!(*value, U256::from(i as u64 * 100));
        }
    }

    #[test]
    fn alternating_updates_and_deletes() {
        let blocks = vec![
            vec![(b256(1), account(1, 100)), (b256(2), account(1, 200))],
            vec![(b256(1), None), (b256(3), account(1, 300))],
            vec![(b256(1), account(2, 150)), (b256(2), None)],
            vec![(b256(1), None)], // final delete
        ];

        let merged = kmerge_apply(&blocks);
        assert_eq!(sequential_apply(&blocks), merged);
        assert_eq!(merged[0], (b256(1), None)); // deleted in block 3
        assert_eq!(merged[1], (b256(2), None)); // deleted in block 2
        assert_eq!(merged[2], (b256(3), account(1, 300))); // only in block 1
    }

    #[test]
    fn first_pop_is_always_highest_block_idx() {
        // This test validates heap ordering: for same key, highest block_idx pops first.
        // Values encode their block_idx, so we can verify the winner.
        let blocks: Vec<Vec<(B256, U256)>> = (0..10)
            .map(|block_idx| {
                // Every block writes to key 1 with value = block_idx
                vec![(b256(1), U256::from(block_idx))]
            })
            .collect();

        let merged = kmerge_apply(&blocks);
        assert_eq!(merged.len(), 1);
        // Block 9 (highest) must win
        assert_eq!(merged[0], (b256(1), U256::from(9)));
    }

    #[test]
    fn heap_ordering_with_gaps_in_block_indices() {
        // Non-contiguous block indices: 0, 5, 3, 9, 2
        // Block 9 should win despite not being last in input order
        let blocks = vec![
            (0usize, vec![(b256(1), U256::from(100))]),
            (5, vec![(b256(1), U256::from(500))]),
            (3, vec![(b256(1), U256::from(300))]),
            (9, vec![(b256(1), U256::from(900))]),
            (2, vec![(b256(1), U256::from(200))]),
        ];

        let mut sorted_blocks: Vec<Vec<(B256, U256)>> =
            blocks.iter().map(|(_, b)| b.clone()).collect();
        for block in &mut sorted_blocks {
            block.sort_by_key(|(k, _)| *k);
        }

        // Use actual block indices, not enumerate
        let sources = blocks.iter().map(|(block_idx, block)| {
            let sorted: Vec<_> = {
                let mut b = block.clone();
                b.sort_by_key(|(k, _)| *k);
                b
            };
            (*block_idx, sorted)
        });

        let sources_with_iters: Vec<_> =
            sources.map(|(idx, block)| (idx, block.into_iter().collect::<Vec<_>>())).collect();

        let merged: Vec<_> = KMergeIter::new(
            sources_with_iters.iter().map(|(idx, block)| (*idx, block.iter().map(|(k, v)| (k, v)))),
        )
        .map(|(k, v)| (*k, *v))
        .collect();

        assert_eq!(merged.len(), 1);
        // Block 9 must win (highest block_idx)
        assert_eq!(merged[0], (b256(1), U256::from(900)));
    }

    #[test]
    fn value_from_first_pop_is_kept() {
        // 5 blocks all write to same key. Values are 1000 + block_idx.
        // If first pop wasn't the winner, we'd get wrong value.
        let blocks: Vec<Vec<(B256, U256)>> = vec![
            vec![(b256(1), U256::from(1000))], // block 0
            vec![(b256(1), U256::from(1001))], // block 1
            vec![(b256(1), U256::from(1002))], // block 2
            vec![(b256(1), U256::from(1003))], // block 3
            vec![(b256(1), U256::from(1004))], // block 4
        ];

        let merged = kmerge_apply(&blocks);

        // Must be 1004 (from block 4, highest index)
        assert_eq!(merged[0].1, U256::from(1004));

        // Also verify via sequential for sanity
        assert_eq!(sequential_apply(&blocks), merged);
    }
}
