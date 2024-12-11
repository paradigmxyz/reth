use crate::{
    hashed_cursor::{HashedCursor, HashedCursorFactory},
    prefix_set::TriePrefixSetsMut,
    proof::{Proof, ProofBlindedProviderFactory},
    trie_cursor::TrieCursorFactory,
    HashedPostState,
};
use alloy_primitives::{
    keccak256,
    map::{B256HashMap, B256HashSet, Entry, HashMap},
    Bytes, B256,
};
use itertools::Itertools;
use reth_execution_errors::{
    SparseStateTrieError, SparseStateTrieErrorKind, SparseTrieError, SparseTrieErrorKind,
    StateProofError, TrieWitnessError,
};
use reth_trie_common::Nibbles;
use reth_trie_sparse::{
    blinded::{BlindedProvider, BlindedProviderFactory},
    SparseStateTrie,
};
use std::sync::{mpsc, Arc};

/// State transition witness for the trie.
#[derive(Debug)]
pub struct TrieWitness<T, H> {
    /// The cursor factory for traversing trie nodes.
    trie_cursor_factory: T,
    /// The factory for hashed cursors.
    hashed_cursor_factory: H,
    /// A set of prefix sets that have changes.
    prefix_sets: TriePrefixSetsMut,
    /// Recorded witness.
    witness: B256HashMap<Bytes>,
}

impl<T, H> TrieWitness<T, H> {
    /// Creates a new witness generator.
    pub fn new(trie_cursor_factory: T, hashed_cursor_factory: H) -> Self {
        Self {
            trie_cursor_factory,
            hashed_cursor_factory,
            prefix_sets: TriePrefixSetsMut::default(),
            witness: HashMap::default(),
        }
    }

    /// Set the trie cursor factory.
    pub fn with_trie_cursor_factory<TF>(self, trie_cursor_factory: TF) -> TrieWitness<TF, H> {
        TrieWitness {
            trie_cursor_factory,
            hashed_cursor_factory: self.hashed_cursor_factory,
            prefix_sets: self.prefix_sets,
            witness: self.witness,
        }
    }

    /// Set the hashed cursor factory.
    pub fn with_hashed_cursor_factory<HF>(self, hashed_cursor_factory: HF) -> TrieWitness<T, HF> {
        TrieWitness {
            trie_cursor_factory: self.trie_cursor_factory,
            hashed_cursor_factory,
            prefix_sets: self.prefix_sets,
            witness: self.witness,
        }
    }

