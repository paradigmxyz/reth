use crate::{hashed_cursor::HashedCursor, trie_cursor::TrieCursor, walker::TrieWalker, Nibbles};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;
use tracing::{instrument, trace};

/// Represents a branch node in the trie.
#[derive(Debug)]
pub struct TrieBranchNode {
    /// The key associated with the node.
    pub key: Nibbles,
    /// The value associated with the node.
    pub value: B256,
    /// Indicates whether children are in the trie.
    pub children_are_in_trie: bool,
}

impl TrieBranchNode {
    /// Creates a new `TrieBranchNode`.
    pub const fn new(key: Nibbles, value: B256, children_are_in_trie: bool) -> Self {
        Self { key, value, children_are_in_trie }
    }
}

/// Represents variants of trie nodes returned by the iteration.
#[derive(Debug)]
pub enum TrieElement<Value> {
    /// Branch node.
    Branch(TrieBranchNode),
    /// Leaf node with hash instead of the actual value.
    LeafHash(Nibbles, B256),
    /// Leaf node.
    Leaf(B256, Value),
}

/// Result of calling [`HashedCursor::seek`].
#[derive(Debug)]
struct SeekedHashedEntry<V> {
    /// The key that was seeked.
    seeked_key: B256,
    /// The result of the seek.

    /// If no entry was found for the provided key, this will be [`None`].
    result: Option<(B256, V)>,
}

/// An iterator over existing intermediate branch nodes and updated leaf nodes.
#[derive(Debug)]
pub struct TrieNodeIter<C, H: HashedCursor> {
    /// The walker over intermediate nodes.
    pub walker: TrieWalker<C>,
    /// The cursor for the hashed entries.
    pub hashed_cursor: H,
    /// The type of the trie.
    trie_type: crate::TrieType,
    /// The previous hashed key. If the iteration was previously interrupted, this value can be
    /// used to resume iterating from the last returned leaf node.
    previous_hashed_key: Option<B256>,

    /// Current hashed  entry.
    current_hashed_entry: Option<(B256, H::Value)>,
    /// Flag indicating whether we should check the current walker key.
    should_check_walker_key: bool,

    /// The last seeked hashed entry.
    ///
    /// We use it to not seek the same hashed entry twice, and instead reuse it.
    last_seeked_hashed_entry: Option<SeekedHashedEntry<H::Value>>,

    #[cfg(feature = "metrics")]
    metrics: crate::metrics::TrieNodeIterMetrics,
    /// The key that the [`HashedCursor`] previously advanced to using [`HashedCursor::next`].
    #[cfg(feature = "metrics")]
    previously_advanced_to_key: Option<B256>,
}

