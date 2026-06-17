use crate::{
    ArenaCachedCursor, ArenaParallelSparseTrie, HashedCursorFactory, SparseStateTrie,
    TrieCursorFactory,
};
use alloy_primitives::{B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{BranchNodeCompact, Nibbles};

/// Hashed cursor factory backed by a [`SparseStateTrie`] and an inner cursor factory.
#[derive(Clone, Debug)]
pub struct SparseStateTrieCursorFactory<'a, H> {
    sparse_trie: &'a SparseStateTrie<ArenaParallelSparseTrie, ArenaParallelSparseTrie>,
    inner: H,
}

impl<'a, H> SparseStateTrieCursorFactory<'a, H> {
    /// Creates a new sparse-state cursor factory.
    pub const fn new(
        sparse_trie: &'a SparseStateTrie<ArenaParallelSparseTrie, ArenaParallelSparseTrie>,
        inner: H,
    ) -> Self {
        Self { sparse_trie, inner }
    }

    /// Returns the wrapped sparse state trie.
    pub const fn sparse_trie(
        &self,
    ) -> &'a SparseStateTrie<ArenaParallelSparseTrie, ArenaParallelSparseTrie> {
        self.sparse_trie
    }

    /// Returns a reference to the inner cursor factory.
    pub const fn inner(&self) -> &H {
        &self.inner
    }
}

impl<'sparse, H> HashedCursorFactory for SparseStateTrieCursorFactory<'sparse, H>
where
    H: HashedCursorFactory,
{
    type AccountCursor<'a>
        = ArenaCachedCursor<'sparse, H::AccountCursor<'a>, B256, Account>
    where
        Self: 'a;
    type StorageCursor<'a>
        = ArenaCachedCursor<'sparse, H::StorageCursor<'a>, B256, U256>
    where
        Self: 'a;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        let inner = self.inner.hashed_account_cursor()?;
        Ok(ArenaCachedCursor::new(self.sparse_trie.state_trie_ref(), inner))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        let inner = self.inner.hashed_storage_cursor(hashed_address)?;
        Ok(ArenaCachedCursor::new(self.sparse_trie.storage_trie_ref(&hashed_address), inner))
    }
}

/// Trie cursor factory backed by a [`SparseStateTrie`] and an inner trie cursor factory.
#[derive(Clone, Debug)]
pub struct SparseStateTrieTrieCursorFactory<'a, T> {
    sparse_trie: &'a SparseStateTrie<ArenaParallelSparseTrie, ArenaParallelSparseTrie>,
    inner: T,
}

impl<'a, T> SparseStateTrieTrieCursorFactory<'a, T> {
    /// Creates a new sparse-state trie cursor factory.
    pub const fn new(
        sparse_trie: &'a SparseStateTrie<ArenaParallelSparseTrie, ArenaParallelSparseTrie>,
        inner: T,
    ) -> Self {
        Self { sparse_trie, inner }
    }

    /// Returns the wrapped sparse state trie.
    pub const fn sparse_trie(
        &self,
    ) -> &'a SparseStateTrie<ArenaParallelSparseTrie, ArenaParallelSparseTrie> {
        self.sparse_trie
    }

    /// Returns a reference to the inner cursor factory.
    pub const fn inner(&self) -> &T {
        &self.inner
    }
}