    /// Set the prefix sets. They have to be mutable in order to allow extension with proof target.
    pub fn with_prefix_sets_mut(mut self, prefix_sets: TriePrefixSetsMut) -> Self {
        self.prefix_sets = prefix_sets;
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
        state: HashedPostState,
    ) -> Result<B256HashMap<Bytes>, TrieWitnessError> {
        if state.is_empty() {
            return Ok(self.witness)
        }

        let proof_targets = self.get_proof_targets(&state)?;
        let multiproof =
            Proof::new(self.trie_cursor_factory.clone(), self.hashed_cursor_factory.clone())
                .with_prefix_sets_mut(self.prefix_sets.clone())
                .multiproof(proof_targets.clone())?;

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

        let (tx, rx) = mpsc::channel();
        let proof_provider_factory = ProofBlindedProviderFactory::new(
            self.trie_cursor_factory,
            self.hashed_cursor_factory,
            Arc::new(self.prefix_sets),
        );
        let mut sparse_trie =
            SparseStateTrie::new(WitnessBlindedProviderFactory::new(proof_provider_factory, tx));
        sparse_trie.reveal_multiproof(proof_targets.clone(), multiproof)?;

        // Attempt to update state trie to gather additional information for the witness.
        for (hashed_address, hashed_slots) in
            proof_targets.into_iter().sorted_unstable_by_key(|(ha, _)| *ha)
        {
            // Update storage trie first.
            let storage = state.storages.get(&hashed_address);
            let storage_trie = sparse_trie
                .storage_trie_mut(&hashed_address)
                .ok_or(SparseStateTrieErrorKind::Sparse(SparseTrieErrorKind::Blind))?;
            for hashed_slot in hashed_slots.into_iter().sorted_unstable() {
                let storage_nibbles = Nibbles::unpack(hashed_slot);
                let maybe_leaf_value = storage
                    .and_then(|s| s.storage.get(&hashed_slot))
                    .filter(|v| !v.is_zero())
                    .map(|v| alloy_rlp::encode_fixed_size(v).to_vec());

                if let Some(value) = maybe_leaf_value {
                    storage_trie
                        .update_leaf(storage_nibbles, value)
                        .map_err(SparseStateTrieError::from)?;
                } else {
                    storage_trie
                        .remove_leaf(&storage_nibbles)
                        .map_err(SparseStateTrieError::from)?;
                }
            }

            // Calculate storage root after updates.
            storage_trie.root();

            let account = state
                .accounts
                .get(&hashed_address)
                .ok_or(TrieWitnessError::MissingAccount(hashed_address))?
                .unwrap_or_default();
            sparse_trie.update_account(hashed_address, account)?;

            while let Ok(node) = rx.try_recv() {
                self.witness.insert(keccak256(&node), node);
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
    ) -> Result<B256HashMap<B256HashSet>, StateProofError> {
        let mut proof_targets = B256HashMap::default();
        for hashed_address in state.accounts.keys() {
            proof_targets.insert(*hashed_address, B256HashSet::default());
        }
        for (hashed_address, storage) in &state.storages {
            let mut storage_keys = storage.storage.keys().copied().collect::<B256HashSet>();
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

#[derive(Debug)]
struct WitnessBlindedProviderFactory<F> {
    /// Blinded node provider factory.
    provider_factory: F,
    /// Sender for forwarding fetched blinded node.
    tx: mpsc::Sender<Bytes>,
}

impl<F> WitnessBlindedProviderFactory<F> {
    const fn new(provider_factory: F, tx: mpsc::Sender<Bytes>) -> Self {
        Self { provider_factory, tx }
    }
}

impl<F> BlindedProviderFactory for WitnessBlindedProviderFactory<F>
where
    F: BlindedProviderFactory,
    F::AccountNodeProvider: BlindedProvider<Error = SparseTrieError>,
    F::StorageNodeProvider: BlindedProvider<Error = SparseTrieError>,
{
    type AccountNodeProvider = WitnessBlindedProvider<F::AccountNodeProvider>;
    type StorageNodeProvider = WitnessBlindedProvider<F::StorageNodeProvider>;

    fn account_node_provider(&self) -> Self::AccountNodeProvider {
        let provider = self.provider_factory.account_node_provider();
        WitnessBlindedProvider::new(provider, self.tx.clone())
    }

    fn storage_node_provider(&self, account: B256) -> Self::StorageNodeProvider {
        let provider = self.provider_factory.storage_node_provider(account);
        WitnessBlindedProvider::new(provider, self.tx.clone())
    }
}

#[derive(Debug)]
struct WitnessBlindedProvider<P> {
    /// Proof-based blinded.
    provider: P,
    /// Sender for forwarding fetched blinded node.
    tx: mpsc::Sender<Bytes>,
}

impl<P> WitnessBlindedProvider<P> {
    const fn new(provider: P, tx: mpsc::Sender<Bytes>) -> Self {
        Self { provider, tx }
    }
}

impl<P> BlindedProvider for WitnessBlindedProvider<P>
where
    P: BlindedProvider<Error = SparseTrieError>,
{
    type Error = P::Error;

    fn blinded_node(&mut self, path: &Nibbles) -> Result<Option<Bytes>, Self::Error> {
        let maybe_node = self.provider.blinded_node(path)?;
        if let Some(node) = &maybe_node {
            self.tx
                .send(node.clone())
                .map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?;
        }
        Ok(maybe_node)
    }
}
