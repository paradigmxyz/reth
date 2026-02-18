use crate::{
    hashed_cursor::{HashedCursor, HashedCursorFactory},
    prefix_set::TriePrefixSetsMut,
    proof::Proof,
    trie_cursor::TrieCursorFactory,
};
use alloy_rlp::EMPTY_STRING_CODE;
use alloy_trie::EMPTY_ROOT_HASH;
use reth_trie_common::HashedPostState;
use reth_trie_sparse::{LeafUpdate, SparseTrie};

use alloy_primitives::{
    keccak256,
    map::{B256Map, B256Set, Entry, HashMap},
    Bytes, B256,
};
use itertools::Itertools;
use reth_execution_errors::{
    SparseStateTrieErrorKind, SparseTrieErrorKind, StateProofError, TrieWitnessError,
};
use reth_trie_common::{MultiProofTargets, Nibbles};
use reth_trie_sparse::SparseStateTrie;

/// State transition witness for the trie.
#[derive(Debug)]
pub struct TrieWitness<T, H> {
    /// The cursor factory for traversing trie nodes.
    trie_cursor_factory: T,
    /// The factory for hashed cursors.
    hashed_cursor_factory: H,
    /// A set of prefix sets that have changes.
    prefix_sets: TriePrefixSetsMut,
    /// Flag indicating whether the root node should always be included (even if the target state
    /// is empty). This setting is useful if the caller wants to verify the witness against the
    /// parent state root.
    /// Set to `false` by default.
    always_include_root_node: bool,
    /// Recorded witness.
    witness: B256Map<Bytes>,
}

impl<T, H> TrieWitness<T, H> {
    /// Creates a new witness generator.
    pub fn new(trie_cursor_factory: T, hashed_cursor_factory: H) -> Self {
        Self {
            trie_cursor_factory,
            hashed_cursor_factory,
            prefix_sets: TriePrefixSetsMut::default(),
            always_include_root_node: false,
            witness: HashMap::default(),
        }
    }

    /// Set the trie cursor factory.
    pub fn with_trie_cursor_factory<TF>(self, trie_cursor_factory: TF) -> TrieWitness<TF, H> {
        TrieWitness {
            trie_cursor_factory,
            hashed_cursor_factory: self.hashed_cursor_factory,
            prefix_sets: self.prefix_sets,
            always_include_root_node: self.always_include_root_node,
            witness: self.witness,
        }
    }

    /// Set the hashed cursor factory.
    pub fn with_hashed_cursor_factory<HF>(self, hashed_cursor_factory: HF) -> TrieWitness<T, HF> {
        TrieWitness {
            trie_cursor_factory: self.trie_cursor_factory,
            hashed_cursor_factory,
            prefix_sets: self.prefix_sets,
            always_include_root_node: self.always_include_root_node,
            witness: self.witness,
        }
    }

    /// Set the prefix sets. They have to be mutable in order to allow extension with proof target.
    pub fn with_prefix_sets_mut(mut self, prefix_sets: TriePrefixSetsMut) -> Self {
        self.prefix_sets = prefix_sets;
        self
    }

    /// Set `always_include_root_node` to true. Root node will be included even in empty state.
    /// This setting is useful if the caller wants to verify the witness against the
    /// parent state root.
    pub const fn always_include_root_node(mut self) -> Self {
        self.always_include_root_node = true;
        self
    }
}

