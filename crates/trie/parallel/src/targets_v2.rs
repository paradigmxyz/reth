//! V2 multiproof targets and chunking.

use alloy_primitives::{map::B256Map, B256};
use reth_trie::proof_v2;

/// A set of account and storage V2 proof targets. The account and storage targets do not need to
/// necessarily overlap.
#[derive(Debug, Default)]
pub struct MultiProofTargetsV2 {
    /// The set of account proof targets to generate proofs for.
    pub account_targets: Vec<proof_v2::Target>,
    /// The sets of storage proof targets to generate proofs for.
    pub storage_targets: B256Map<Vec<proof_v2::Target>>,
}

impl MultiProofTargetsV2 {
    /// Returns true is there are no account or storage targets.
    pub fn is_empty(&self) -> bool {
        self.account_targets.is_empty() && self.storage_targets.is_empty()
    }

    /// Returns the number of items that will be considered during chunking.
    pub fn chunking_length(&self) -> usize {
        self.account_targets.len() +
            self.storage_targets.values().map(|slots| slots.len()).sum::<usize>()
    }

    /// Returns an iterator that yields chunks of the specified size.
    pub fn chunks(self, chunk_size: usize) -> impl Iterator<Item = Self> {
        ChunkedMultiProofTargetsV2::new(self, chunk_size)
    }
}

/// An iterator that yields chunks of V2 proof targets of at most `size` account and storage
/// targets.
///
/// Unlike legacy chunking, V2 preserves account targets exactly as they were (with their `min_len`
/// metadata). Account targets must appear in a chunk. Storage targets for those accounts are
/// chunked together, but if they exceed the chunk size, subsequent chunks contain only the
/// remaining storage targets without repeating the account target.
#[derive(Debug)]
pub struct ChunkedMultiProofTargetsV2 {
    /// Remaining account targets to process
    account_targets: std::vec::IntoIter<proof_v2::Target>,
    /// Storage targets by account address
    storage_targets: B256Map<Vec<proof_v2::Target>>,
    /// Current account being processed (if any storage slots remain)
    current_account_storage: Option<(B256, std::vec::IntoIter<proof_v2::Target>)>,
    /// Chunk size
    size: usize,
}

impl ChunkedMultiProofTargetsV2 {
    /// Creates a new chunked iterator for the given targets.
    pub fn new(targets: MultiProofTargetsV2, size: usize) -> Self {
        Self {
            account_targets: targets.account_targets.into_iter(),
            storage_targets: targets.storage_targets,
            current_account_storage: None,
            size,
        }
    }
}

impl Iterator for ChunkedMultiProofTargetsV2 {
    type Item = MultiProofTargetsV2;

    fn next(&mut self) -> Option<Self::Item> {
        let mut chunk = MultiProofTargetsV2::default();
        let mut count = 0;

        // First, finish any remaining storage slots from previous account
        if let Some((account_addr, ref mut storage_iter)) = self.current_account_storage {
            let remaining_capacity = self.size - count;
            let slots: Vec<_> = storage_iter.by_ref().take(remaining_capacity).collect();

            count += slots.len();
            chunk.storage_targets.insert(account_addr, slots);

            // If iterator is exhausted, clear current_account_storage
            if storage_iter.len() == 0 {
                self.current_account_storage = None;
            }
        }

        // Process account targets and their storage
        while count < self.size {
            let Some(account_target) = self.account_targets.next() else {
                break;
            };

            // Add the account target
            chunk.account_targets.push(account_target);
            count += 1;

            // Check if this account has storage targets
            let account_addr = account_target.key();
            if let Some(storage_slots) = self.storage_targets.remove(&account_addr) {
                let remaining_capacity = self.size - count;

                if storage_slots.len() <= remaining_capacity {
                    // Optimization: We can take all slots, just move the vec
                    count += storage_slots.len();
                    chunk.storage_targets.insert(account_addr, storage_slots);
                } else {
                    // We need to split the storage slots
                    let mut storage_iter = storage_slots.into_iter();
                    let slots_in_chunk: Vec<_> =
                        storage_iter.by_ref().take(remaining_capacity).collect();
                    count += slots_in_chunk.len();

                    chunk.storage_targets.insert(account_addr, slots_in_chunk);

                    // Save remaining storage slots for next chunk
                    self.current_account_storage = Some((account_addr, storage_iter));
                    break;
                }
            }
        }

        // Process any remaining storage-only entries (accounts not in account_targets)
        while let Some((account_addr, storage_slots)) = self.storage_targets.iter_mut().next() &&
            count < self.size
        {
            let account_addr = *account_addr;
            let storage_slots = std::mem::take(storage_slots);
            let remaining_capacity = self.size - count;

            // Always remove from the map - if there are remaining slots they go to
            // current_account_storage
            self.storage_targets.remove(&account_addr);

            if storage_slots.len() <= remaining_capacity {
                // Optimization: We can take all slots, just move the vec
                count += storage_slots.len();
                chunk.storage_targets.insert(account_addr, storage_slots);
            } else {
                // We need to split the storage slots
                let mut storage_iter = storage_slots.into_iter();
                let slots_in_chunk: Vec<_> =
                    storage_iter.by_ref().take(remaining_capacity).collect();

                chunk.storage_targets.insert(account_addr, slots_in_chunk);

                // Save remaining storage slots for next chunk
                if storage_iter.len() > 0 {
                    self.current_account_storage = Some((account_addr, storage_iter));
                }
                break;
            }
        }

        if chunk.account_targets.is_empty() && chunk.storage_targets.is_empty() {
            None
        } else {
            Some(chunk)
        }
    }
}
