use crate::{
    hashed_cursor::{HashedCursor, HashedCursorFactory},
    proof::Proof,
    trie_cursor::TrieCursorFactory,
    TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use alloy_primitives::{
    keccak256,
    map::{B256Map, HashMap},
    Bytes, B256, U256,
};
use alloy_rlp::{Decodable, Encodable, EMPTY_STRING_CODE};
use alloy_trie::EMPTY_ROOT_HASH;
use reth_execution_errors::{SparseStateTrieErrorKind, StateProofError, TrieWitnessError};
use reth_trie_common::{
    DecodedMultiProofV2, HashedPostState, MultiProofTargetsV2, ProofV2Target, TrieAccount,
};
use reth_trie_sparse::{LeafUpdate, SparseStateTrie, SparseTrie as _};

/// State transition witness for the trie.
#[derive(Debug)]
pub struct TrieWitness<T, H> {
    /// The cursor factory for traversing trie nodes.
    trie_cursor_factory: T,
    /// The factory for hashed cursors.
    hashed_cursor_factory: H,
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
            always_include_root_node: false,
            witness: HashMap::default(),
        }
    }

    /// Set the trie cursor factory.
    pub fn with_trie_cursor_factory<TF>(self, trie_cursor_factory: TF) -> TrieWitness<TF, H> {
        TrieWitness {
            trie_cursor_factory,
            hashed_cursor_factory: self.hashed_cursor_factory,
            always_include_root_node: self.always_include_root_node,
            witness: self.witness,
        }
    }

    /// Set the hashed cursor factory.
    pub fn with_hashed_cursor_factory<HF>(self, hashed_cursor_factory: HF) -> TrieWitness<T, HF> {
        TrieWitness {
            trie_cursor_factory: self.trie_cursor_factory,
            hashed_cursor_factory,
            always_include_root_node: self.always_include_root_node,
            witness: self.witness,
        }
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
    pub fn compute(
        mut self,
        mut state: HashedPostState,
    ) -> Result<B256Map<Bytes>, TrieWitnessError> {
        let is_state_empty = state.is_empty();
        if is_state_empty && !self.always_include_root_node {
            return Ok(Default::default())
        }

        // Expand wiped storages into explicit zero-value entries for every existing slot,
        // so that downstream code can treat all storages uniformly.
        self.expand_wiped_storages(&mut state)?;

        let proof_targets = if is_state_empty {
            MultiProofTargetsV2 {
                account_targets: vec![ProofV2Target::new(B256::ZERO)],
                ..Default::default()
            }
        } else {
            Self::get_proof_targets(&state)
        };
        let multiproof =
            Proof::new(self.trie_cursor_factory.clone(), self.hashed_cursor_factory.clone())
                .multiproof_v2(proof_targets)?;

        // No need to reconstruct the rest of the trie, we just need to include
        // the root node and return.
        if is_state_empty {
            let (root_hash, root_node) = if let Some(root_node) =
                multiproof.account_proofs.into_iter().find(|n| n.path.is_empty())
            {
                let mut encoded = Vec::new();
                root_node.node.encode(&mut encoded);
                let bytes = Bytes::from(encoded);
                (keccak256(&bytes), bytes)
            } else {
                (EMPTY_ROOT_HASH, Bytes::from([EMPTY_STRING_CODE]))
            };
            return Ok(B256Map::from_iter([(root_hash, root_node)]))
        }

        // Record all nodes from multiproof in the witness.
        self.record_decoded_multiproof_v2(&multiproof);

        let mut sparse_trie = SparseStateTrie::new();
        sparse_trie.reveal_decoded_multiproof_v2(multiproof)?;

        // Build storage leaf updates for all accounts with storage changes.
        let mut storage_updates: B256Map<B256Map<LeafUpdate>> = B256Map::default();
        for (hashed_address, storage) in &state.storages {
            let slot_updates = storage
                .storage
                .iter()
                .map(|(&hashed_slot, value)| {
                    if value.is_zero() {
                        (hashed_slot, LeafUpdate::Changed(vec![]))
                    } else {
                        (
                            hashed_slot,
                            LeafUpdate::Changed(alloy_rlp::encode_fixed_size(value).to_vec()),
                        )
                    }
                })
                .collect();
            storage_updates.insert(*hashed_address, slot_updates);
        }

        // Apply storage updates, fetching additional proofs as needed.
        loop {
            let mut targets = MultiProofTargetsV2::default();

            for (&hashed_address, slot_updates) in &mut storage_updates {
                if slot_updates.is_empty() {
                    continue;
                }
                let storage_trie = sparse_trie
                    .storage_trie_mut(&hashed_address)
                    .expect("storage trie was revealed from multiproof");
                storage_trie
                    .update_leaves(slot_updates, |key, min_len| {
                        targets
                            .storage_targets
                            .entry(hashed_address)
                            .or_default()
                            .push(ProofV2Target::new(key).with_min_len(min_len));
                    })
                    .map_err(|err| {
                        SparseStateTrieErrorKind::SparseStorageTrie(hashed_address, err.into_kind())
                    })?;
            }

            if targets.is_empty() {
                break;
            }

            let multiproof =
                Proof::new(self.trie_cursor_factory.clone(), self.hashed_cursor_factory.clone())
                    .multiproof_v2(targets)?;
            self.record_decoded_multiproof_v2(&multiproof);
            sparse_trie.reveal_decoded_multiproof_v2(multiproof)?;
        }

        // Build account leaf updates.
        let mut account_updates: B256Map<LeafUpdate> = B256Map::default();
        for &hashed_address in state.accounts.keys().chain(state.storages.keys()) {
            if account_updates.contains_key(&hashed_address) {
                continue;
            }

            let account = state
                .accounts
                .get(&hashed_address)
                .ok_or(TrieWitnessError::MissingAccount(hashed_address))?
                .unwrap_or_default();

            let storage_root =
                if let Some(storage_trie) = sparse_trie.storage_trie_mut(&hashed_address) {
                    storage_trie.root()
                } else if sparse_trie.is_account_revealed(hashed_address) {
                    if let Some(value) = sparse_trie.get_account_value(&hashed_address) {
                        TrieAccount::decode(&mut &value[..])?.storage_root
                    } else {
                        EMPTY_ROOT_HASH
                    }
                } else {
                    EMPTY_ROOT_HASH
                };

            if account.is_empty() && storage_root == EMPTY_ROOT_HASH {
                account_updates.insert(hashed_address, LeafUpdate::Changed(vec![]));
            } else {
                let mut rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
                account.into_trie_account(storage_root).encode(&mut rlp);
                account_updates.insert(hashed_address, LeafUpdate::Changed(rlp));
            }
        }

        // Apply account updates, fetching additional proofs as needed.
        loop {
            let mut targets = MultiProofTargetsV2::default();

            sparse_trie
                .trie_mut()
                .update_leaves(&mut account_updates, |key, min_len| {
                    targets.account_targets.push(ProofV2Target::new(key).with_min_len(min_len));
                })
                .map_err(SparseStateTrieErrorKind::from)?;

            if targets.is_empty() {
                break;
            }

            let multiproof =
                Proof::new(self.trie_cursor_factory.clone(), self.hashed_cursor_factory.clone())
                    .multiproof_v2(targets)?;
            self.record_decoded_multiproof_v2(&multiproof);
            sparse_trie.reveal_decoded_multiproof_v2(multiproof)?;
        }

        Ok(self.witness)
    }

    /// Record all nodes from a V2 decoded multiproof in the witness.
    fn record_decoded_multiproof_v2(&mut self, multiproof: &DecodedMultiProofV2) {
        let mut encoded = Vec::new();
        for proof_node in &multiproof.account_proofs {
            encoded.clear();
            proof_node.node.encode(&mut encoded);
            let bytes = Bytes::from(encoded.clone());
            self.witness.entry(keccak256(&bytes)).or_insert(bytes);
        }
        for proof_node in multiproof.storage_proofs.values().flatten() {
            encoded.clear();
            proof_node.node.encode(&mut encoded);
            let bytes = Bytes::from(encoded.clone());
            self.witness.entry(keccak256(&bytes)).or_insert(bytes);
        }
    }

    /// Expand wiped storages into explicit zero-value entries for every existing slot in the
    /// database. After this, all storages can be treated uniformly without special wiped handling.
    fn expand_wiped_storages(&self, state: &mut HashedPostState) -> Result<(), StateProofError> {
        for (hashed_address, storage) in &mut state.storages {
            if !storage.wiped {
                continue;
            }
            let mut storage_cursor =
                self.hashed_cursor_factory.hashed_storage_cursor(*hashed_address)?;
            let mut current_entry = storage_cursor.seek(B256::ZERO)?;
            while let Some((hashed_slot, _)) = current_entry {
                storage.storage.entry(hashed_slot).or_insert(U256::ZERO);
                current_entry = storage_cursor.next()?;
            }
            storage.wiped = false;
        }
        Ok(())
    }

    /// Retrieve proof targets for incoming hashed state.
    /// Aggregates all accounts and slots present in the state. Wiped storages must have been
    /// expanded via [`Self::expand_wiped_storages`] before calling this.
    fn get_proof_targets(state: &HashedPostState) -> MultiProofTargetsV2 {
        let mut targets = MultiProofTargetsV2::default();
        for &hashed_address in state.accounts.keys() {
            targets.account_targets.push(ProofV2Target::new(hashed_address));
        }
        for (&hashed_address, storage) in &state.storages {
            if !state.accounts.contains_key(&hashed_address) {
                targets.account_targets.push(ProofV2Target::new(hashed_address));
            }
            let storage_keys = storage.storage.keys().map(|k| ProofV2Target::new(*k)).collect();
            targets.storage_targets.insert(hashed_address, storage_keys);
        }
        targets
    }
}
