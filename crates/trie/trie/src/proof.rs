use crate::{
    hashed_cursor::{HashedCursor, HashedCursorFactory, HashedStorageCursor},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::TriePrefixSetsMut,
    trie_cursor::{TrieCursor, TrieCursorFactory},
    walker::TrieWalker,
    HashBuilder, Nibbles,
};
use alloy_rlp::{BufMut, Encodable};
use reth_execution_errors::{StateRootError, StorageRootError};
use reth_primitives::{constants::EMPTY_ROOT_HASH, keccak256, Address, B256};
use reth_trie_common::{proof::ProofRetainer, AccountProof, StorageProof, TrieAccount};

/// A struct for generating merkle proofs.
///
/// Proof generator adds the target address and slots to the prefix set, enables the proof retainer
/// on the hash builder and follows the same algorithm as the state root calculator.
/// See `StateRoot::root` for more info.
#[derive(Debug)]
pub struct Proof<T, H> {
    /// The factory for hashed cursors.
    hashed_cursor_factory: H,
    /// Creates cursor for traversing trie entities.
    trie_cursor_factory: T,
    /// A set of prefix sets that have changes.
    prefix_sets: TriePrefixSetsMut,
}

impl<T, H> Proof<T, H> {
    /// Create a new [Proof] instance.
    pub fn new(t: T, h: H) -> Self {
        Self {
            trie_cursor_factory: t,
            hashed_cursor_factory: h,
            prefix_sets: TriePrefixSetsMut::default(),
        }
    }

    /// Set the hashed cursor factory.
    pub fn with_hashed_cursor_factory<HF>(self, hashed_cursor_factory: HF) -> Proof<T, HF> {
        Proof {
            trie_cursor_factory: self.trie_cursor_factory,
            hashed_cursor_factory,
            prefix_sets: self.prefix_sets,
        }
    }

    /// Set the prefix sets. They have to be mutable in order to allow extension with proof target.
    pub fn with_prefix_sets_mut(mut self, prefix_sets: TriePrefixSetsMut) -> Self {
        self.prefix_sets = prefix_sets;
        self
    }
}

impl<T, H> Proof<T, H>
where
    T: TrieCursorFactory,
    H: HashedCursorFactory + Clone,
    <T::AccountTrieCursor as TrieCursor>::Err: From<<H::AccountCursor as HashedCursor>::Err>,
    <T::StorageTrieCursor as TrieCursor>::Err: From<<H::StorageCursor as HashedCursor>::Err>,
    StateRootError: From<T::Err> + From<H::Err> + From<<T::AccountTrieCursor as TrieCursor>::Err>,
    StorageRootError: From<T::Err>
        + From<H::Err>
        + From<<T::StorageTrieCursor as TrieCursor>::Err>
        + From<<H::StorageCursor as HashedCursor>::Err>,
{
    /// Generate an account proof from intermediate nodes.
    pub fn account_proof(
        &self,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateRootError> {
        let target_hashed_address = keccak256(address);
        let target_nibbles = Nibbles::unpack(target_hashed_address);
        let mut account_proof = AccountProof::new(address);

        let hashed_account_cursor = self.hashed_cursor_factory.hashed_account_cursor()?;
        let trie_cursor = self.trie_cursor_factory.account_trie_cursor()?;

        // Create the walker.
        let mut prefix_set = self.prefix_sets.account_prefix_set.clone();
        prefix_set.insert(target_nibbles.clone());
        let walker = TrieWalker::new(trie_cursor, prefix_set.freeze());

        // Create a hash builder to rebuild the root node since it is not available in the database.
        let retainer = ProofRetainer::from_iter([target_nibbles]);
        let mut hash_builder = HashBuilder::default().with_proof_retainer(retainer);

        let mut account_rlp = Vec::with_capacity(128);
        let mut account_node_iter = TrieNodeIter::new(walker, hashed_account_cursor);
        while let Some(account_node) = account_node_iter.try_next()? {
            match account_node {
                TrieElement::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_address, account) => {
                    let storage_root = if hashed_address == target_hashed_address {
                        let (storage_root, storage_proofs) =
                            self.storage_root_with_proofs(hashed_address, slots)?;
                        account_proof.set_account(account, storage_root, storage_proofs);
                        storage_root
                    } else {
                        self.storage_root(hashed_address)?
                    };

                    account_rlp.clear();
                    let account = TrieAccount::from((account, storage_root));
                    account.encode(&mut account_rlp as &mut dyn BufMut);

                    hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);
                }
            }
        }

        let _ = hash_builder.root();

        let proofs = hash_builder.take_proofs();
        account_proof.set_proof(proofs.values().cloned().collect());

        Ok(account_proof)
    }

    /// Compute storage root.
    pub fn storage_root(&self, hashed_address: B256) -> Result<B256, StorageRootError> {
        let (storage_root, _) = self.storage_root_with_proofs(hashed_address, &[])?;
        Ok(storage_root)
    }

    /// Compute the storage root and retain proofs for requested slots.
    pub fn storage_root_with_proofs(
        &self,
        hashed_address: B256,
        slots: &[B256],
    ) -> Result<(B256, Vec<StorageProof>), StorageRootError> {
        let mut hashed_storage_cursor =
            self.hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;

        let mut proofs = slots.iter().copied().map(StorageProof::new).collect::<Vec<_>>();

        // short circuit on empty storage
        if hashed_storage_cursor.is_storage_empty()? {
            return Ok((EMPTY_ROOT_HASH, proofs))
        }

        let target_nibbles = proofs.iter().map(|p| p.nibbles.clone()).collect::<Vec<_>>();
        let mut prefix_set =
            self.prefix_sets.storage_prefix_sets.get(&hashed_address).cloned().unwrap_or_default();
        prefix_set.extend(target_nibbles.clone());
        let trie_cursor = self.trie_cursor_factory.storage_trie_cursor(hashed_address)?;
        let walker = TrieWalker::new(trie_cursor, prefix_set.freeze());

        let retainer = ProofRetainer::from_iter(target_nibbles);
        let mut hash_builder = HashBuilder::default().with_proof_retainer(retainer);
        let mut storage_node_iter = TrieNodeIter::new(walker, hashed_storage_cursor);
        while let Some(node) = storage_node_iter.try_next()? {
            match node {
                TrieElement::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_slot, value) => {
                    let nibbles = Nibbles::unpack(hashed_slot);
                    if let Some(proof) = proofs.iter_mut().find(|proof| proof.nibbles == nibbles) {
                        proof.set_value(value);
                    }
                    hash_builder.add_leaf(nibbles, alloy_rlp::encode_fixed_size(&value).as_ref());
                }
            }
        }

        let root = hash_builder.root();

        let all_proof_nodes = hash_builder.take_proofs();
        for proof in &mut proofs {
            // Iterate over all proof nodes and find the matching ones.
            // The filtered results are guaranteed to be in order.
            let matching_proof_nodes = all_proof_nodes
                .iter()
                .filter(|(path, _)| proof.nibbles.starts_with(path))
                .map(|(_, node)| node.clone());
            proof.set_proof(matching_proof_nodes.collect());
        }

        Ok((root, proofs))
    }
}
