//! V2 proof targets and chunking.

use crate::Nibbles;
use alloc::vec::Vec;
use alloy_primitives::{keccak256, map::B256Map, B256};
use revm::state::EvmState;

/// Target describes a proof target. For every proof target given, a proof calculator will calculate
/// and return all nodes whose path is a prefix of the target's `key_nibbles`.
#[derive(Debug, Copy, Clone)]
pub struct ProofV2Target {
    /// The key of the proof target, as nibbles.
    pub key_nibbles: Nibbles,
    /// The minimum length of a node's path for it to be retained.
    pub min_len: u8,
}

impl ProofV2Target {
    /// Returns a new [`ProofV2Target`] which matches all trie nodes whose path is a prefix of this
    /// key.
    pub fn new(key: B256) -> Self {
        // SAFETY: key is a B256 and so is exactly 32-bytes.
        let key_nibbles = unsafe { Nibbles::unpack_unchecked(key.as_slice()) };
        Self { key_nibbles, min_len: 0 }
    }

    /// Returns the key the target was initialized with.
    pub fn key(&self) -> B256 {
        B256::from_slice(&self.key_nibbles.pack())
    }

    /// Only match trie nodes whose path is at least this long.
    ///
    /// # Panics
    ///
    /// This method panics if `min_len` is greater than 64.
    pub fn with_min_len(mut self, min_len: u8) -> Self {
        debug_assert!(min_len <= 64);
        self.min_len = min_len;
        self
    }
}

impl From<B256> for ProofV2Target {
    fn from(key: B256) -> Self {
        Self::new(key)
    }
}

/// A set of account and storage V2 proof targets. The account and storage targets do not need to
/// necessarily overlap.
#[derive(Clone, Debug, Default)]
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

    /// Returns a set of [`MultiProofTargetsV2`] and the total amount of storage targets, based on
    /// the given state.
    pub fn from_state(state: EvmState) -> (Self, usize) {
        Self::from_state_ref(&state)
    }

    /// Returns proof targets for changed accounts and storage slots without consuming `state`.
    pub fn from_state_ref(state: &EvmState) -> (Self, usize) {
        let mut targets = Self::default();
        targets.account_targets.reserve(state.len());
        targets.storage_targets.reserve(state.len());
        let mut storage_target_count = 0;
        for (addr, account) in state {
            // if the account was not touched, or if the account was selfdestructed, do not
            // fetch proofs for it
            //
            // Since selfdestruct can only happen in the same transaction, we can skip
            // prefetching proofs for selfdestructed accounts
            //
            // See: https://eips.ethereum.org/EIPS/eip-6780
            if !account.is_touched() || account.is_selfdestructed() {
                continue
            }

            let hashed_address = keccak256(addr);

            if account.info != account.original_info() {
                targets.account_targets.push(hashed_address.into());
            }

            let mut storage_slots = Vec::with_capacity(account.storage.len());
            for (key, slot) in &account.storage {
                // do nothing if unchanged
                if !slot.is_changed() {
                    continue
                }

                let hashed_slot = keccak256(B256::new(key.to_be_bytes()));
                storage_slots.push(ProofV2Target::from(hashed_slot));
            }

            storage_target_count += storage_slots.len();
            if !storage_slots.is_empty() {
                targets.storage_targets.insert(hashed_address, storage_slots);
            }
        }

        (targets, storage_target_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};
    use revm::state::{Account, EvmStorageSlot, TransactionId};

    #[test]
    fn from_state_ref_returns_changed_targets_without_consuming_state() {
        let changed_account_address = Address::with_last_byte(1);
        let changed_storage_address = Address::with_last_byte(2);
        let unchanged_address = Address::with_last_byte(3);
        let selfdestructed_address = Address::with_last_byte(4);
        let changed_slot = U256::from(1);
        let unchanged_slot = U256::from(2);

        let mut changed_account = Account::default();
        changed_account.caller_initial_modification(U256::from(1), false);

        let changed_storage = Account::default().with_touched_mark().with_storage(
            [
                (
                    changed_slot,
                    EvmStorageSlot::new_changed(U256::ZERO, U256::from(1), TransactionId::ZERO),
                ),
                (unchanged_slot, EvmStorageSlot::new(U256::from(2), TransactionId::ZERO)),
            ]
            .into_iter(),
        );

        let mut selfdestructed = Account::default();
        selfdestructed.caller_initial_modification(U256::from(1), false);
        selfdestructed.mark_selfdestruct();

        let mut state = EvmState::default();
        state.insert(changed_account_address, changed_account);
        state.insert(changed_storage_address, changed_storage);
        state.insert(unchanged_address, Account::default().with_touched_mark());
        state.insert(selfdestructed_address, selfdestructed);
        let original_state = state.clone();

        let (targets, storage_target_count) = MultiProofTargetsV2::from_state_ref(&state);

        assert_eq!(state, original_state);
        assert_eq!(storage_target_count, 1);
        assert_eq!(targets.account_targets.len(), 1);
        assert_eq!(targets.account_targets[0].key(), keccak256(changed_account_address));
        assert_eq!(targets.storage_targets.len(), 1);

        let hashed_storage_address = keccak256(changed_storage_address);
        let storage_targets = targets.storage_targets.get(&hashed_storage_address).unwrap();
        assert_eq!(storage_targets.len(), 1);
        assert_eq!(storage_targets[0].key(), keccak256(B256::new(changed_slot.to_be_bytes())));
    }

    #[test]
    fn from_state_delegates_to_borrowed_constructor() {
        let address = Address::with_last_byte(1);
        let slot = U256::from(1);
        let account = Account::default().with_touched_mark().with_storage(std::iter::once((
            slot,
            EvmStorageSlot::new_changed(U256::ZERO, U256::from(1), TransactionId::ZERO),
        )));
        let state = EvmState::from_iter([(address, account)]);

        let (borrowed, borrowed_storage_count) = MultiProofTargetsV2::from_state_ref(&state);
        let (owned, owned_storage_count) = MultiProofTargetsV2::from_state(state);

        assert_eq!(borrowed_storage_count, owned_storage_count);
        assert_eq!(borrowed.account_targets.len(), owned.account_targets.len());
        assert_eq!(borrowed.storage_targets.len(), owned.storage_targets.len());
        assert_eq!(
            borrowed.storage_targets[&keccak256(address)][0].key(),
            owned.storage_targets[&keccak256(address)][0].key()
        );
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