impl<C, H: HashedCursor> TrieNodeIter<C, H>
where
    H::Value: Copy,
{
    /// Creates a new [`TrieNodeIter`] for the state trie.
    pub fn state_trie(walker: TrieWalker<C>, hashed_cursor: H) -> Self {
        Self::new(walker, hashed_cursor, crate::TrieType::State)
    }

    /// Creates a new [`TrieNodeIter`] for the storage trie.
    pub fn storage_trie(walker: TrieWalker<C>, hashed_cursor: H) -> Self {
        Self::new(walker, hashed_cursor, crate::TrieType::Storage)
    }

    /// Creates a new [`TrieNodeIter`].
    fn new(walker: TrieWalker<C>, hashed_cursor: H, trie_type: crate::TrieType) -> Self {
        Self {
            walker,
            hashed_cursor,
            trie_type,
            previous_hashed_key: None,
            current_hashed_entry: None,
            should_check_walker_key: false,
            last_seeked_hashed_entry: None,
            #[cfg(feature = "metrics")]
            metrics: crate::metrics::TrieNodeIterMetrics::new(trie_type),
            #[cfg(feature = "metrics")]
            previously_advanced_to_key: None,
        }
    }

    /// Sets the last iterated hashed key and returns the modified [`TrieNodeIter`].
    /// This is used to resume iteration from the last checkpoint.
    pub const fn with_last_hashed_key(mut self, previous_hashed_key: B256) -> Self {
        self.previous_hashed_key = Some(previous_hashed_key);
        self
    }

    /// Seeks the hashed cursor to the given key.
    ///
    /// If the key is the same as the last seeked key, the result of the last seek is returned.
    ///
    /// If `metrics` feature is enabled, also updates the metrics.
    fn seek_hashed_entry(&mut self, key: B256) -> Result<Option<(B256, H::Value)>, DatabaseError> {
        if let Some(entry) = self
            .last_seeked_hashed_entry
            .as_ref()
            .filter(|entry| entry.seeked_key == key)
            .map(|entry| entry.result)
        {
            #[cfg(feature = "metrics")]
            self.metrics.inc_leaf_nodes_same_seeked();
            return Ok(entry);
        }

        let result = self.hashed_cursor.seek(key)?;
        self.last_seeked_hashed_entry = Some(SeekedHashedEntry { seeked_key: key, result });

        #[cfg(feature = "metrics")]
        {
            self.metrics.inc_leaf_nodes_seeked();

            if Some(key) == self.previously_advanced_to_key {
                self.metrics.inc_leaf_nodes_same_seeked_as_advanced();
            }
        }
        Ok(result)
    }

    /// Advances the hashed cursor to the next entry.
    ///
    /// If `metrics` feature is enabled, also updates the metrics.
    fn next_hashed_entry(&mut self) -> Result<Option<(B256, H::Value)>, DatabaseError> {
        let result = self.hashed_cursor.next();
        #[cfg(feature = "metrics")]
        {
            self.metrics.inc_leaf_nodes_advanced();

            self.previously_advanced_to_key =
                result.as_ref().ok().and_then(|result| result.as_ref().map(|(k, _)| *k));
        }
        result
    }
}

