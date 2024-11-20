use crate::{
    hashed_cursor::{HashedCursor, HashedCursorFactory},
    prefix_set::TriePrefixSetsMut,
    proof::{Proof, StorageProof},
    trie_cursor::TrieCursorFactory,
    HashedPostState, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use alloy_consensus::EMPTY_ROOT_HASH;
use alloy_primitives::{
    keccak256,
    map::{HashMap, HashSet},
    Bytes, B256,
};
use alloy_rlp::{BufMut, Decodable, Encodable};
use itertools::{Either, Itertools};
use reth_execution_errors::{StateProofError, TrieWitnessError};
use reth_trie_common::{
    BranchNode, HashBuilder, Nibbles, StorageMultiProof, TrieAccount, TrieNode, CHILD_INDEX_RANGE,
};
use std::collections::BTreeMap;

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
    witness: HashMap<B256, Bytes>,
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
    ) -> Result<HashMap<B256, Bytes>, TrieWitnessError> {
        if state.is_empty() {
            return Ok(self.witness)
        }

        let proof_targets = self.get_proof_targets(&state)?;
        let mut account_multiproof =
            Proof::new(self.trie_cursor_factory.clone(), self.hashed_cursor_factory.clone())
                .with_prefix_sets_mut(self.prefix_sets.clone())
                .multiproof(proof_targets.clone())?;

        // Attempt to compute state root from proofs and gather additional
        // information for the witness.
        let mut account_rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
        let mut account_trie_nodes = BTreeMap::default();
        for (hashed_address, hashed_slots) in proof_targets {
            let storage_multiproof = account_multiproof
                .storages
                .remove(&hashed_address)
                .unwrap_or_else(StorageMultiProof::empty);

            // Gather and record account trie nodes.
            let account = state
                .accounts
                .get(&hashed_address)
                .ok_or(TrieWitnessError::MissingAccount(hashed_address))?;
            let value =
                (account.is_some() || storage_multiproof.root != EMPTY_ROOT_HASH).then(|| {
                    account_rlp.clear();
                    TrieAccount::from((account.unwrap_or_default(), storage_multiproof.root))
                        .encode(&mut account_rlp as &mut dyn BufMut);
                    account_rlp.clone()
                });
            let key = Nibbles::unpack(hashed_address);
            account_trie_nodes.extend(target_nodes(
                key.clone(),
                value,
                Some(&mut self.witness),
                account_multiproof
                    .account_subtree
                    .matching_nodes_iter(&key)
                    .sorted_by(|a, b| a.0.cmp(b.0)),
            )?);

            // Gather and record storage trie nodes for this account.
            let mut storage_trie_nodes = BTreeMap::default();
            let storage = state.storages.get(&hashed_address);
            for hashed_slot in hashed_slots {
                let slot_nibbles = Nibbles::unpack(hashed_slot);
                let slot_value = storage
                    .and_then(|s| s.storage.get(&hashed_slot))
                    .filter(|v| !v.is_zero())
                    .map(|v| alloy_rlp::encode_fixed_size(v).to_vec());
                storage_trie_nodes.extend(target_nodes(
                    slot_nibbles.clone(),
                    slot_value,
                    Some(&mut self.witness),
                    storage_multiproof
                        .subtree
                        .matching_nodes_iter(&slot_nibbles)
                        .sorted_by(|a, b| a.0.cmp(b.0)),
                )?);
            }

            next_root_from_proofs(storage_trie_nodes, |key: Nibbles| {
                // Right pad the target with 0s.
                let mut padded_key = key.pack();
                padded_key.resize(32, 0);
                let target_key = B256::from_slice(&padded_key);
                let storage_prefix_set = self
                    .prefix_sets
                    .storage_prefix_sets
                    .get(&hashed_address)
                    .cloned()
                    .unwrap_or_default();
                let proof = StorageProof::new_hashed(
                    self.trie_cursor_factory.clone(),
                    self.hashed_cursor_factory.clone(),
                    hashed_address,
                )
                .with_prefix_set_mut(storage_prefix_set)
                .storage_multiproof(HashSet::from_iter([target_key]))?;

                // The subtree only contains the proof for a single target.
                let node =
                    proof.subtree.get(&key).ok_or(TrieWitnessError::MissingTargetNode(key))?;
                self.witness.insert(keccak256(node.as_ref()), node.clone()); // record in witness
                Ok(node.clone())
            })?;
        }

        next_root_from_proofs(account_trie_nodes, |key: Nibbles| {
            // Right pad the target with 0s.
            let mut padded_key = key.pack();
            padded_key.resize(32, 0);
            let targets = HashMap::from_iter([(B256::from_slice(&padded_key), HashSet::default())]);
            let proof =
                Proof::new(self.trie_cursor_factory.clone(), self.hashed_cursor_factory.clone())
                    .with_prefix_sets_mut(self.prefix_sets.clone())
                    .multiproof(targets)?;

            // The subtree only contains the proof for a single target.
            let node =
                proof.account_subtree.get(&key).ok_or(TrieWitnessError::MissingTargetNode(key))?;
            self.witness.insert(keccak256(node.as_ref()), node.clone()); // record in witness
            Ok(node.clone())
        })?;

        Ok(self.witness)
    }

    /// Retrieve proof targets for incoming hashed state.
    /// This method will aggregate all accounts and slots present in the hash state as well as
    /// select all existing slots from the database for the accounts that have been destroyed.
    fn get_proof_targets(
        &self,
        state: &HashedPostState,
    ) -> Result<HashMap<B256, HashSet<B256>>, StateProofError> {
        let mut proof_targets = HashMap::default();
        for hashed_address in state.accounts.keys() {
            proof_targets.insert(*hashed_address, HashSet::default());
        }
        for (hashed_address, storage) in &state.storages {
            let mut storage_keys = storage.storage.keys().copied().collect::<HashSet<_>>();
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

/// Decodes and unrolls all nodes from the proof. Returns only sibling nodes
/// in the path of the target and the final leaf node with updated value.
pub fn target_nodes<'b>(
    key: Nibbles,
    value: Option<Vec<u8>>,
    mut witness: Option<&mut HashMap<B256, Bytes>>,
    proof: impl IntoIterator<Item = (&'b Nibbles, &'b Bytes)>,
) -> Result<BTreeMap<Nibbles, Either<B256, Vec<u8>>>, TrieWitnessError> {
    let mut trie_nodes = BTreeMap::default();
    let mut proof_iter = proof.into_iter().enumerate().peekable();
    while let Some((idx, (path, encoded))) = proof_iter.next() {
        // Record the node in witness.
        if let Some(witness) = witness.as_mut() {
            witness.insert(keccak256(encoded.as_ref()), encoded.clone());
        }

        let mut next_path = path.clone();
        match TrieNode::decode(&mut &encoded[..])? {
            TrieNode::Branch(branch) => {
                next_path.push(key[path.len()]);
                let children = branch_node_children(path.clone(), &branch);
                for (child_path, value) in children {
                    if !key.starts_with(&child_path) {
                        let value = if value.len() < B256::len_bytes() {
                            Either::Right(value.to_vec())
                        } else {
                            Either::Left(B256::from_slice(&value[1..]))
                        };
                        trie_nodes.insert(child_path, value);
                    }
                }
            }
            TrieNode::Extension(extension) => {
                next_path.extend_from_slice(&extension.key);
            }
            TrieNode::Leaf(leaf) => {
                next_path.extend_from_slice(&leaf.key);
                if next_path != key {
                    trie_nodes
                        .insert(next_path.clone(), Either::Right(leaf.value.as_slice().to_vec()));
                }
            }
            TrieNode::EmptyRoot => {
                if idx != 0 || proof_iter.peek().is_some() {
                    return Err(TrieWitnessError::UnexpectedEmptyRoot(next_path))
                }
            }
        };
    }

    if let Some(value) = value {
        trie_nodes.insert(key, Either::Right(value));
    }

    Ok(trie_nodes)
}

/// Computes the next root hash of a trie by processing a set of trie nodes and
/// their provided values.
pub fn next_root_from_proofs(
    trie_nodes: BTreeMap<Nibbles, Either<B256, Vec<u8>>>,
    mut trie_node_provider: impl FnMut(Nibbles) -> Result<Bytes, TrieWitnessError>,
) -> Result<B256, TrieWitnessError> {
    // Ignore branch child hashes in the path of leaves or lower child hashes.
    let mut keys = trie_nodes.keys().peekable();
    let mut ignored = HashSet::<Nibbles>::default();
    while let Some(key) = keys.next() {
        if keys.peek().is_some_and(|next| next.starts_with(key)) {
            ignored.insert(key.clone());
        }
    }

    let mut hash_builder = HashBuilder::default();
    let mut trie_nodes = trie_nodes.into_iter().filter(|e| !ignored.contains(&e.0)).peekable();
    while let Some((path, value)) = trie_nodes.next() {
        match value {
            Either::Left(branch_hash) => {
                let parent_branch_path = path.slice(..path.len() - 1);
                if hash_builder.key.starts_with(&parent_branch_path) ||
                    trie_nodes.peek().is_some_and(|next| next.0.starts_with(&parent_branch_path))
                {
                    hash_builder.add_branch(path, branch_hash, false);
                } else {
                    // Parent is a branch node that needs to be turned into an extension node.
                    let mut path = path.clone();
                    loop {
                        let node = trie_node_provider(path.clone())?;
                        match TrieNode::decode(&mut &node[..])? {
                            TrieNode::Branch(branch) => {
                                let children = branch_node_children(path, &branch);
                                for (child_path, value) in children {
                                    if value.len() < B256::len_bytes() {
                                        hash_builder.add_leaf(child_path, value);
                                    } else {
                                        let hash = B256::from_slice(&value[1..]);
                                        hash_builder.add_branch(child_path, hash, false);
                                    }
                                }
                                break
                            }
                            TrieNode::Leaf(leaf) => {
                                let mut child_path = path;
                                child_path.extend_from_slice(&leaf.key);
                                hash_builder.add_leaf(child_path, &leaf.value);
                                break
                            }
                            TrieNode::Extension(ext) => {
                                path.extend_from_slice(&ext.key);
                            }
                            TrieNode::EmptyRoot => {
                                return Err(TrieWitnessError::UnexpectedEmptyRoot(path))
                            }
                        }
                    }
                }
            }
            Either::Right(leaf_value) => {
                hash_builder.add_leaf(path, &leaf_value);
            }
        }
    }
    Ok(hash_builder.root())
}

/// Returned branch node children with keys in order.
fn branch_node_children(prefix: Nibbles, node: &BranchNode) -> Vec<(Nibbles, &[u8])> {
    let mut children = Vec::with_capacity(node.state_mask.count_ones() as usize);
    let mut stack_ptr = node.as_ref().first_child_index();
    for index in CHILD_INDEX_RANGE {
        if node.state_mask.is_bit_set(index) {
            let mut child_path = prefix.clone();
            child_path.push(index);
            children.push((child_path, &node.stack[stack_ptr][..]));
            stack_ptr += 1;
        }
    }
    children
}
