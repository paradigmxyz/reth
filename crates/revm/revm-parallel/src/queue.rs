//! The lists representing the order of execution of the block.

use crate::rw_set::TransactionRWSet;
use derive_more::{Deref, DerefMut};
use reth_primitives::BlockNumber;
use std::{collections::HashMap, ops::IndexMut};

/// The batch of transaction indexes that can be executed in parallel.
/// Transaction index is the position of the transaction within the block.
#[derive(
    Deref, DerefMut, serde::Serialize, serde::Deserialize, PartialEq, Eq, Clone, Default, Debug,
)]
pub struct TransactionBatch(Vec<u32>);

/// The queue of transaction lists that represent the order of execution for the block.
#[derive(
    Deref, DerefMut, serde::Serialize, serde::Deserialize, PartialEq, Eq, Clone, Default, Debug,
)]
pub struct BlockQueue(Vec<TransactionBatch>);

impl<T> From<T> for BlockQueue
where
    T: IntoIterator<Item = Vec<u32>>,
{
    fn from(value: T) -> Self {
        Self(value.into_iter().map(TransactionBatch).collect())
    }
}

impl BlockQueue {
    /// Resolve block queue from an ordered list of transaction read write sets.
    pub fn resolve(sets: &[TransactionRWSet]) -> Self {
        let mut this = Self::default();

        for tx_index in 0..sets.len() {
            let depth = this.find_highest_dependency(tx_index, sets).map_or(0, |dep| dep + 1);
            this.insert_at(depth, tx_index as u32);
        }

        this
    }

    /// Find dependency with the highest index in the queue.
    /// Returns [None] if transaction is independent.
    ///
    /// # Panics
    ///
    /// - If the target index has no corresponding rw set.
    /// - If the block queue contains an index that has no corresponding rw set.
    pub fn find_highest_dependency(&self, idx: usize, sets: &[TransactionRWSet]) -> Option<usize> {
        // Iterate over the list in reverse to find dependency with the highest index.
        let target = &sets[idx];
        for (queue_depth, tx_list) in self.iter().enumerate().rev() {
            for tx_index in tx_list.iter() {
                let tx = &sets[*tx_index as usize];
                if target.depends_on(tx) || tx.depends_on(target) {
                    return Some(queue_depth)
                }
            }
        }

        None
    }

    /// Insert transaction index at depth or append it to the end of the queue.
    pub fn insert_at(&mut self, depth: usize, tx_index: u32) {
        if depth < self.0.len() {
            self.0.index_mut(depth).push(tx_index);
        } else {
            self.append_transaction(tx_index);
        }
    }

    /// Appends transaction as the separate list at the end of the queue.
    pub fn append_transaction(&mut self, tx_index: u32) {
        self.0.push(TransactionBatch(Vec::from([tx_index])))
    }
}

/// The collection of block queues by block number.
#[derive(Default, Debug)]
pub struct BlockQueueStore(HashMap<BlockNumber, BlockQueue>);

impl BlockQueueStore {
    /// Create new queue store.
    pub fn new(queues: HashMap<BlockNumber, BlockQueue>) -> Self {
        Self(queues)
    }

    /// Returns block queue for a given number.
    pub fn get_queue(&self, block: BlockNumber) -> Option<&BlockQueue> {
        self.0.get(&block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rw_set::{RevmAccessSet, RevmAccountDataKey, RevmKey};
    use reth_primitives::Address;

    #[test]
    #[should_panic]
    fn highest_dependency_target_out_of_bounds() {
        assert_eq!(BlockQueue::default().find_highest_dependency(0, &[]), None);
    }

    #[test]
    #[should_panic]
    fn highest_dependency_queue_item_out_of_bounds() {
        let mut queue = BlockQueue::default();
        queue.append_transaction(1);
        assert_eq!(queue.find_highest_dependency(0, &[TransactionRWSet::default()]), None);
    }

    #[test]
    fn highest_dependency() {
        let queue = BlockQueue::default();
        assert_eq!(queue.find_highest_dependency(0, &[TransactionRWSet::default()]), None);

        let account_balance_key = RevmKey::Account(Address::random(), RevmAccountDataKey::Balance);
        let sets = Vec::from([
            TransactionRWSet::default().with_write_set(RevmAccessSet::from([account_balance_key])),
            TransactionRWSet::default().with_read_set(RevmAccessSet::from([account_balance_key])),
        ]);
        let queue = BlockQueue::from([vec![0]]);
        assert_eq!(queue.find_highest_dependency(1, &sets), Some(0));
    }

    #[test]
    fn resolve() {
        let account_balance_key = RevmKey::Account(Address::random(), RevmAccountDataKey::Balance);
        let account_nonce_key = RevmKey::Account(Address::random(), RevmAccountDataKey::Nonce);
        let sets = Vec::from([
            // 0: first hence independent
            TransactionRWSet::default().with_write_set(RevmAccessSet::from([account_balance_key])),
            // 1: independent
            TransactionRWSet::default(),
            // 2: depends on 0
            TransactionRWSet::default()
                .with_read_set(RevmAccessSet::from([account_balance_key]))
                .with_write_set(RevmAccessSet::from([account_nonce_key])),
            // 3: independent
            TransactionRWSet::default(),
            // 4: depends on 0
            TransactionRWSet::default().with_read_set(RevmAccessSet::from([account_nonce_key])),
            // 5: depends on 0, 2
            TransactionRWSet::default()
                .with_read_set(RevmAccessSet::from([account_balance_key, account_nonce_key])),
        ]);

        // [0, 1, 3]
        // [2]
        // [4, 5]
        let expected_queue = BlockQueue::from([vec![0, 1, 3], vec![2], vec![4, 5]]);
        assert_eq!(BlockQueue::resolve(&sets), expected_queue);
    }
}