impl<'sparse, T> TrieCursorFactory for SparseStateTrieTrieCursorFactory<'sparse, T>
where
    T: TrieCursorFactory,
{
    type AccountTrieCursor<'a>
        = ArenaCachedCursor<'sparse, T::AccountTrieCursor<'a>, Nibbles, BranchNodeCompact>
    where
        Self: 'a;
    type StorageTrieCursor<'a>
        = ArenaCachedCursor<'sparse, T::StorageTrieCursor<'a>, Nibbles, BranchNodeCompact>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        let inner = self.inner.account_trie_cursor()?;
        Ok(ArenaCachedCursor::new(self.sparse_trie.state_trie_ref(), inner))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        let inner = self.inner.storage_trie_cursor(hashed_address)?;
        Ok(ArenaCachedCursor::new(self.sparse_trie.storage_trie_ref(&hashed_address), inner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        HashedCursor, HashedStorageCursor, LeafUpdate, RevealableSparseTrie, SparseTrie,
        TrieCursor, TrieStorageCursor,
    };
    use alloy_primitives::{b256, map::B256Map, U256};
    use reth_trie_common::{
        BranchNodeV2, DecodedMultiProofV2, LeafNode, Nibbles, ProofTrieNodeV2, RlpNode, TrieMask,
        TrieNodeV2, EMPTY_ROOT_HASH,
    };
    use std::{
        collections::BTreeMap,
        fmt::Debug,
        ops::Bound::{Excluded, Unbounded},
    };

    const KEY_0: B256 = b256!("0x0000000000000000000000000000000000000000000000000000000000000000");
    const KEY_1: B256 = b256!("0x1000000000000000000000000000000000000000000000000000000000000000");
    const KEY_2: B256 = b256!("0x2000000000000000000000000000000000000000000000000000000000000000");
    const ADDRESS_0: B256 =
        b256!("0xa000000000000000000000000000000000000000000000000000000000000000");
    const ADDRESS_1: B256 =
        b256!("0xb000000000000000000000000000000000000000000000000000000000000000");

    fn account(nonce: u64) -> Account {
        Account { nonce, balance: U256::from(nonce), bytecode_hash: None }
    }

    fn leaf_key(suffix: impl AsRef<[u8]>, total_len: usize) -> Nibbles {
        let suffix = suffix.as_ref();
        let mut nibbles = Nibbles::from_nibbles(suffix);
        nibbles.extend(&Nibbles::from_nibbles_unchecked(vec![0; total_len - suffix.len()]));
        nibbles
    }

    fn account_leaf(account: Account) -> TrieNodeV2 {
        TrieNodeV2::Leaf(LeafNode::new(
            leaf_key([], 63),
            alloy_rlp::encode(account.into_trie_account(EMPTY_ROOT_HASH)),
        ))
    }

    fn storage_leaf(value: U256) -> TrieNodeV2 {
        TrieNodeV2::Leaf(LeafNode::new(
            leaf_key([], 63),
            alloy_rlp::encode_fixed_size(&value).to_vec(),
        ))
    }

    fn partial_two_leaf_proof(leaf_0: TrieNodeV2, leaf_1: TrieNodeV2) -> Vec<ProofTrieNodeV2> {
        let leaf_0_rlp = alloy_rlp::encode(&leaf_0);
        let leaf_1_rlp = alloy_rlp::encode(&leaf_1);
        let root = TrieNodeV2::Branch(BranchNodeV2 {
            key: Nibbles::default(),
            stack: vec![RlpNode::from_rlp(&leaf_0_rlp), RlpNode::from_rlp(&leaf_1_rlp)],
            state_mask: TrieMask::new(0b11),
            branch_rlp_node: None,
        });

        vec![
            ProofTrieNodeV2 { path: Nibbles::default(), node: root, masks: None },
            ProofTrieNodeV2 { path: Nibbles::from_nibbles([0x0]), node: leaf_0, masks: None },
        ]
    }

    #[derive(Clone, Debug, Default)]
    struct TestHashedCursorFactory {
        accounts: BTreeMap<B256, Account>,
        storage: B256Map<BTreeMap<B256, U256>>,
    }

    impl TestHashedCursorFactory {
        fn new(accounts: BTreeMap<B256, Account>, storage: B256Map<BTreeMap<B256, U256>>) -> Self {
            Self { accounts, storage }
        }
    }

    impl HashedCursorFactory for TestHashedCursorFactory {
        type AccountCursor<'a>
            = TestAccountCursor<Account>
        where
            Self: 'a;
        type StorageCursor<'a>
            = TestStorageCursor
        where
            Self: 'a;

        fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
            Ok(TestAccountCursor { values: self.accounts.clone(), current: None })
        }

        fn hashed_storage_cursor(
            &self,
            hashed_address: B256,
        ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
            Ok(TestStorageCursor { values: self.storage.clone(), hashed_address, current: None })
        }
    }

    #[derive(Debug)]
    struct TestAccountCursor<T> {
        values: BTreeMap<B256, T>,
        current: Option<B256>,
    }

    impl<T> HashedCursor for TestAccountCursor<T>
    where
        T: Debug + Clone,
    {
        type Value = T;

        fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
            let entry = self.values.range(key..).next().map(|(key, value)| (*key, value.clone()));
            self.current = entry.as_ref().map(|(key, _)| *key);
            Ok(entry)
        }

        fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
            let entry = match self.current {
                Some(current) => self
                    .values
                    .range((Excluded(current), Unbounded))
                    .next()
                    .map(|(key, value)| (*key, value.clone())),
                None => self.values.iter().next().map(|(key, value)| (*key, value.clone())),
            };
            self.current = entry.as_ref().map(|(key, _)| *key);
            Ok(entry)
        }

        fn reset(&mut self) {
            self.current = None;
        }
    }

    #[derive(Debug)]
    struct TestStorageCursor {
        values: B256Map<BTreeMap<B256, U256>>,
        hashed_address: B256,
        current: Option<B256>,
    }

    impl TestStorageCursor {
        fn current_values(&self) -> Option<&BTreeMap<B256, U256>> {
            self.values.get(&self.hashed_address)
        }
    }

    impl HashedCursor for TestStorageCursor {
        type Value = U256;

        fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
            let entry = self
                .current_values()
                .and_then(|values| values.range(key..).next())
                .map(|(key, value)| (*key, *value));
            self.current = entry.as_ref().map(|(key, _)| *key);
            Ok(entry)
        }

        fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
            let entry = match self.current {
                Some(current) => self
                    .current_values()
                    .and_then(|values| values.range((Excluded(current), Unbounded)).next())
                    .map(|(key, value)| (*key, *value)),
                None => self
                    .current_values()
                    .and_then(|values| values.iter().next())
                    .map(|(key, value)| (*key, *value)),
            };
            self.current = entry.as_ref().map(|(key, _)| *key);
            Ok(entry)
        }

        fn reset(&mut self) {
            self.current = None;
        }
    }

    impl HashedStorageCursor for TestStorageCursor {
        fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
            Ok(self.current_values().is_none_or(BTreeMap::is_empty))
        }

        fn set_hashed_address(&mut self, hashed_address: B256) {
            self.hashed_address = hashed_address;
            self.reset();
        }
    }

    #[derive(Clone, Debug, Default)]
    struct TestTrieCursorFactory {
        account_nodes: BTreeMap<Nibbles, BranchNodeCompact>,
        storage_nodes: B256Map<BTreeMap<Nibbles, BranchNodeCompact>>,
    }

    impl TestTrieCursorFactory {
        fn new(
            account_nodes: BTreeMap<Nibbles, BranchNodeCompact>,
            storage_nodes: B256Map<BTreeMap<Nibbles, BranchNodeCompact>>,
        ) -> Self {
            Self { account_nodes, storage_nodes }
        }
    }

    impl TrieCursorFactory for TestTrieCursorFactory {
        type AccountTrieCursor<'a>
            = TestTrieCursor
        where
            Self: 'a;
        type StorageTrieCursor<'a>
            = TestStorageTrieCursor
        where
            Self: 'a;

        fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
            Ok(TestTrieCursor { nodes: self.account_nodes.clone(), current: None })
        }

        fn storage_trie_cursor(
            &self,
            hashed_address: B256,
        ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
            Ok(TestStorageTrieCursor {
                nodes: self.storage_nodes.clone(),
                hashed_address,
                current: None,
            })
        }
    }

    #[derive(Debug)]
    struct TestTrieCursor {
        nodes: BTreeMap<Nibbles, BranchNodeCompact>,
        current: Option<Nibbles>,
    }

    impl TrieCursor for TestTrieCursor {
        fn seek_exact(
            &mut self,
            key: Nibbles,
        ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            let entry = self.nodes.get(&key).cloned().map(|node| (key, node));
            self.current = entry.as_ref().map(|(key, _)| *key);
            Ok(entry)
        }

        fn seek(
            &mut self,
            key: Nibbles,
        ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            let entry = self.nodes.range(key..).next().map(|(key, node)| (*key, node.clone()));
            self.current = entry.as_ref().map(|(key, _)| *key);
            Ok(entry)
        }

        fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            let entry = match self.current {
                Some(current) => self
                    .nodes
                    .range((Excluded(current), Unbounded))
                    .next()
                    .map(|(key, node)| (*key, node.clone())),
                None => self.nodes.iter().next().map(|(key, node)| (*key, node.clone())),
            };
            self.current = entry.as_ref().map(|(key, _)| *key);
            Ok(entry)
        }

        fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
            Ok(self.current)
        }

        fn reset(&mut self) {
            self.current = None;
        }
    }

    #[derive(Debug)]
    struct TestStorageTrieCursor {
        nodes: B256Map<BTreeMap<Nibbles, BranchNodeCompact>>,
        hashed_address: B256,
        current: Option<Nibbles>,
    }

    impl TestStorageTrieCursor {
        fn current_nodes(&self) -> Option<&BTreeMap<Nibbles, BranchNodeCompact>> {
            self.nodes.get(&self.hashed_address)
        }
    }

    impl TrieCursor for TestStorageTrieCursor {
        fn seek_exact(
            &mut self,
            key: Nibbles,
        ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            let entry = self
                .current_nodes()
                .and_then(|nodes| nodes.get(&key))
                .cloned()
                .map(|node| (key, node));
            self.current = entry.as_ref().map(|(key, _)| *key);
            Ok(entry)
        }

        fn seek(
            &mut self,
            key: Nibbles,
        ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            let entry = self
                .current_nodes()
                .and_then(|nodes| nodes.range(key..).next())
                .map(|(key, node)| (*key, node.clone()));
            self.current = entry.as_ref().map(|(key, _)| *key);
            Ok(entry)
        }

        fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            let entry = match self.current {
                Some(current) => self
                    .current_nodes()
                    .and_then(|nodes| nodes.range((Excluded(current), Unbounded)).next())
                    .map(|(key, node)| (*key, node.clone())),
                None => self
                    .current_nodes()
                    .and_then(|nodes| nodes.iter().next())
                    .map(|(key, node)| (*key, node.clone())),
            };
            self.current = entry.as_ref().map(|(key, _)| *key);
            Ok(entry)
        }

        fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
            Ok(self.current)
        }

        fn reset(&mut self) {
            self.current = None;
        }
    }

    impl TrieStorageCursor for TestStorageTrieCursor {
        fn set_hashed_address(&mut self, hashed_address: B256) {
            self.hashed_address = hashed_address;
            self.reset();
        }
    }

    fn empty_branch_node() -> BranchNodeCompact {
        BranchNodeCompact::default()
    }

    fn dirty_cursor_error(error: DatabaseError) -> bool {
        error.to_string().contains("dirty node")
    }

    #[test]
    fn blind_account_trie_delegates_to_inner_cursor() {
        let inner = TestHashedCursorFactory::new(
            BTreeMap::from([(KEY_1, account(1)), (KEY_2, account(2))]),
            B256Map::default(),
        );
        let sparse = SparseStateTrie::<ArenaParallelSparseTrie, ArenaParallelSparseTrie>::default();
        let factory = SparseStateTrieCursorFactory::new(&sparse, inner);

        let mut cursor = factory.hashed_account_cursor().unwrap();
        assert_eq!(cursor.seek(B256::ZERO).unwrap(), Some((KEY_1, account(1))));
        assert_eq!(cursor.next().unwrap(), Some((KEY_2, account(2))));
        assert_eq!(cursor.next().unwrap(), None);
    }

    #[test]
    fn account_cursor_uses_sparse_topology_before_inner_cursor() {
        let account_0 = account(10);
        let account_1 = account(11);
        let outside_account = account(12);

        let mut sparse =
            SparseStateTrie::<ArenaParallelSparseTrie, ArenaParallelSparseTrie>::default();
        sparse
            .reveal_decoded_multiproof_v2(DecodedMultiProofV2 {
                account_proofs: partial_two_leaf_proof(
                    account_leaf(account_0),
                    account_leaf(account_1),
                ),
                ..Default::default()
            })
            .unwrap();

        let inner = TestHashedCursorFactory::new(
            BTreeMap::from([
                (KEY_0, outside_account),
                (KEY_1, account_1),
                (KEY_2, outside_account),
            ]),
            B256Map::default(),
        );
        let factory = SparseStateTrieCursorFactory::new(&sparse, inner);

        let mut cursor = factory.hashed_account_cursor().unwrap();
        assert_eq!(cursor.seek(B256::ZERO).unwrap(), Some((KEY_0, account_0)));
        assert_eq!(cursor.next().unwrap(), Some((KEY_1, account_1)));
        assert_eq!(cursor.next().unwrap(), None);
    }

    #[test]
    fn storage_cursor_refreshes_sparse_topology_when_address_changes() {
        let value_0 = U256::from(20);
        let value_1 = U256::from(21);
        let outside_value = U256::from(22);
        let other_storage_value = U256::from(23);

        let mut sparse =
            SparseStateTrie::<ArenaParallelSparseTrie, ArenaParallelSparseTrie>::default();
        sparse.set_accounts_trie(RevealableSparseTrie::revealed_empty());
        sparse
            .reveal_decoded_multiproof_v2(DecodedMultiProofV2 {
                storage_proofs: B256Map::from_iter([(
                    ADDRESS_0,
                    partial_two_leaf_proof(storage_leaf(value_0), storage_leaf(value_1)),
                )]),
                ..Default::default()
            })
            .unwrap();

        let inner = TestHashedCursorFactory::new(
            BTreeMap::new(),
            B256Map::from_iter([
                (ADDRESS_0, BTreeMap::from([(KEY_1, value_1), (KEY_2, outside_value)])),
                (ADDRESS_1, BTreeMap::from([(KEY_0, other_storage_value)])),
            ]),
        );
        let factory = SparseStateTrieCursorFactory::new(&sparse, inner);

        let mut cursor = factory.hashed_storage_cursor(ADDRESS_0).unwrap();
        assert_eq!(cursor.seek(B256::ZERO).unwrap(), Some((KEY_0, value_0)));
        assert_eq!(cursor.next().unwrap(), Some((KEY_1, value_1)));
        assert_eq!(cursor.next().unwrap(), None);

        cursor.set_hashed_address(ADDRESS_1);
        assert_eq!(cursor.seek(B256::ZERO).unwrap(), Some((KEY_0, other_storage_value)));
        assert_eq!(cursor.next().unwrap(), None);
    }

    #[test]
    fn account_trie_cursor_returns_sparse_root_and_delegates_blinded_children() {
        let account_0 = account(30);
        let account_1 = account(31);
        let inner_node = empty_branch_node();

        let mut sparse =
            SparseStateTrie::<ArenaParallelSparseTrie, ArenaParallelSparseTrie>::default();
        sparse
            .reveal_decoded_multiproof_v2(DecodedMultiProofV2 {
                account_proofs: partial_two_leaf_proof(
                    account_leaf(account_0),
                    account_leaf(account_1),
                ),
                ..Default::default()
            })
            .unwrap();
        sparse.root().unwrap();

        let inner = TestTrieCursorFactory::new(
            BTreeMap::from([
                (Nibbles::from_nibbles([0x1, 0x2]), inner_node.clone()),
                (Nibbles::from_nibbles([0x2]), empty_branch_node()),
            ]),
            B256Map::default(),
        );
        let factory = SparseStateTrieTrieCursorFactory::new(&sparse, inner);

        let mut cursor = factory.account_trie_cursor().unwrap();
        let (root_path, root_node) = cursor.seek(Nibbles::default()).unwrap().unwrap();
        assert_eq!(root_path, Nibbles::default());
        assert_eq!(root_node.state_mask, TrieMask::new(0b11));
        assert_eq!(root_node.tree_mask, TrieMask::new(0b10));
        assert_eq!(root_node.hash_mask, TrieMask::new(0b11));
        assert_eq!(root_node.hashes.len(), 2);

        assert_eq!(cursor.next().unwrap(), Some((Nibbles::from_nibbles([0x1, 0x2]), inner_node)));
        assert_eq!(cursor.next().unwrap(), None);
    }

    #[test]
    fn storage_trie_cursor_refreshes_sparse_topology_when_address_changes() {
        let value_0 = U256::from(40);
        let value_1 = U256::from(41);
        let inner_node = empty_branch_node();
        let other_inner_node = BranchNodeCompact::new(
            TrieMask::new(0b1),
            TrieMask::default(),
            TrieMask::default(),
            vec![],
            None,
        );

        let mut sparse =
            SparseStateTrie::<ArenaParallelSparseTrie, ArenaParallelSparseTrie>::default();
        sparse.set_accounts_trie(RevealableSparseTrie::revealed_empty());
        sparse
            .reveal_decoded_multiproof_v2(DecodedMultiProofV2 {
                storage_proofs: B256Map::from_iter([(
                    ADDRESS_0,
                    partial_two_leaf_proof(storage_leaf(value_0), storage_leaf(value_1)),
                )]),
                ..Default::default()
            })
            .unwrap();
        sparse.storage_root(&ADDRESS_0).unwrap();

        let inner = TestTrieCursorFactory::new(
            BTreeMap::new(),
            B256Map::from_iter([
                (
                    ADDRESS_0,
                    BTreeMap::from([
                        (Nibbles::from_nibbles([0x1, 0x2]), inner_node.clone()),
                        (Nibbles::from_nibbles([0x2]), empty_branch_node()),
                    ]),
                ),
                (
                    ADDRESS_1,
                    BTreeMap::from([(Nibbles::from_nibbles([0x0, 0x1]), other_inner_node.clone())]),
                ),
            ]),
        );
        let factory = SparseStateTrieTrieCursorFactory::new(&sparse, inner);

        let mut cursor = factory.storage_trie_cursor(ADDRESS_0).unwrap();
        assert_eq!(cursor.seek(Nibbles::default()).unwrap().unwrap().0, Nibbles::default());
        assert_eq!(cursor.next().unwrap(), Some((Nibbles::from_nibbles([0x1, 0x2]), inner_node)));
        assert_eq!(cursor.next().unwrap(), None);

        cursor.set_hashed_address(ADDRESS_1);
        assert_eq!(
            cursor.seek(Nibbles::default()).unwrap(),
            Some((Nibbles::from_nibbles([0x0, 0x1]), other_inner_node))
        );
        assert_eq!(cursor.next().unwrap(), None);
    }

    #[test]
    fn cursors_error_when_sparse_nodes_are_dirty() {
        let account_0 = account(50);
        let account_1 = account(51);

        let mut sparse =
            SparseStateTrie::<ArenaParallelSparseTrie, ArenaParallelSparseTrie>::default();
        sparse
            .reveal_decoded_multiproof_v2(DecodedMultiProofV2 {
                account_proofs: partial_two_leaf_proof(
                    account_leaf(account_0),
                    account_leaf(account_1),
                ),
                ..Default::default()
            })
            .unwrap();
        sparse.root().unwrap();
        let mut updates = B256Map::from_iter([(
            KEY_0,
            LeafUpdate::Changed(alloy_rlp::encode(account(52).into_trie_account(EMPTY_ROOT_HASH))),
        )]);
        sparse
            .trie_mut()
            .as_revealed_mut()
            .unwrap()
            .update_leaves(&mut updates, |_, _| panic!("leaf should already be revealed"))
            .unwrap();

        let hashed_factory = SparseStateTrieCursorFactory::new(
            &sparse,
            TestHashedCursorFactory::new(BTreeMap::new(), B256Map::default()),
        );
        let mut hashed_cursor = hashed_factory.hashed_account_cursor().unwrap();
        assert!(dirty_cursor_error(hashed_cursor.seek(B256::ZERO).unwrap_err()));

        let trie_factory =
            SparseStateTrieTrieCursorFactory::new(&sparse, TestTrieCursorFactory::default());
        let mut trie_cursor = trie_factory.account_trie_cursor().unwrap();
        assert!(dirty_cursor_error(trie_cursor.seek(Nibbles::default()).unwrap_err()));
    }
}