impl<C, H> TrieNodeIter<C, H>
where
    C: TrieCursor,
    H: HashedCursor,
    H::Value: Copy,
{
    /// Return the next trie node to be added to the hash builder.
    ///
    /// Returns the nodes using this algorithm:
    /// 1. Return the current intermediate branch node if it hasn't been updated.
    /// 2. Advance the trie walker to the next intermediate branch node and retrieve next
    ///    unprocessed key.
    /// 3. Reposition the hashed cursor on the next unprocessed key.
    /// 4. Return every hashed entry up to the key of the current intermediate branch node.
    /// 5. Repeat.
    ///
    /// NOTE: The iteration will start from the key of the previous hashed entry if it was supplied.
    #[instrument(
        level = "trace",
        target = "trie::node_iter",
        skip_all,
        fields(trie_type = ?self.trie_type),
        ret
    )]
    pub fn try_next(
        &mut self,
    ) -> Result<Option<TrieElement<<H as HashedCursor>::Value>>, DatabaseError> {
        loop {
            // If the walker has a key...
            if let Some(key) = self.walker.key() {
                // Ensure that the current walker key shouldn't be checked and there's no previous
                // hashed key
                if !self.should_check_walker_key && self.previous_hashed_key.is_none() {
                    // Make sure we check the next walker key, because we only know we can skip the
                    // current one.
                    self.should_check_walker_key = true;
                    // If it's possible to skip the current node in the walker, return a branch node
                    if self.walker.can_skip_current_node {
                        let hash = self.walker.hash().unwrap();
                        let tree_flag = self.walker.children_are_in_trie();
                        if self.walker.all_branch_nodes_in_database && !tree_flag {
                            return Ok(Some(TrieElement::LeafHash(key.clone(), hash)))
                        }

                        #[cfg(feature = "metrics")]
                        self.metrics.inc_branch_nodes_returned();
                        return Ok(Some(TrieElement::Branch(TrieBranchNode::new(
                            key.clone(),
                            hash,
                            tree_flag,
                        ))))
                    }
                }
            }

            // If there's a hashed entry...
            if let Some((hashed_key, value)) = self.current_hashed_entry.take() {
                // Check if the walker's key is less than the key of the current hashed entry
                if self.walker.key().is_some_and(|key| key < &Nibbles::unpack(hashed_key)) {
                    self.should_check_walker_key = false;
                    continue
                }

                // Set the next hashed entry as a leaf node and return
                trace!(target: "trie::node_iter", ?hashed_key, "next hashed entry");
                self.current_hashed_entry = self.next_hashed_entry()?;

                #[cfg(feature = "metrics")]
                self.metrics.inc_leaf_nodes_returned();
                return Ok(Some(TrieElement::Leaf(hashed_key, value)))
            }

            // Handle seeking and advancing based on the previous hashed key
            match self.previous_hashed_key.take() {
                Some(hashed_key) => {
                    trace!(target: "trie::node_iter", ?hashed_key, "seeking to the previous hashed entry");
                    // Seek to the previous hashed key and get the next hashed entry
                    self.seek_hashed_entry(hashed_key)?;
                    self.current_hashed_entry = self.next_hashed_entry()?;
                }
                None => {
                    // Get the seek key and set the current hashed entry based on walker's next
                    // unprocessed key
                    let (seek_key, seek_prefix) = match self.walker.next_unprocessed_key() {
                        Some(key) => key,
                        None => break, // no more keys
                    };

                    trace!(
                        target: "trie::node_iter",
                        ?seek_key,
                        can_skip_current_node = self.walker.can_skip_current_node,
                        last = ?self.walker.stack.last(),
                        "seeking to the next unprocessed hashed entry"
                    );
                    let can_skip_node = self.walker.can_skip_current_node;
                    self.walker.advance()?;
                    trace!(
                        target: "trie::node_iter",
                        last = ?self.walker.stack.last(),
                        "advanced walker"
                    );

                    // We should get the iterator to return a branch node if we can skip the
                    // current node and the tree flag for the current node is set.
                    //
                    // `can_skip_node` is already set when the hash flag is set, so we don't need
                    // to check for the hash flag explicitly.
                    //
                    // It is possible that the branch node at the key `seek_key` is not stored in
                    // the database, so the walker will advance to the branch node after it. Because
                    // of this, we need to check that the current walker key has a prefix of the key
                    // that we seeked to.
                    let skip_hashed_seek = can_skip_node &&
                        self.walker.key().is_some_and(|key| key.has_prefix(&seek_prefix)) &&
                        self.walker.children_are_in_trie();
                    trace!(
                        target: "trie::node_iter",
                        can_skip_node,
                        walker_key = ?self.walker.key(),
                        ?seek_key,
                        all_branch_nodes_in_database = self.walker.all_branch_nodes_in_database,
                        children_are_in_trie = self.walker.children_are_in_trie(),
                        skip_hashed_seek,
                        "checking if we can skip hashed seek"
                    );
                    if skip_hashed_seek {
                        trace!(
                            target: "trie::node_iter",
                            ?seek_key,
                            walker_hash = ?self.walker.maybe_hash(),
                            "skipping hashed seek"
                        );

                        self.should_check_walker_key = false;
                        continue
                    }

                    self.current_hashed_entry = self.seek_hashed_entry(seek_key)?;
                }
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        hashed_cursor::{
            mock::MockHashedCursorFactory, noop::NoopHashedAccountCursor, HashedCursorFactory,
            HashedPostStateAccountCursor,
        },
        mock::{CursorType, KeyVisit},
        trie_cursor::{
            mock::MockTrieCursorFactory, noop::NoopAccountTrieCursor, TrieCursorFactory,
        },
        walker::TrieWalker,
    };
    use alloy_primitives::{
        b256,
        map::{B256Map, B256Set, HashMap},
    };
    use alloy_trie::{
        BranchNodeCompact, HashBuilder, Nibbles, TrieAccount, TrieMask, EMPTY_ROOT_HASH,
    };
    use itertools::Itertools;
    use reth_primitives_traits::Account;
    use reth_trie_common::{
        prefix_set::PrefixSetMut, updates::TrieUpdates, BranchNode, HashedPostState, LeafNode,
        RlpNode,
    };
    use std::collections::BTreeMap;

    use super::{TrieElement, TrieNodeIter};

    /// Calculate the branch node stored in the database by feeding the provided state to the hash
    /// builder and taking the trie updates.
    fn get_hash_builder_branch_nodes(
        state: impl IntoIterator<Item = (Nibbles, Account)> + Clone,
    ) -> HashMap<Nibbles, BranchNodeCompact> {
        let mut hash_builder = HashBuilder::default().with_updates(true);

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.extend_keys(state.clone().into_iter().map(|(nibbles, _)| nibbles));
        let walker =
            TrieWalker::state_trie(NoopAccountTrieCursor, prefix_set.freeze(), &B256Set::default());

        let hashed_post_state = HashedPostState::default()
            .with_accounts(state.into_iter().map(|(nibbles, account)| {
                (nibbles.pack().into_inner().unwrap().into(), Some(account))
            }))
            .into_sorted();

        let mut node_iter = TrieNodeIter::state_trie(
            walker,
            HashedPostStateAccountCursor::new(
                NoopHashedAccountCursor::default(),
                hashed_post_state.accounts(),
            ),
        );

        while let Some(node) = node_iter.try_next().unwrap() {
            match node {
                TrieElement::Branch(branch) => {
                    hash_builder.add_branch(branch.key, branch.value, branch.children_are_in_trie);
                }
                TrieElement::LeafHash(key, hash) => {
                    hash_builder.add_leaf_hash(key, hash);
                }
                TrieElement::Leaf(key, account) => {
                    hash_builder.add_leaf(
                        Nibbles::unpack(key),
                        &alloy_rlp::encode(account.into_trie_account(EMPTY_ROOT_HASH)),
                    );
                }
            }
        }
        hash_builder.root();

        let mut trie_updates = TrieUpdates::default();
        trie_updates.finalize(hash_builder, Default::default(), Default::default());

        trie_updates.account_nodes
    }

    #[test]
    fn test_trie_node_iter() {
        fn empty_leaf_rlp_for_key(key: Nibbles) -> RlpNode {
            RlpNode::from_rlp(&alloy_rlp::encode(LeafNode::new(
                key,
                alloy_rlp::encode(TrieAccount::default()),
            )))
        }

        reth_tracing::init_test_tracing();

        // Extension (Key = 0x0000000000000000000000000000000000000000000000000000000000000)
        // └── Branch (`branch_node_0`)
        //     ├── 0 -> Branch (`branch_node_1`)
        //     │      ├── 0 -> Leaf (`account_1`, Key = 0x0)
        //     │      └── 1 -> Leaf (`account_2`, Key = 0x0)
        //     ├── 1 -> Branch (`branch_node_2`)
        //     │      ├── 0 -> Branch (`branch_node_3`)
        //     │      │      ├── 0 -> Leaf (`account_3`)
        //     │      │      └── 1 -> Leaf (`account_4`)
        //     │      └── 1 -> Leaf (`account_5`, Key = 0x0, marked as changed)

        let account_1 = b256!("0x0000000000000000000000000000000000000000000000000000000000000000");
        let account_2 = b256!("0x0000000000000000000000000000000000000000000000000000000000000010");
        let account_3 = b256!("0x0000000000000000000000000000000000000000000000000000000000000100");
        let account_4 = b256!("0x0000000000000000000000000000000000000000000000000000000000000101");
        let account_5 = b256!("0x0000000000000000000000000000000000000000000000000000000000000110");
        let empty_account = Account::default();

        let hash_builder_branch_nodes = get_hash_builder_branch_nodes(vec![
            (Nibbles::unpack(account_1), empty_account),
            (Nibbles::unpack(account_2), empty_account),
            (Nibbles::unpack(account_3), empty_account),
            (Nibbles::unpack(account_4), empty_account),
            (Nibbles::unpack(account_5), empty_account),
        ]);

        let branch_node_1 = (
            Nibbles::from_nibbles([0; 62]),
            BranchNodeCompact::new(
                TrieMask::new(0b11),
                TrieMask::new(0b00),
                TrieMask::new(0b11),
                vec![
                    empty_leaf_rlp_for_key(Nibbles::from_nibbles([0])),
                    empty_leaf_rlp_for_key(Nibbles::from_nibbles([0])),
                ],
                None,
            ),
        );
        let branch_node_1_rlp = RlpNode::from_rlp(&alloy_rlp::encode(BranchNode::new(
            vec![
                empty_leaf_rlp_for_key(Nibbles::from_nibbles([0])),
                empty_leaf_rlp_for_key(Nibbles::from_nibbles([0])),
            ],
            TrieMask::new(0b11),
        )));

        let branch_node_3 = (
            Nibbles::from_nibbles([vec![0; 61], vec![1, 0]].concat()),
            BranchNodeCompact::new(
                TrieMask::new(0b11),
                TrieMask::new(0b00),
                TrieMask::new(0b11),
                vec![
                    empty_leaf_rlp_for_key(Nibbles::default()),
                    empty_leaf_rlp_for_key(Nibbles::default()),
                ],
                None,
            ),
        );
        let branch_node_3_rlp = RlpNode::from_rlp(&alloy_rlp::encode(BranchNode::new(
            vec![
                empty_leaf_rlp_for_key(Nibbles::default()),
                empty_leaf_rlp_for_key(Nibbles::default()),
            ],
            TrieMask::new(0b11),
        )));

        let branch_node_2 = (
            Nibbles::from_nibbles([vec![0; 61], vec![1]].concat()),
            BranchNodeCompact::new(
                TrieMask::new(0b11),
                TrieMask::new(0b01),
                TrieMask::new(0b11),
                vec![branch_node_3_rlp.clone(), empty_leaf_rlp_for_key(Nibbles::from_nibbles([0]))],
                None,
            ),
        );
        let branch_node_2_rlp = RlpNode::from_rlp(&alloy_rlp::encode(BranchNode::new(
            vec![branch_node_3_rlp, empty_leaf_rlp_for_key(Nibbles::from_nibbles([0]))],
            TrieMask::new(0b11),
        )));
        let branch_node_0 = (
            Nibbles::from_nibbles([0; 61]),
            BranchNodeCompact::new(
                TrieMask::new(0b11),
                TrieMask::new(0b11),
                TrieMask::new(0b11),
                vec![branch_node_1_rlp, branch_node_2_rlp],
                None,
            ),
        );

        let mock_trie_nodes =
            vec![branch_node_0.clone(), branch_node_1, branch_node_2.clone(), branch_node_3];
        pretty_assertions::assert_eq!(
            hash_builder_branch_nodes.into_iter().sorted().collect::<Vec<_>>(),
            mock_trie_nodes,
        );

        let trie_cursor_factory =
            MockTrieCursorFactory::new(mock_trie_nodes.into_iter().collect(), B256Map::default());

        // Mark the account 5 as changed.
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(account_5));
        let prefix_set = prefix_set.freeze();

        let walker = TrieWalker::state_trie(
            trie_cursor_factory.account_trie_cursor().unwrap(),
            prefix_set,
            &B256Set::default(),
        );

        let hashed_cursor_factory = MockHashedCursorFactory::new(
            BTreeMap::from([
                (account_1, empty_account),
                (account_2, empty_account),
                (account_3, empty_account),
                (account_4, empty_account),
                (account_5, empty_account),
            ]),
            B256Map::default(),
        );

        let mut iter = TrieNodeIter::state_trie(
            walker,
            hashed_cursor_factory.hashed_account_cursor().unwrap(),
        );

        // Walk the iterator until it's exhausted.
        while iter.try_next().unwrap().is_some() {}

        let visited_account_nodes = trie_cursor_factory
            .visited_account_keys()
            .iter()
            .cloned()
            .map(CursorType::Trie)
            .chain(
                hashed_cursor_factory
                    .visited_account_keys()
                    .iter()
                    .cloned()
                    .map(CursorType::Hashed),
            )
            .sorted()
            .collect::<Vec<_>>();

        pretty_assertions::assert_eq!(
            visited_account_nodes,
            vec![
                CursorType::Trie(KeyVisit::seek_exact(Nibbles::default(), None)),
                CursorType::Trie(KeyVisit::seek_non_exact(
                    Nibbles::from_nibbles([0x0]),
                    Some(branch_node_0.0)
                )),
                CursorType::Hashed(KeyVisit::seek_non_exact(account_1, Some(account_1))),
                CursorType::Trie(KeyVisit::seek_non_exact(
                    branch_node_2.0.clone(),
                    Some(branch_node_2.0)
                )),
                CursorType::Hashed(KeyVisit::seek_non_exact(account_3, Some(account_3))),
                CursorType::Hashed(KeyVisit::seek_non_exact(account_5, Some(account_5))),
                CursorType::Trie(KeyVisit::seek_non_exact(Nibbles::from_nibbles([0x1]), None)),
                CursorType::Hashed(KeyVisit::next(None)),
            ]
        );
    }
}