impl<T, H> TrieWitness<T, H>
where
    T: TrieCursorFactory + Clone,
    H: HashedCursorFactory + Clone,
{
    /// Compute the state transition witness for the trie. Gather all required nodes
    /// to apply `state` on top of the current trie state.
    ///
    /// # Arguments
    ///
    /// `state` - state transition containing both modified and touched accounts and storage slots.
    pub fn compute(mut self, state: HashedPostState) -> Result<B256Map<Bytes>, TrieWitnessError> {
        let is_state_empty = state.is_empty();
        if is_state_empty && !self.always_include_root_node {
            return Ok(Default::default())
        }

        let proof_targets = if is_state_empty {
            MultiProofTargets::account(B256::ZERO)
        } else {
            self.get_proof_targets(&state)?
        };
        let prefix_sets = core::mem::take(&mut self.prefix_sets);
        let multiproof =
            Proof::new(self.trie_cursor_factory.clone(), self.hashed_cursor_factory.clone())
                .with_prefix_sets_mut(prefix_sets)
                .multiproof(proof_targets.clone())?;

        // No need to reconstruct the rest of the trie, we just need to include
        // the root node and return.
        if is_state_empty {
            let (root_hash, root_node) = if let Some(root_node) =
                multiproof.account_subtree.into_inner().remove(&Nibbles::default())
            {
                (keccak256(&root_node), root_node)
            } else {
                (EMPTY_ROOT_HASH, Bytes::from([EMPTY_STRING_CODE]))
            };
            return Ok(B256Map::from_iter([(root_hash, root_node)]))
        }

        // Record all nodes from multiproof in the witness
        for account_node in multiproof.account_subtree.values() {
            if let Entry::Vacant(entry) = self.witness.entry(keccak256(account_node.as_ref())) {
                entry.insert(account_node.clone());
            }
        }
        for storage_node in multiproof.storages.values().flat_map(|s| s.subtree.values()) {
            if let Entry::Vacant(entry) = self.witness.entry(keccak256(storage_node.as_ref())) {
                entry.insert(storage_node.clone());
            }
        }

        let mut sparse_trie = SparseStateTrie::new();
        sparse_trie.reveal_multiproof(multiproof)?;

        // Attempt to update state trie to gather additional information for the witness.
        for (hashed_address, hashed_slots) in
            proof_targets.into_iter().sorted_unstable_by_key(|(ha, _)| *ha)
        {
            // Update storage trie first.
            let storage = state.storages.get(&hashed_address);
            let storage_trie = sparse_trie.storage_trie_mut(&hashed_address).ok_or(
                SparseStateTrieErrorKind::SparseStorageTrie(
                    hashed_address,
                    SparseTrieErrorKind::Blind,
                ),
            )?;

            // Collect storage updates into a B256Map for batch update.
            let mut storage_updates = B256Map::default();
            for hashed_slot in hashed_slots.into_iter().sorted_unstable() {
                let maybe_leaf_value = storage
                    .and_then(|s| s.storage.get(&hashed_slot))
                    .filter(|v| !v.is_zero())
                    .map(|v| alloy_rlp::encode_fixed_size(v).to_vec());

                let value = maybe_leaf_value.unwrap_or_default();
                storage_updates.insert(hashed_slot, LeafUpdate::Changed(value));
            }

            storage_trie.update_leaves(&mut storage_updates, |_, _| {}).map_err(|err| {
                SparseStateTrieErrorKind::SparseStorageTrie(hashed_address, err.into_kind())
            })?;

            let account = state
                .accounts
                .get(&hashed_address)
                .ok_or(TrieWitnessError::MissingAccount(hashed_address))?
                .unwrap_or_default();

            // Update account leaf via update_leaves.
            let storage_root =
                if let Some(storage_trie) = sparse_trie.storage_trie_mut(&hashed_address) {
                    storage_trie.root()
                } else {
                    EMPTY_ROOT_HASH
                };

            if !account.is_empty() || storage_root != EMPTY_ROOT_HASH {
                let mut account_rlp = Vec::new();
                alloy_rlp::Encodable::encode(
                    &account.into_trie_account(storage_root),
                    &mut account_rlp,
                );
                let mut account_updates = B256Map::default();
                account_updates.insert(hashed_address, LeafUpdate::Changed(account_rlp));
                sparse_trie
                    .trie_mut()
                    .update_leaves(&mut account_updates, |_, _| {})
                    .map_err(SparseStateTrieErrorKind::from)?;
            } else {
                // Account is empty - remove it.
                let mut account_updates = B256Map::default();
                account_updates.insert(hashed_address, LeafUpdate::Changed(Vec::new()));
                sparse_trie
                    .trie_mut()
                    .update_leaves(&mut account_updates, |_, _| {})
                    .map_err(SparseStateTrieErrorKind::from)?;
            }
        }

        Ok(self.witness)
    }

    /// Retrieve proof targets for incoming hashed state.
    /// This method will aggregate all accounts and slots present in the hash state as well as
    /// select all existing slots from the database for the accounts that have been destroyed.
    fn get_proof_targets(
        &self,
        state: &HashedPostState,
    ) -> Result<MultiProofTargets, StateProofError> {
        let mut proof_targets = MultiProofTargets::default();
        for hashed_address in state.accounts.keys() {
            proof_targets.insert(*hashed_address, B256Set::default());
        }
        for (hashed_address, storage) in &state.storages {
            let mut storage_keys = storage.storage.keys().copied().collect::<B256Set>();
            if storage.wiped {
                // storage for this account was destroyed, gather all slots from the current state
                let mut storage_cursor =
                    self.hashed_cursor_factory.hashed_storage_cursor(*hashed_address)?;
                // position cursor at the start
                let mut current_entry = storage_cursor.seek(B256::ZERO)?;
                while let Some((hashed_slot, _)) = current_entry {
                    storage_keys.insert(hashed_slot);
                    current_entry = storage_cursor.next()?;
                }
            }
            proof_targets.insert(*hashed_address, storage_keys);
        }
        Ok(proof_targets)
    }
}
