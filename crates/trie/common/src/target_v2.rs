//! V2 proof targets and chunking.

use crate::Nibbles;
use alloc::vec::Vec;
use alloy_primitives::{map::B256Map, B256};

/// Target describes a proof target. For every proof target given, a proof calculator will calculate
/// and return all nodes whose path is a prefix of the target's `key_nibbles`.
#[derive(Debug, Copy, Clone)]
pub struct ProofV2Target {
    /// The key of the proof target, as nibbles.
    pub key_nibbles: Nibbles,
    /// The known-parent context for this target.
    pub parent: ProofV2TargetParent,
}

impl ProofV2Target {
    /// Returns a new [`ProofV2Target`] which matches all trie nodes whose path is a prefix of this
    /// key.
    pub fn new(key: B256) -> Self {
        // SAFETY: key is a B256 and so is exactly 32-bytes.
        let key_nibbles = unsafe { Nibbles::unpack_unchecked(key.as_slice()) };
        Self { key_nibbles, parent: ProofV2TargetParent::NONE }
    }

    /// Returns the key the target was initialized with.
    pub fn key(&self) -> B256 {
        B256::from_slice(&self.key_nibbles.pack())
    }

    /// Sets the already-revealed parent branch of this target.
    pub const fn with_parent(mut self, parent: ProofV2TargetParent) -> Self {
        self.parent = parent;
        self
    }
}

impl From<B256> for ProofV2Target {
    fn from(key: B256) -> Self {
        Self::new(key)
    }
}

/// The already-revealed parent branch of a [`ProofV2Target`].
///
/// [`Self::NONE`] indicates that no parent is known and the proof must include the actual trie
/// root. A known parent at path length `n` makes the proof start at its direct child at length
/// `n + 1`. In particular, a known parent at path length zero means the root branch is already
/// revealed, so the proof starts at one of its direct children. Known parent path lengths are
/// always less than 64.
///
/// Parent contexts are ordered from broadest to narrowest: [`Self::NONE`] precedes every known
/// parent, and known parents are ordered by path length.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProofV2TargetParent(Option<u8>);

impl ProofV2TargetParent {
    /// No parent branch is known, so the proof must include the actual trie root.
    pub const NONE: Self = Self(None);

    /// Returns a known parent branch with the given logical path length.
    ///
    /// # Panics
    ///
    /// Panics if `path_len` is greater than or equal to 64.
    pub const fn new(path_len: usize) -> Self {
        assert!(path_len < 64, "parent path length must be less than 64");
        Self(Some(path_len as u8))
    }

    /// Returns `true` if the parent branch is already known.
    pub const fn is_known(self) -> bool {
        self.0.is_some()
    }

    /// Returns the logical path length of the known parent branch, if any.
    pub const fn path_len(self) -> Option<usize> {
        match self.0 {
            Some(path_len) => Some(path_len as usize),
            None => None,
        }
    }

    /// Returns the path of the known parent branch for the target path, if any.
    pub fn path(self, mut target_path: Nibbles) -> Option<Nibbles> {
        target_path.truncate(self.path_len()?);
        Some(target_path)
    }
}

/// A set of account and storage V2 proof targets. The account and storage targets do not need to
/// necessarily overlap.
#[derive(Debug, Default)]
pub struct MultiProofTargetsV2 {
    /// The set of account proof targets to generate proofs for.
    pub account_targets: Vec<ProofV2Target>,
    /// The sets of storage proof targets to generate proofs for.
    pub storage_targets: B256Map<Vec<ProofV2Target>>,
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
/// Unlike legacy chunking, V2 preserves account targets exactly as they were (including their
/// parent metadata). Account targets must appear in a chunk. Storage targets for those accounts
/// are chunked together, but if they exceed the chunk size, subsequent chunks contain only the
/// remaining storage targets without repeating the account target.
#[derive(Debug)]
pub struct ChunkedMultiProofTargetsV2 {
    /// Remaining account targets to process
    account_targets: alloc::vec::IntoIter<ProofV2Target>,
    /// Storage targets by account address
    storage_targets: B256Map<Vec<ProofV2Target>>,
    /// Current account being processed (if any storage slots remain)
    current_account_storage: Option<(B256, alloc::vec::IntoIter<ProofV2Target>)>,
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
            let storage_slots = core::mem::take(storage_slots);
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
