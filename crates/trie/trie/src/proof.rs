use crate::{
    hashed_cursor::{HashedCursorFactory, HashedStorageCursor},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::TriePrefixSetsMut,
    trie_cursor::TrieCursorFactory,
    walker::TrieWalker,
    HashBuilder, Nibbles,
};
use alloy_rlp::{BufMut, Encodable};
use reth_execution_errors::trie::StateProofError;
use reth_primitives::{keccak256, Address, B256};
use reth_trie_common::{
    proof::ProofRetainer, AccountProof, MultiProof, StorageMultiProof, TrieAccount,
};
use std::collections::HashMap;

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
    /// Proof targets.
    targets: HashMap<B256, Vec<B256>>,
}

impl<T, H> Proof<T, H> {
    /// Create a new [Proof] instance.
    pub fn new(t: T, h: H) -> Self {
        Self {
            trie_cursor_factory: t,
            hashed_cursor_factory: h,
            prefix_sets: TriePrefixSetsMut::default(),
            targets: HashMap::default(),
        }
    }

    /// Set the hashed cursor factory.
    pub fn with_hashed_cursor_factory<HF>(self, hashed_cursor_factory: HF) -> Proof<T, HF> {
        Proof {
            trie_cursor_factory: self.trie_cursor_factory,
            hashed_cursor_factory,
            prefix_sets: self.prefix_sets,
            targets: self.targets,
        }
    }

    /// Set the prefix sets. They have to be mutable in order to allow extension with proof target.
    pub fn with_prefix_sets_mut(mut self, prefix_sets: TriePrefixSetsMut) -> Self {
        self.prefix_sets = prefix_sets;
        self
    }

    /// Set the target accounts and slots.
    pub fn with_targets(mut self, targets: HashMap<B256, Vec<B256>>) -> Self {
        self.targets = targets;
        self
    }
}

impl<T, H> Proof<T, H>
where
    T: TrieCursorFactory,
    H: HashedCursorFactory + Clone,
{
    /// Generate an account proof from intermediate nodes.
    pub fn account_proof(
        self,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateProofError> {
        Ok(self
            .with_targets(HashMap::from([(
                keccak256(address),
                slots.iter().map(keccak256).collect(),
            )]))
            .multiproof()?
            .account_proof(address, slots)?)
    }

    /// Generate a state multiproof according to specified targets.
    pub fn multiproof(&self) -> Result<MultiProof, StateProofError> {
        let hashed_account_cursor = self.hashed_cursor_factory.hashed_account_cursor()?;
        let trie_cursor = self.trie_cursor_factory.account_trie_cursor()?;

        // Create the walker.
        let mut prefix_set = self.prefix_sets.account_prefix_set.clone();
        prefix_set.extend(self.targets.keys().map(Nibbles::unpack));
        let walker = TrieWalker::new(trie_cursor, prefix_set.freeze());

        // Create a hash builder to rebuild the root node since it is not available in the database.
        let retainer = ProofRetainer::from_iter(self.targets.keys().map(Nibbles::unpack));
        let mut hash_builder = HashBuilder::default().with_proof_retainer(retainer);

        let mut storages = HashMap::default();
        let mut account_rlp = Vec::with_capacity(128);
        let mut account_node_iter = TrieNodeIter::new(walker, hashed_account_cursor);
        while let Some(account_node) = account_node_iter.try_next()? {
            match account_node {
                TrieElement::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_address, account) => {
                    let storage_multiproof = self.storage_multiproof(hashed_address)?;

                    // Encode account
                    account_rlp.clear();
                    let account = TrieAccount::from((account, storage_multiproof.root));
                    account.encode(&mut account_rlp as &mut dyn BufMut);

                    hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);
                    storages.insert(hashed_address, storage_multiproof);
                }
            }
        }
        let _ = hash_builder.root();
        Ok(MultiProof { account_subtree: hash_builder.take_proofs(), storages })
    }

    /// Generate a storage multiproof according to specified targets.
    pub fn storage_multiproof(
        &self,
        hashed_address: B256,
    ) -> Result<StorageMultiProof, StateProofError> {
        let mut hashed_storage_cursor =
            self.hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;

        // short circuit on empty storage
        if hashed_storage_cursor.is_storage_empty()? {
            return Ok(StorageMultiProof::default())
        }

        let target_nibbles = self
            .targets
            .get(&hashed_address)
            .map_or(Vec::new(), |slots| slots.iter().map(Nibbles::unpack).collect());

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
                    hash_builder.add_leaf(
                        Nibbles::unpack(hashed_slot),
                        alloy_rlp::encode_fixed_size(&value).as_ref(),
                    );
                }
            }
        }

        let root = hash_builder.root();
        Ok(StorageMultiProof { root, subtree: hash_builder.take_proofs() })
    }
}
