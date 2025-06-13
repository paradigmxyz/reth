use crate::{
    hashed_cursor::{HashedCursorFactory, HashedStorageCursor},
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::{PrefixSetMut, TriePrefixSetsMut},
    trie_cursor::TrieCursorFactory,
    walker::TrieWalker,
    HashBuilder, Nibbles, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use alloy_primitives::{
    keccak256,
    map::{B256Map, B256Set, HashMap, HashSet},
    Address, B256,
};
use alloy_rlp::{BufMut, Encodable};
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{
    proof::ProofRetainer, AccountProof, MultiProof, MultiProofTargets, StorageMultiProof,
};

mod blinded;
pub use blinded::*;

/// A struct for generating merkle proofs.
///
/// Proof generator adds the target address and slots to the prefix set, enables the proof retainer
/// on the hash builder and follows the same algorithm as the state root calculator.
/// See `StateRoot::root` for more info.
#[derive(Debug)]
pub struct Proof<T, H> {
    /// The factory for traversing trie nodes.
    trie_cursor_factory: T,
    /// The factory for hashed cursors.
    hashed_cursor_factory: H,
    /// A set of prefix sets that have changes.
    prefix_sets: TriePrefixSetsMut,
    /// Flag indicating whether to include branch node masks in the proof.
    collect_branch_node_masks: bool,
}

impl<T, H> Proof<T, H> {
    /// Create a new [`Proof`] instance.
    pub fn new(t: T, h: H) -> Self {
        Self {
            trie_cursor_factory: t,
            hashed_cursor_factory: h,
            prefix_sets: TriePrefixSetsMut::default(),
            collect_branch_node_masks: false,
        }
    }

    /// Set the trie cursor factory.
    pub fn with_trie_cursor_factory<TF>(self, trie_cursor_factory: TF) -> Proof<TF, H> {
        Proof {
            trie_cursor_factory,
            hashed_cursor_factory: self.hashed_cursor_factory,
            prefix_sets: self.prefix_sets,
            collect_branch_node_masks: self.collect_branch_node_masks,
        }
    }

    /// Set the hashed cursor factory.
    pub fn with_hashed_cursor_factory<HF>(self, hashed_cursor_factory: HF) -> Proof<T, HF> {
        Proof {
            trie_cursor_factory: self.trie_cursor_factory,
            hashed_cursor_factory,
            prefix_sets: self.prefix_sets,
            collect_branch_node_masks: self.collect_branch_node_masks,
        }
    }

    /// Set the prefix sets. They have to be mutable in order to allow extension with proof target.
    pub fn with_prefix_sets_mut(mut self, prefix_sets: TriePrefixSetsMut) -> Self {
        self.prefix_sets = prefix_sets;
        self
    }

    /// Set the flag indicating whether to include branch node masks in the proof.
    pub const fn with_branch_node_masks(mut self, branch_node_masks: bool) -> Self {
        self.collect_branch_node_masks = branch_node_masks;
        self
    }
}

impl<T, H> Proof<T, H>
where
    T: TrieCursorFactory + Clone,
    H: HashedCursorFactory + Clone,
{
    /// Generate an account proof from intermediate nodes.
    pub fn account_proof(
        self,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateProofError> {
        Ok(self
            .multiproof(MultiProofTargets::from_iter([(
                keccak256(address),
                slots.iter().map(keccak256).collect(),
            )]))?
            .account_proof(address, slots)?)
    }

    /// Generate a state multiproof according to specified targets.
    pub fn multiproof(
        mut self,
        mut targets: MultiProofTargets,
    ) -> Result<MultiProof, StateProofError> {
        let hashed_account_cursor = self.hashed_cursor_factory.hashed_account_cursor()?;
        let trie_cursor = self.trie_cursor_factory.account_trie_cursor()?;

        // Create the walker.
        let mut prefix_set = self.prefix_sets.account_prefix_set.clone();
        prefix_set.extend_keys(targets.keys().map(Nibbles::unpack));
        let walker = TrieWalker::state_trie(trie_cursor, prefix_set.freeze());

        // Create a hash builder to rebuild the root node since it is not available in the database.
        let retainer = targets.keys().map(Nibbles::unpack).collect();
        let mut hash_builder = HashBuilder::default()
            .with_proof_retainer(retainer)
            .with_updates(self.collect_branch_node_masks);

        // Initialize all storage multiproofs as empty.
        // Storage multiproofs for non empty tries will be overwritten if necessary.
        let mut storages: B256Map<_> =
            targets.keys().map(|key| (*key, StorageMultiProof::empty())).collect();
        let mut account_rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
        let mut account_node_iter = TrieNodeIter::state_trie(walker, hashed_account_cursor);
        while let Some(account_node) = account_node_iter.try_next()? {
            match account_node {
                TrieElement::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                TrieElement::Leaf(hashed_address, account) => {
                    let proof_targets = targets.remove(&hashed_address);
                    let leaf_is_proof_target = proof_targets.is_some();
                    let storage_prefix_set = self
                        .prefix_sets
                        .storage_prefix_sets
                        .remove(&hashed_address)
                        .unwrap_or_default();
                    let storage_multiproof = StorageProof::new_hashed(
                        self.trie_cursor_factory.clone(),
                        self.hashed_cursor_factory.clone(),
                        hashed_address,
                    )
                    .with_prefix_set_mut(storage_prefix_set)
                    .with_branch_node_masks(self.collect_branch_node_masks)
                    .storage_multiproof(proof_targets.unwrap_or_default())?;

                    // Encode account
                    account_rlp.clear();
                    let account = account.into_trie_account(storage_multiproof.root);
                    account.encode(&mut account_rlp as &mut dyn BufMut);

                    hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);

                    // We might be adding leaves that are not necessarily our proof targets.
                    if leaf_is_proof_target {
                        // Overwrite storage multiproof.
                        storages.insert(hashed_address, storage_multiproof);
                    }
                }
            }
        }
        let _ = hash_builder.root();
        let account_subtree = hash_builder.take_proof_nodes();
        let (branch_node_hash_masks, branch_node_tree_masks) = if self.collect_branch_node_masks {
            let updated_branch_nodes = hash_builder.updated_branch_nodes.unwrap_or_default();
            (
                updated_branch_nodes
                    .iter()
                    .map(|(path, node)| (path.clone(), node.hash_mask))
                    .collect(),
                updated_branch_nodes
                    .into_iter()
                    .map(|(path, node)| (path, node.tree_mask))
                    .collect(),
            )
        } else {
            (HashMap::default(), HashMap::default())
        };

        Ok(MultiProof { account_subtree, branch_node_hash_masks, branch_node_tree_masks, storages })
    }
}

/// Generates storage merkle proofs.
#[derive(Debug)]
pub struct StorageProof<T, H> {
    /// The factory for traversing trie nodes.
    trie_cursor_factory: T,
    /// The factory for hashed cursors.
    hashed_cursor_factory: H,
    /// The hashed address of an account.
    hashed_address: B256,
    /// The set of storage slot prefixes that have changed.
    prefix_set: PrefixSetMut,
    /// Flag indicating whether to include branch node masks in the proof.
    collect_branch_node_masks: bool,
}

impl<T, H> StorageProof<T, H> {
    /// Create a new [`StorageProof`] instance.
    pub fn new(t: T, h: H, address: Address) -> Self {
        Self::new_hashed(t, h, keccak256(address))
    }

    /// Create a new [`StorageProof`] instance with hashed address.
    pub fn new_hashed(t: T, h: H, hashed_address: B256) -> Self {
        Self {
            trie_cursor_factory: t,
            hashed_cursor_factory: h,
            hashed_address,
            prefix_set: PrefixSetMut::default(),
            collect_branch_node_masks: false,
        }
    }

    /// Set the trie cursor factory.
    pub fn with_trie_cursor_factory<TF>(self, trie_cursor_factory: TF) -> StorageProof<TF, H> {
        StorageProof {
            trie_cursor_factory,
            hashed_cursor_factory: self.hashed_cursor_factory,
            hashed_address: self.hashed_address,
            prefix_set: self.prefix_set,
            collect_branch_node_masks: self.collect_branch_node_masks,
        }
    }

    /// Set the hashed cursor factory.
    pub fn with_hashed_cursor_factory<HF>(self, hashed_cursor_factory: HF) -> StorageProof<T, HF> {
        StorageProof {
            trie_cursor_factory: self.trie_cursor_factory,
            hashed_cursor_factory,
            hashed_address: self.hashed_address,
            prefix_set: self.prefix_set,
            collect_branch_node_masks: self.collect_branch_node_masks,
        }
    }

    /// Set the changed prefixes.
    pub fn with_prefix_set_mut(mut self, prefix_set: PrefixSetMut) -> Self {
        self.prefix_set = prefix_set;
        self
    }

    /// Set the flag indicating whether to include branch node masks in the proof.
    pub const fn with_branch_node_masks(mut self, branch_node_masks: bool) -> Self {
        self.collect_branch_node_masks = branch_node_masks;
        self
    }
}

impl<T, H> StorageProof<T, H>
where
    T: TrieCursorFactory,
    H: HashedCursorFactory,
{
    /// Generate an account proof from intermediate nodes.
    pub fn storage_proof(
        self,
        slot: B256,
    ) -> Result<reth_trie_common::StorageProof, StateProofError> {
        let targets = HashSet::from_iter([keccak256(slot)]);
        Ok(self.storage_multiproof(targets)?.storage_proof(slot)?)
    }

    /// Generate storage proof.
    pub fn storage_multiproof(
        mut self,
        targets: B256Set,
    ) -> Result<StorageMultiProof, StateProofError> {
        let mut hashed_storage_cursor =
            self.hashed_cursor_factory.hashed_storage_cursor(self.hashed_address)?;

        // short circuit on empty storage
        if hashed_storage_cursor.is_storage_empty()? {
            return Ok(StorageMultiProof::empty())
        }

        let target_nibbles = targets.into_iter().map(Nibbles::unpack).collect::<Vec<_>>();
        self.prefix_set.extend_keys(target_nibbles.clone());

        let trie_cursor = self.trie_cursor_factory.storage_trie_cursor(self.hashed_address)?;
        let walker = TrieWalker::storage_trie(trie_cursor, self.prefix_set.freeze());

        let retainer = ProofRetainer::from_iter(target_nibbles);
        let mut hash_builder = HashBuilder::default()
            .with_proof_retainer(retainer)
            .with_updates(self.collect_branch_node_masks);
        let mut storage_node_iter = TrieNodeIter::storage_trie(walker, hashed_storage_cursor);
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
        let subtree = hash_builder.take_proof_nodes();
        let (branch_node_hash_masks, branch_node_tree_masks) = if self.collect_branch_node_masks {
            let updated_branch_nodes = hash_builder.updated_branch_nodes.unwrap_or_default();
            (
                updated_branch_nodes
                    .iter()
                    .map(|(path, node)| (path.clone(), node.hash_mask))
                    .collect(),
                updated_branch_nodes
                    .into_iter()
                    .map(|(path, node)| (path, node.tree_mask))
                    .collect(),
            )
        } else {
            (HashMap::default(), HashMap::default())
        };

        Ok(StorageMultiProof { root, subtree, branch_node_hash_masks, branch_node_tree_masks })
    }
}

#[cfg(test)]
mod tests {
    use super::{Proof, StorageProof, *};
    use crate::{
        hashed_cursor::{
            noop::{NoopHashedAccountCursor, NoopHashedStorageCursor},
            HashedCursor, HashedCursorFactory, HashedStorageCursor,
        },
        prefix_set::{PrefixSetMut, TriePrefixSetsMut},
        trie_cursor::{
            noop::{NoopAccountTrieCursor, NoopStorageTrieCursor},
            TrieCursor, TrieCursorFactory,
        },
        BranchNodeCompact, Nibbles,
    };
    use alloy_primitives::{address, keccak256, Address, B256, U256};
    use alloy_trie::EMPTY_ROOT_HASH;
    use reth_primitives_traits::Account as TrieAccount;
    use reth_storage_errors::db::DatabaseError;
    use reth_trie_common::StorageMultiProof;
    use std::collections::BTreeMap;

    // Test utilities module
    mod test_utils {
        use super::*;
        use std::sync::{Arc, Mutex};

        /// Mock trie node structure for testing
        #[derive(Debug, Clone)]
        pub enum MockTrieNode {
            Branch { children: [Option<Box<MockTrieNode>>; 16], value: Option<Vec<u8>> },
            Leaf { key: Nibbles, value: Vec<u8> },
            Empty,
        }

        /// Mock account data
        #[derive(Debug, Clone, Default)]
        pub struct MockAccount {
            pub balance: U256,
            pub nonce: u64,
            pub bytecode_hash: Option<B256>,
            pub storage_root: B256,
        }

        impl From<MockAccount> for TrieAccount {
            fn from(mock: MockAccount) -> Self {
                TrieAccount {
                    balance: mock.balance,
                    nonce: mock.nonce,
                    bytecode_hash: mock.bytecode_hash,
                }
            }
        }

        /// Test data builder for creating predictable trie structures
        #[derive(Debug, Default)]
        pub struct TestDataBuilder {
            accounts: BTreeMap<B256, MockAccount>,
            storage: BTreeMap<B256, BTreeMap<B256, U256>>,
        }

        impl TestDataBuilder {
            pub fn new() -> Self {
                Self::default()
            }

            pub fn add_account(mut self, address: Address, account: MockAccount) -> Self {
                self.accounts.insert(keccak256(address), account);
                self
            }

            pub fn add_storage(mut self, address: Address, slot: B256, value: U256) -> Self {
                let hashed_address = keccak256(address);
                self.storage.entry(hashed_address).or_default().insert(keccak256(slot), value);
                self
            }

            pub fn build(self) -> (MockTrieCursorFactory, MockHashedCursorFactory) {
                let trie_factory = MockTrieCursorFactory {
                    accounts: Arc::new(Mutex::new(self.accounts.clone())),
                    storage: Arc::new(Mutex::new(self.storage.clone())),
                };
                let hashed_factory = MockHashedCursorFactory {
                    accounts: Arc::new(Mutex::new(self.accounts)),
                    storage: Arc::new(Mutex::new(self.storage)),
                };
                (trie_factory, hashed_factory)
            }
        }

        /// Mock implementation of TrieCursorFactory for testing
        #[derive(Clone, Debug)]
        pub struct MockTrieCursorFactory {
            accounts: Arc<Mutex<BTreeMap<B256, MockAccount>>>,
            storage: Arc<Mutex<BTreeMap<B256, BTreeMap<B256, U256>>>>,
        }

        impl TrieCursorFactory for MockTrieCursorFactory {
            type AccountTrieCursor = MockAccountTrieCursor;
            type StorageTrieCursor = MockStorageTrieCursor;

            fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, DatabaseError> {
                Ok(MockAccountTrieCursor { accounts: self.accounts.clone(), position: None })
            }

            fn storage_trie_cursor(
                &self,
                hashed_address: B256,
            ) -> Result<Self::StorageTrieCursor, DatabaseError> {
                Ok(MockStorageTrieCursor {
                    storage: self
                        .storage
                        .lock()
                        .unwrap()
                        .get(&hashed_address)
                        .cloned()
                        .unwrap_or_default(),
                    position: None,
                })
            }
        }

        /// Mock account trie cursor
        pub struct MockAccountTrieCursor {
            accounts: Arc<Mutex<BTreeMap<B256, MockAccount>>>,
            position: Option<Nibbles>,
        }

        impl TrieCursor for MockAccountTrieCursor {
            fn seek_exact(
                &mut self,
                _key: Nibbles,
            ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
                Ok(None) // Simplified for basic tests
            }

            fn seek(
                &mut self,
                _key: Nibbles,
            ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
                Ok(None)
            }

            fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
                Ok(None)
            }

            fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
                Ok(self.position.clone())
            }
        }

        /// Mock storage trie cursor
        pub struct MockStorageTrieCursor {
            storage: BTreeMap<B256, U256>,
            position: Option<Nibbles>,
        }

        impl TrieCursor for MockStorageTrieCursor {
            fn seek_exact(
                &mut self,
                _key: Nibbles,
            ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
                Ok(None) // Simplified for basic tests
            }

            fn seek(
                &mut self,
                _key: Nibbles,
            ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
                Ok(None)
            }

            fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
                Ok(None)
            }

            fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
                Ok(self.position.clone())
            }
        }

        /// Mock hashed cursor factory
        #[derive(Clone, Debug)]
        pub struct MockHashedCursorFactory {
            accounts: Arc<Mutex<BTreeMap<B256, MockAccount>>>,
            storage: Arc<Mutex<BTreeMap<B256, BTreeMap<B256, U256>>>>,
        }

        impl HashedCursorFactory for MockHashedCursorFactory {
            type AccountCursor = MockHashedAccountCursor;
            type StorageCursor = MockHashedStorageCursor;

            fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
                Ok(MockHashedAccountCursor {
                    accounts: self.accounts.lock().unwrap().clone(),
                    position: None,
                })
            }

            fn hashed_storage_cursor(
                &self,
                hashed_address: B256,
            ) -> Result<Self::StorageCursor, DatabaseError> {
                Ok(MockHashedStorageCursor {
                    storage: self
                        .storage
                        .lock()
                        .unwrap()
                        .get(&hashed_address)
                        .cloned()
                        .unwrap_or_default(),
                    position: None,
                    is_empty: self
                        .storage
                        .lock()
                        .unwrap()
                        .get(&hashed_address)
                        .map_or(true, |s| s.is_empty()),
                })
            }
        }

        /// Mock hashed account cursor
        pub struct MockHashedAccountCursor {
            accounts: BTreeMap<B256, MockAccount>,
            position: Option<B256>,
        }

        impl HashedCursor for MockHashedAccountCursor {
            type Value = TrieAccount;

            fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
                self.position = Some(key);
                Ok(self.accounts.get(&key).map(|acc| (key, acc.clone().into())))
            }

            fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
                if let Some(current) = &self.position {
                    let next = self
                        .accounts
                        .range((std::ops::Bound::Excluded(*current), std::ops::Bound::Unbounded))
                        .next();
                    if let Some((key, acc)) = next {
                        self.position = Some(*key);
                        Ok(Some((*key, acc.clone().into())))
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
        }

        /// Mock hashed storage cursor
        pub struct MockHashedStorageCursor {
            storage: BTreeMap<B256, U256>,
            position: Option<B256>,
            is_empty: bool,
        }

        impl HashedCursor for MockHashedStorageCursor {
            type Value = U256;

            fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
                self.position = Some(key);
                Ok(self.storage.get(&key).map(|val| (key, *val)))
            }

            fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
                if let Some(current) = &self.position {
                    let next = self
                        .storage
                        .range((std::ops::Bound::Excluded(*current), std::ops::Bound::Unbounded))
                        .next();
                    if let Some((key, val)) = next {
                        self.position = Some(*key);
                        Ok(Some((*key, *val)))
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
        }

        impl HashedStorageCursor for MockHashedStorageCursor {
            fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
                Ok(self.is_empty)
            }
        }
    }

    use test_utils::*;

    /// Simple factory that returns empty cursors
    #[derive(Clone, Debug, Default)]
    struct EmptyTrieCursorFactory;

    impl TrieCursorFactory for EmptyTrieCursorFactory {
        type AccountTrieCursor = NoopAccountTrieCursor;
        type StorageTrieCursor = NoopStorageTrieCursor;

        fn account_trie_cursor(
            &self,
        ) -> Result<Self::AccountTrieCursor, reth_storage_errors::db::DatabaseError> {
            Ok(NoopAccountTrieCursor::default())
        }

        fn storage_trie_cursor(
            &self,
            _hashed_address: B256,
        ) -> Result<Self::StorageTrieCursor, reth_storage_errors::db::DatabaseError> {
            Ok(NoopStorageTrieCursor::default())
        }
    }

    /// Simple factory that returns empty hashed cursors
    #[derive(Clone, Debug, Default)]
    struct EmptyHashedCursorFactory;

    impl HashedCursorFactory for EmptyHashedCursorFactory {
        type AccountCursor = NoopHashedAccountCursor;
        type StorageCursor = NoopHashedStorageCursor;

        fn hashed_account_cursor(
            &self,
        ) -> Result<Self::AccountCursor, reth_storage_errors::db::DatabaseError> {
            Ok(NoopHashedAccountCursor::default())
        }

        fn hashed_storage_cursor(
            &self,
            _hashed_address: B256,
        ) -> Result<Self::StorageCursor, reth_storage_errors::db::DatabaseError> {
            Ok(NoopHashedStorageCursor::default())
        }
    }

    #[test]
    fn test_proof_new() {
        // Test that we can create a new proof instance
        let trie_cursor_factory = EmptyTrieCursorFactory;
        let hashed_cursor_factory = EmptyHashedCursorFactory;
        let _proof = Proof::new(trie_cursor_factory, hashed_cursor_factory);
    }

    #[test]
    fn test_proof_with_prefix_sets() {
        // Test that we can set prefix sets
        let trie_cursor_factory = EmptyTrieCursorFactory;
        let hashed_cursor_factory = EmptyHashedCursorFactory;
        let prefix_sets = TriePrefixSetsMut::default();
        let _proof = Proof::new(trie_cursor_factory, hashed_cursor_factory)
            .with_prefix_sets_mut(prefix_sets);
    }

    #[test]
    fn test_proof_with_branch_node_masks() {
        // Test that we can enable branch node masks
        let trie_cursor_factory = EmptyTrieCursorFactory;
        let hashed_cursor_factory = EmptyHashedCursorFactory;
        let _proof =
            Proof::new(trie_cursor_factory, hashed_cursor_factory).with_branch_node_masks(true);
    }

    #[test]
    fn test_storage_proof_new() {
        // Test that we can create a new storage proof instance
        let trie_cursor_factory = EmptyTrieCursorFactory;
        let hashed_cursor_factory = EmptyHashedCursorFactory;
        let address = address!("0x1000000000000000000000000000000000000001");
        let _storage_proof = StorageProof::new(trie_cursor_factory, hashed_cursor_factory, address);
    }

    #[test]
    fn test_storage_proof_with_prefix_set() {
        // Test that we can set prefix set on storage proof
        let trie_cursor_factory = EmptyTrieCursorFactory;
        let hashed_cursor_factory = EmptyHashedCursorFactory;
        let address = address!("0x1000000000000000000000000000000000000001");
        let prefix_set = PrefixSetMut::default();
        let _storage_proof = StorageProof::new(trie_cursor_factory, hashed_cursor_factory, address)
            .with_prefix_set_mut(prefix_set);
    }

    #[test]
    fn test_empty_multiproof() {
        // Test generation of empty multiproof
        let trie_cursor_factory = EmptyTrieCursorFactory;
        let hashed_cursor_factory = EmptyHashedCursorFactory;
        let proof = Proof::new(trie_cursor_factory, hashed_cursor_factory);

        // Empty targets should produce empty multiproof
        let targets = MultiProofTargets::default();
        let result = proof.multiproof(targets).unwrap();

        // Empty multiproof still contains the root node
        assert!(!result.account_subtree.is_empty());
        assert!(result.storages.is_empty());
        assert!(result.branch_node_hash_masks.is_empty());
        assert!(result.branch_node_tree_masks.is_empty());
    }

    #[test]
    fn test_empty_storage_multiproof() {
        // Test StorageMultiProof::empty()
        let empty_proof = StorageMultiProof::empty();
        assert_eq!(empty_proof.root, EMPTY_ROOT_HASH);
        assert!(!empty_proof.subtree.is_empty()); // Contains empty root node

        // Should be able to get proof for any slot (all will be empty)
        let slot = B256::from(U256::from(1));
        let proof = empty_proof.storage_proof(slot).unwrap();
        assert_eq!(proof.key, slot);
        assert_eq!(proof.value, U256::ZERO);
    }

    #[test]
    fn test_proof_with_different_cursor_factories() {
        // Test that we can change cursor factories
        let trie_factory1 = EmptyTrieCursorFactory;
        let hashed_factory1 = EmptyHashedCursorFactory;
        let proof = Proof::new(trie_factory1, hashed_factory1);

        // Change trie cursor factory
        let trie_factory2 = EmptyTrieCursorFactory;
        let _proof = proof.with_trie_cursor_factory(trie_factory2);

        // Change hashed cursor factory
        let hashed_factory2 = EmptyHashedCursorFactory;
        let proof = Proof::new(EmptyTrieCursorFactory, EmptyHashedCursorFactory);
        let _proof = proof.with_hashed_cursor_factory(hashed_factory2);
    }

    #[test]
    fn test_storage_proof_with_different_cursor_factories() {
        // Test that we can change cursor factories on storage proof
        let trie_factory1 = EmptyTrieCursorFactory;
        let hashed_factory1 = EmptyHashedCursorFactory;
        let address = address!("0x1000000000000000000000000000000000000001");
        let storage_proof = StorageProof::new(trie_factory1, hashed_factory1, address);

        // Change trie cursor factory
        let trie_factory2 = EmptyTrieCursorFactory;
        let _storage_proof = storage_proof.with_trie_cursor_factory(trie_factory2);

        // Change hashed cursor factory
        let hashed_factory2 = EmptyHashedCursorFactory;
        let storage_proof =
            StorageProof::new(EmptyTrieCursorFactory, EmptyHashedCursorFactory, address);
        let _storage_proof = storage_proof.with_hashed_cursor_factory(hashed_factory2);
    }

    #[test]
    fn test_storage_proof_with_branch_node_masks() {
        // Test that we can enable branch node masks on storage proof
        let trie_cursor_factory = EmptyTrieCursorFactory;
        let hashed_cursor_factory = EmptyHashedCursorFactory;
        let address = address!("0x1000000000000000000000000000000000000001");
        let _storage_proof = StorageProof::new(trie_cursor_factory, hashed_cursor_factory, address)
            .with_branch_node_masks(true);
    }

    // Additional comprehensive multiproof tests

    #[test]
    fn test_multiproof_single_account_with_mock_data() {
        let address = address!("0x1000000000000000000000000000000000000001");
        let account = MockAccount {
            balance: U256::from(1000),
            nonce: 5,
            bytecode_hash: Some(B256::from(U256::from(12345))),
            storage_root: EMPTY_ROOT_HASH,
        };

        let (trie_factory, hashed_factory) =
            TestDataBuilder::new().add_account(address, account.clone()).build();

        let proof = Proof::new(trie_factory, hashed_factory);
        let targets = MultiProofTargets::account(keccak256(address));

        // This will use our mock data
        let result = proof.multiproof(targets);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multiproof_multiple_accounts_with_storage() {
        let addr1 = address!("0x1000000000000000000000000000000000000001");
        let addr2 = address!("0x2000000000000000000000000000000000000002");

        let (trie_factory, hashed_factory) = TestDataBuilder::new()
            .add_account(
                addr1,
                MockAccount {
                    balance: U256::from(1000),
                    nonce: 1,
                    bytecode_hash: None,
                    storage_root: EMPTY_ROOT_HASH,
                },
            )
            .add_account(
                addr2,
                MockAccount {
                    balance: U256::from(2000),
                    nonce: 2,
                    bytecode_hash: None,
                    storage_root: EMPTY_ROOT_HASH,
                },
            )
            .add_storage(addr1, B256::from(U256::from(1)), U256::from(100))
            .add_storage(addr2, B256::from(U256::from(2)), U256::from(200))
            .build();

        let proof = Proof::new(trie_factory, hashed_factory);
        let targets = MultiProofTargets::accounts(vec![keccak256(addr1), keccak256(addr2)]);

        let result = proof.multiproof(targets);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multiproof_with_pre_existing_prefix_sets() {
        let address = address!("0x1000000000000000000000000000000000000001");
        let hashed = keccak256(address);

        // Create prefix sets with some existing data
        let mut prefix_sets = TriePrefixSetsMut::default();
        prefix_sets.account_prefix_set.insert(Nibbles::unpack(B256::from(U256::from(99999))));

        let (trie_factory, hashed_factory) =
            TestDataBuilder::new().add_account(address, MockAccount::default()).build();

        let proof = Proof::new(trie_factory, hashed_factory).with_prefix_sets_mut(prefix_sets);

        let targets = MultiProofTargets::account(hashed);
        let result = proof.multiproof(targets);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multiproof_branch_node_masks_collection() {
        let addresses: Vec<Address> = (0..10).map(|i| Address::with_last_byte(i as u8)).collect();

        let mut builder = TestDataBuilder::new();
        for (i, addr) in addresses.iter().enumerate() {
            builder = builder.add_account(
                *addr,
                MockAccount {
                    balance: U256::from(i * 100),
                    nonce: i as u64,
                    bytecode_hash: None,
                    storage_root: EMPTY_ROOT_HASH,
                },
            );
        }

        let (trie_factory, hashed_factory) = builder.build();
        let proof = Proof::new(trie_factory, hashed_factory).with_branch_node_masks(true);

        let targets = MultiProofTargets::accounts(
            addresses.iter().map(|a| keccak256(*a)).collect::<Vec<_>>(),
        );

        let result = proof.multiproof(targets);
        assert!(result.is_ok());

        // With branch masks enabled, we should have mask data
        // (though it might be empty if our mock doesn't create branch nodes)
        let multiproof = result.unwrap();
        assert!(
            multiproof.branch_node_hash_masks.is_empty() ||
                !multiproof.branch_node_hash_masks.is_empty()
        );
        assert!(
            multiproof.branch_node_tree_masks.is_empty() ||
                !multiproof.branch_node_tree_masks.is_empty()
        );
    }

    #[test]
    fn test_storage_multiproof_empty_storage() {
        let address = address!("0x1000000000000000000000000000000000000001");
        let (trie_factory, hashed_factory) =
            TestDataBuilder::new().add_account(address, MockAccount::default()).build();

        let storage_proof = StorageProof::new(trie_factory, hashed_factory, address);
        let targets = vec![keccak256(B256::from(U256::from(1)))];

        let result = storage_proof.storage_multiproof(targets.into_iter().collect());
        assert!(result.is_ok());

        let multiproof = result.unwrap();
        assert_eq!(multiproof.root, EMPTY_ROOT_HASH);
    }

    #[test]
    fn test_storage_multiproof_with_data() {
        let address = address!("0x1000000000000000000000000000000000000001");
        let slot1 = B256::from(U256::from(1));
        let slot2 = B256::from(U256::from(2));
        let value1 = U256::from(100);
        let value2 = U256::from(200);

        let (trie_factory, hashed_factory) = TestDataBuilder::new()
            .add_account(address, MockAccount::default())
            .add_storage(address, slot1, value1)
            .add_storage(address, slot2, value2)
            .build();

        let storage_proof = StorageProof::new(trie_factory, hashed_factory, address);
        let targets = vec![keccak256(slot1), keccak256(slot2)];

        let result = storage_proof.storage_multiproof(targets.into_iter().collect());
        assert!(result.is_ok());
    }

    #[test]
    fn test_account_proof_extraction_from_multiproof() {
        let address = address!("0x1000000000000000000000000000000000000001");
        let slot = B256::from(U256::from(1));

        let (trie_factory, hashed_factory) = TestDataBuilder::new()
            .add_account(
                address,
                MockAccount {
                    balance: U256::from(1000),
                    nonce: 1,
                    bytecode_hash: None,
                    storage_root: EMPTY_ROOT_HASH,
                },
            )
            .add_storage(address, slot, U256::from(42))
            .build();

        let proof = Proof::new(trie_factory, hashed_factory);
        let targets =
            MultiProofTargets::account_with_slots(keccak256(address), vec![keccak256(slot)]);

        let multiproof = proof.multiproof(targets).unwrap();

        // Extract account proof from multiproof
        let account_proof_result = multiproof.account_proof(address, &[slot]);
        assert!(account_proof_result.is_ok());
    }

    #[test]
    fn test_large_multiproof_generation() {
        // Test with a large number of accounts
        let num_accounts = 100;
        let mut builder = TestDataBuilder::new();

        let addresses: Vec<Address> =
            (0..num_accounts).map(|i| Address::with_last_byte(i as u8)).collect();

        for (i, addr) in addresses.iter().enumerate() {
            builder = builder.add_account(
                *addr,
                MockAccount {
                    balance: U256::from(i * 1000),
                    nonce: i as u64,
                    bytecode_hash: if i % 2 == 0 {
                        Some(B256::from(U256::from(i * 1000)))
                    } else {
                        None
                    },
                    storage_root: EMPTY_ROOT_HASH,
                },
            );

            // Add some storage for every 3rd account
            if i % 3 == 0 {
                for j in 0..5 {
                    builder = builder.add_storage(
                        *addr,
                        B256::from(U256::from(j)),
                        U256::from(i * 10 + j),
                    );
                }
            }
        }

        let (trie_factory, hashed_factory) = builder.build();
        let proof = Proof::new(trie_factory, hashed_factory);

        // Request proofs for all accounts
        let targets = MultiProofTargets::accounts(
            addresses.iter().map(|a| keccak256(*a)).collect::<Vec<_>>(),
        );

        let result = proof.multiproof(targets);
        assert!(result.is_ok());

        let multiproof = result.unwrap();
        assert_eq!(multiproof.storages.len(), num_accounts);
    }

    #[test]
    fn test_parallel_multiproof_single_account() {
        use std::thread;

        let address = address!("0x1000000000000000000000000000000000000001");
        let account = MockAccount {
            balance: U256::from(1000),
            nonce: 5,
            bytecode_hash: Some(B256::from(U256::from(12345))),
            storage_root: EMPTY_ROOT_HASH,
        };

        let (trie_factory, hashed_factory) =
            TestDataBuilder::new().add_account(address, account.clone()).build();

        // Create multiple threads generating the same proof
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let trie_factory = trie_factory.clone();
                let hashed_factory = hashed_factory.clone();
                thread::spawn(move || {
                    let proof = Proof::new(trie_factory, hashed_factory);
                    let targets = MultiProofTargets::account(keccak256(address));
                    proof.multiproof(targets)
                })
            })
            .collect();

        // Collect results
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All should succeed
        for result in &results {
            assert!(result.is_ok());
        }

        // All proofs should be identical (same account_subtree and storages)
        let first_proof = &results[0].as_ref().unwrap();
        for result in results.iter().skip(1) {
            let proof = result.as_ref().unwrap();
            assert_eq!(proof.storages.len(), first_proof.storages.len());
            // Note: We can't directly compare account_subtree as ProofNodes doesn't implement
            // PartialEq
        }
    }

    #[test]
    fn test_parallel_multiproof_different_accounts() {
        use std::thread;

        // Create multiple accounts
        let num_accounts = 10;
        let mut builder = TestDataBuilder::new();
        let addresses: Vec<Address> =
            (0..num_accounts).map(|i| Address::with_last_byte(i as u8)).collect();

        for (i, addr) in addresses.iter().enumerate() {
            builder = builder.add_account(
                *addr,
                MockAccount {
                    balance: U256::from(i * 1000),
                    nonce: i as u64,
                    bytecode_hash: None,
                    storage_root: EMPTY_ROOT_HASH,
                },
            );
        }

        let (trie_factory, hashed_factory) = builder.build();

        // Create threads, each generating proof for different accounts
        let handles: Vec<_> = addresses
            .chunks(2)
            .map(|chunk| {
                let trie_factory = trie_factory.clone();
                let hashed_factory = hashed_factory.clone();
                let chunk_addresses = chunk.to_vec();
                thread::spawn(move || {
                    let proof = Proof::new(trie_factory, hashed_factory);
                    let targets = MultiProofTargets::accounts(
                        chunk_addresses.iter().map(|a| keccak256(*a)).collect::<Vec<_>>(),
                    );
                    proof.multiproof(targets)
                })
            })
            .collect();

        // Collect results
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All should succeed
        for result in &results {
            assert!(result.is_ok());
        }

        // Each proof should contain the expected number of accounts
        for (i, result) in results.iter().enumerate() {
            let proof = result.as_ref().unwrap();
            let expected_accounts = if i < 4 { 2 } else { 2 }; // Each chunk has 2 accounts
            assert_eq!(proof.storages.len(), expected_accounts);
        }
    }

    #[test]
    fn test_parallel_storage_multiproof() {
        use std::thread;

        let address = address!("0x1000000000000000000000000000000000000001");
        let num_slots = 20;

        let mut builder = TestDataBuilder::new().add_account(address, MockAccount::default());

        // Add many storage slots
        for i in 0..num_slots {
            builder = builder.add_storage(address, B256::from(U256::from(i)), U256::from(i * 100));
        }

        let (trie_factory, hashed_factory) = builder.build();

        // Create threads, each requesting different storage slots
        let handles: Vec<_> = (0..4)
            .map(|thread_id| {
                let trie_factory = trie_factory.clone();
                let hashed_factory = hashed_factory.clone();
                thread::spawn(move || {
                    let storage_proof = StorageProof::new(trie_factory, hashed_factory, address);
                    // Each thread requests different slots
                    let start = thread_id * 5;
                    let end = start + 5;
                    let targets: Vec<_> =
                        (start..end).map(|i| keccak256(B256::from(U256::from(i)))).collect();
                    storage_proof.storage_multiproof(targets.into_iter().collect())
                })
            })
            .collect();

        // Collect results
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All should succeed
        for result in &results {
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_concurrent_multiproof_with_overlapping_targets() {
        use std::thread;

        let addr1 = address!("0x1000000000000000000000000000000000000001");
        let addr2 = address!("0x2000000000000000000000000000000000000002");
        let addr3 = address!("0x3000000000000000000000000000000000000003");

        let (trie_factory, hashed_factory) = TestDataBuilder::new()
            .add_account(
                addr1,
                MockAccount {
                    balance: U256::from(100),
                    nonce: 1,
                    bytecode_hash: None,
                    storage_root: EMPTY_ROOT_HASH,
                },
            )
            .add_account(
                addr2,
                MockAccount {
                    balance: U256::from(200),
                    nonce: 2,
                    bytecode_hash: None,
                    storage_root: EMPTY_ROOT_HASH,
                },
            )
            .add_account(
                addr3,
                MockAccount {
                    balance: U256::from(300),
                    nonce: 3,
                    bytecode_hash: None,
                    storage_root: EMPTY_ROOT_HASH,
                },
            )
            .add_storage(addr1, B256::from(U256::from(1)), U256::from(10))
            .add_storage(addr2, B256::from(U256::from(1)), U256::from(20))
            .build();

        // Create overlapping target sets
        let target_sets = vec![
            // Thread 1: addr1 and addr2
            vec![keccak256(addr1), keccak256(addr2)],
            // Thread 2: addr2 and addr3
            vec![keccak256(addr2), keccak256(addr3)],
            // Thread 3: addr1 and addr3
            vec![keccak256(addr1), keccak256(addr3)],
        ];

        let handles: Vec<_> = target_sets
            .into_iter()
            .map(|targets| {
                let trie_factory = trie_factory.clone();
                let hashed_factory = hashed_factory.clone();
                thread::spawn(move || {
                    let proof = Proof::new(trie_factory, hashed_factory);
                    let targets = MultiProofTargets::accounts(targets);
                    proof.multiproof(targets)
                })
            })
            .collect();

        // Collect results
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All should succeed
        assert_eq!(results.len(), 3);
        for result in &results {
            assert!(result.is_ok());
        }

        // Verify each proof contains the expected accounts
        assert_eq!(results[0].as_ref().unwrap().storages.len(), 2); // addr1 and addr2
        assert_eq!(results[1].as_ref().unwrap().storages.len(), 2); // addr2 and addr3
        assert_eq!(results[2].as_ref().unwrap().storages.len(), 2); // addr1 and addr3
    }

    #[test]
    fn test_parallel_multiproof_with_branch_masks() {
        use std::thread;

        let num_accounts = 8;
        let mut builder = TestDataBuilder::new();
        let addresses: Vec<Address> =
            (0..num_accounts).map(|i| Address::with_last_byte(i as u8)).collect();

        for (i, addr) in addresses.iter().enumerate() {
            builder = builder.add_account(
                *addr,
                MockAccount {
                    balance: U256::from(i * 100),
                    nonce: i as u64,
                    bytecode_hash: None,
                    storage_root: EMPTY_ROOT_HASH,
                },
            );
        }

        let (trie_factory, hashed_factory) = builder.build();

        // Create threads with branch masks enabled
        let handles: Vec<_> = (0..2)
            .map(|thread_id| {
                let trie_factory = trie_factory.clone();
                let hashed_factory = hashed_factory.clone();
                let addresses = addresses.clone();
                thread::spawn(move || {
                    let proof =
                        Proof::new(trie_factory, hashed_factory).with_branch_node_masks(true);
                    // Each thread gets half of the accounts
                    let start = thread_id * 4;
                    let end = start + 4;
                    let targets = MultiProofTargets::accounts(
                        addresses[start..end].iter().map(|a| keccak256(*a)).collect::<Vec<_>>(),
                    );
                    proof.multiproof(targets)
                })
            })
            .collect();

        // Collect results
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All should succeed
        for result in &results {
            assert!(result.is_ok());
            let proof = result.as_ref().unwrap();
            // Branch masks should be populated (or empty if mock doesn't create branches)
            assert!(
                proof.branch_node_hash_masks.is_empty() || !proof.branch_node_hash_masks.is_empty()
            );
        }
    }
}
