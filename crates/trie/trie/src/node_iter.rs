use crate::{hashed_cursor::HashedCursor, trie_cursor::TrieCursor, walker::TrieWalker, Nibbles};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;
use tracing::trace;

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
    /// Leaf node.
    Leaf(B256, Value),
}

/// An iterator over existing intermediate branch nodes and updated leaf nodes.
#[derive(Debug)]
pub struct TrieNodeIter<C, H: HashedCursor> {
    /// The walker over intermediate nodes.
    pub walker: TrieWalker<C>,
    /// The cursor for the hashed entries.
    pub hashed_cursor: H,
    /// The previous hashed key. If the iteration was previously interrupted, this value can be
    /// used to resume iterating from the last returned leaf node.
    previous_hashed_key: Option<B256>,

    /// Current hashed  entry.
    current_hashed_entry: Option<(B256, <H as HashedCursor>::Value)>,
    /// Flag indicating whether we should check the current walker key.
    current_walker_key_checked: bool,
}

impl<C, H: HashedCursor> TrieNodeIter<C, H> {
    /// Creates a new [`TrieNodeIter`].
    pub const fn new(walker: TrieWalker<C>, hashed_cursor: H) -> Self {
        Self {
            walker,
            hashed_cursor,
            previous_hashed_key: None,
            current_hashed_entry: None,
            current_walker_key_checked: false,
        }
    }

    /// Sets the last iterated hashed key and returns the modified [`TrieNodeIter`].
    /// This is used to resume iteration from the last checkpoint.
    pub const fn with_last_hashed_key(mut self, previous_hashed_key: B256) -> Self {
        self.previous_hashed_key = Some(previous_hashed_key);
        self
    }
}

impl<C, H> TrieNodeIter<C, H>
where
    C: TrieCursor,
    H: HashedCursor,
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
    pub fn try_next(
        &mut self,
    ) -> Result<Option<TrieElement<<H as HashedCursor>::Value>>, DatabaseError> {
        loop {
            // If the walker has a key...
            if let Some(key) = self.walker.key() {
                // Check if the current walker key is unchecked and there's no previous hashed key
                if !self.current_walker_key_checked && self.previous_hashed_key.is_none() {
                    self.current_walker_key_checked = true;
                    // If it's possible to skip the current node in the walker, return a branch node
                    if self.walker.can_skip_current_node {
                        return Ok(Some(TrieElement::Branch(TrieBranchNode::new(
                            key.clone(),
                            self.walker.hash().unwrap(),
                            self.walker.children_are_in_trie(),
                        ))))
                    }
                }
            }

            // If there's a hashed entry...
            if let Some((hashed_key, value)) = self.current_hashed_entry.take() {
                // If the walker's key is less than the unpacked hashed key,
                // reset the checked status and continue
                if self.walker.key().is_some_and(|key| key < &Nibbles::unpack(hashed_key)) {
                    self.current_walker_key_checked = false;
                    continue
                }

                // Set the next hashed entry as a leaf node and return
                self.current_hashed_entry = self.hashed_cursor.next()?;
                trace!(
                    target: "trie::node_iter",
                    seek_from = ?hashed_key,
                    seek_to = ?self.current_hashed_entry.as_ref().map(|(k, _)| k),
                    "seeking to the next hashed entry"
                );
                return Ok(Some(TrieElement::Leaf(hashed_key, value)))
            }

            // Handle seeking and advancing based on the previous hashed key
            match self.previous_hashed_key.take() {
                Some(hashed_key) => {
                    trace!(target: "trie::node_iter", ?hashed_key, "seeking to the previous hashed entry");
                    // Seek to the previous hashed key and get the next hashed entry
                    self.hashed_cursor.seek(hashed_key)?;
                    self.current_hashed_entry = self.hashed_cursor.next()?;
                }
                None => {
                    // Get the seek key and set the current hashed entry based on walker's next
                    // unprocessed key
                    let seek_key = match self.walker.next_unprocessed_key() {
                        Some(key) => key,
                        None => break, // no more keys
                    };
                    trace!(
                        target: "trie::node_iter",
                        ?seek_key,
                        can_skip_current_node = self.walker.can_skip_current_node,
                        "seeking to the next unprocessed hashed entry"
                    );
                    self.current_hashed_entry = self.hashed_cursor.seek(seek_key)?;
                    self.walker.advance()?;
                }
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{
        b256,
        map::{B256Map, HashMap},
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

    use crate::{
        hashed_cursor::{
            mock::MockHashedCursorFactory, noop::NoopHashedAccountCursor, HashedCursorFactory,
            HashedPostStateAccountCursor,
        },
        mock::{KeyVisit, KeyVisitType},
        trie_cursor::{
            mock::MockTrieCursorFactory, noop::NoopAccountTrieCursor, TrieCursorFactory,
        },
        walker::TrieWalker,
    };

    use super::{TrieElement, TrieNodeIter};

    /// Calculate the branch node stored in the database by feeding the provided state to the hash
    /// builder and taking the trie updates.
    fn get_hash_builder_branch_nodes(
        state: impl IntoIterator<Item = (Nibbles, Account)> + Clone,
    ) -> HashMap<Nibbles, BranchNodeCompact> {
        let mut hash_builder = HashBuilder::default().with_updates(true);

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.extend_keys(state.clone().into_iter().map(|(nibbles, _)| nibbles));
        let walker = TrieWalker::new(NoopAccountTrieCursor, prefix_set.freeze());

        let hashed_post_state = HashedPostState::default()
            .with_accounts(state.into_iter().map(|(nibbles, account)| {
                (nibbles.pack().into_inner().unwrap().into(), Some(account))
            }))
            .into_sorted();

        let mut node_iter = TrieNodeIter::new(
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

        // Extension (Key = 0x0000000000000000000000000000000000000000000000000000000000000)
        // └── Branch (`branch_node_0`)
        //     ├── 0 -> Branch (`branch_node_1`)
        //     │      ├── 0 -> Leaf (`account_1`, Key = 0x0)
        //     │      ├── 1 -> Branch (`branch_node_2`)
        //     │      │      ├── 0 -> Leaf (`account_2`)
        //     │      │      ├── 1 -> Leaf (`account_3`)
        //     │      │      └── 2 -> Leaf (`account_4`)
        //     │      └── 2 -> Leaf (`account_5`, Key = 0x0)
        //     ├── 1 -> Branch (`branch_node_3`)
        //     │      ├── 0 -> Leaf (`account_6`, Key = 0x0)
        //     │      ├── 1 -> Branch (`branch_node_2`)
        //     │      │      ├── 0 -> Leaf (`account_7`)
        //     │      │      ├── 1 -> Leaf (`account_8`)
        //     │      │      └── 2 -> Leaf (`account_9`)
        //     │      └── 2 -> Leaf (`account_10`, Key = 0x0)
        //     └── 2 -> Branch (`branch_node_4`)
        //            ├── 0 -> Leaf (`account_11`, Key = 0x0)
        //            ├── 1 -> Branch (`branch_node_2`)
        //            │      ├── 0 -> Leaf (`account_12`)
        //            │      ├── 1 -> Leaf (`account_13`, marked as changed)
        //            │      └── 2 -> Leaf (`account_14`)
        //            └── 2 -> Leaf (`account_15`, Key = 0x0)

        let account_1 = b256!("0x0000000000000000000000000000000000000000000000000000000000000000");
        let account_2 = b256!("0x0000000000000000000000000000000000000000000000000000000000000010");
        let account_3 = b256!("0x0000000000000000000000000000000000000000000000000000000000000011");
        let account_4 = b256!("0x0000000000000000000000000000000000000000000000000000000000000012");
        let account_5 = b256!("0x0000000000000000000000000000000000000000000000000000000000000020");
        let account_6 = b256!("0x0000000000000000000000000000000000000000000000000000000000000100");
        let account_7 = b256!("0x0000000000000000000000000000000000000000000000000000000000000110");
        let account_8 = b256!("0x0000000000000000000000000000000000000000000000000000000000000111");
        let account_9 = b256!("0x0000000000000000000000000000000000000000000000000000000000000112");
        let account_10 =
            b256!("0x0000000000000000000000000000000000000000000000000000000000000120");
        let account_11 =
            b256!("0x0000000000000000000000000000000000000000000000000000000000000200");
        let account_12 =
            b256!("0x0000000000000000000000000000000000000000000000000000000000000210");
        let account_13 =
            b256!("0x0000000000000000000000000000000000000000000000000000000000000211");
        let account_14 =
            b256!("0x0000000000000000000000000000000000000000000000000000000000000212");
        let account_15 =
            b256!("0x0000000000000000000000000000000000000000000000000000000000000220");
        let empty_account = Account::default();

        let accounts = vec![
            (account_1, empty_account),
            (account_2, empty_account),
            (account_3, empty_account),
            (account_4, empty_account),
            (account_5, empty_account),
            (account_6, empty_account),
            (account_7, empty_account),
            (account_8, empty_account),
            (account_9, empty_account),
            (account_10, empty_account),
            (account_11, empty_account),
            (account_12, empty_account),
            (account_13, empty_account),
            (account_14, empty_account),
            (account_15, empty_account),
        ];

        let hash_builder_branch_nodes =
            get_hash_builder_branch_nodes(accounts.iter().map(|(k, v)| (Nibbles::unpack(*k), *v)));

        let branch_node_2_rlp = RlpNode::from_rlp(&alloy_rlp::encode(BranchNode::new(
            vec![
                empty_leaf_rlp_for_key(Nibbles::default()),
                empty_leaf_rlp_for_key(Nibbles::default()),
                empty_leaf_rlp_for_key(Nibbles::default()),
            ],
            TrieMask::new(0b111),
        )));

        let branch_node_1_3_4 = BranchNodeCompact::new(
            TrieMask::new(0b111),
            TrieMask::new(0b000),
            TrieMask::new(0b010),
            vec![branch_node_2_rlp.as_hash().unwrap()],
            None,
        );
        let branch_node_1_3_4_rlp = RlpNode::from_rlp(&alloy_rlp::encode(BranchNode::new(
            vec![
                empty_leaf_rlp_for_key(Nibbles::from_nibbles([0])),
                branch_node_2_rlp,
                empty_leaf_rlp_for_key(Nibbles::from_nibbles([0])),
            ],
            TrieMask::new(0b111),
        )));

        let branch_node_1 =
            (Nibbles::from_nibbles([vec![0; 61], vec![0]].concat()), branch_node_1_3_4.clone());
        let branch_node_3 =
            (Nibbles::from_nibbles([vec![0; 61], vec![1]].concat()), branch_node_1_3_4.clone());
        let branch_node_4 =
            (Nibbles::from_nibbles([vec![0; 61], vec![2]].concat()), branch_node_1_3_4);

        let branch_node_0 = (
            Nibbles::from_nibbles([0; 61]),
            BranchNodeCompact::new(
                TrieMask::new(0b111),
                TrieMask::new(0b111),
                TrieMask::new(0b111),
                vec![
                    branch_node_1_3_4_rlp.as_hash().unwrap(),
                    branch_node_1_3_4_rlp.as_hash().unwrap(),
                    branch_node_1_3_4_rlp.as_hash().unwrap(),
                ],
                None,
            ),
        );

        let mock_trie_nodes =
            vec![branch_node_0.clone(), branch_node_1, branch_node_3, branch_node_4.clone()];
        pretty_assertions::assert_eq!(
            hash_builder_branch_nodes.into_iter().sorted().collect::<Vec<_>>(),
            mock_trie_nodes,
        );

        let trie_cursor_factory =
            MockTrieCursorFactory::new(mock_trie_nodes.into_iter().collect(), B256Map::default());

        // Mark the account 13 as changed.
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(account_13));
        let prefix_set = prefix_set.freeze();

        let walker =
            TrieWalker::new(trie_cursor_factory.account_trie_cursor().unwrap(), prefix_set);

        let hashed_cursor_factory =
            MockHashedCursorFactory::new(accounts.into_iter().collect(), B256Map::default());

        let mut iter =
            TrieNodeIter::new(walker, hashed_cursor_factory.hashed_account_cursor().unwrap());

        // We initialize the tracing only here to not pollute the logs with hash builder above.
        reth_tracing::init_test_tracing();

        // Walk the iterator until it's exhausted.
        while iter.try_next().unwrap().is_some() {}

        pretty_assertions::assert_eq!(
            *trie_cursor_factory.visited_account_keys(),
            vec![
                KeyVisit {
                    visit_type: KeyVisitType::SeekExact(Nibbles::default()),
                    visited_key: None
                },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(Nibbles::from_nibbles([0x0])),
                    visited_key: Some(branch_node_0.0)
                },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(branch_node_4.0.clone()),
                    visited_key: Some(branch_node_4.0)
                },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(Nibbles::from_nibbles([0x1])),
                    visited_key: None
                }
            ]
        );
        pretty_assertions::assert_eq!(
            *hashed_cursor_factory.visited_account_keys(),
            vec![
                // We should not seek to account 1, we already have the hash for its branch node.
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_1),
                    visited_key: Some(account_1)
                },
                // We should not seek to account 6, we already have the hash for its branch node.
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_6),
                    visited_key: Some(account_6)
                },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_11),
                    visited_key: Some(account_11)
                },
                // We should not seek to account 11 two additional times, we're already at it.
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_11),
                    visited_key: Some(account_11)
                },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_11),
                    visited_key: Some(account_11)
                },
                KeyVisit { visit_type: KeyVisitType::Next, visited_key: Some(account_12) },
                // We should not seek to account 12, we're already at it.
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_12),
                    visited_key: Some(account_12)
                },
                KeyVisit { visit_type: KeyVisitType::Next, visited_key: Some(account_13) },
                KeyVisit { visit_type: KeyVisitType::Next, visited_key: Some(account_14) },
                KeyVisit { visit_type: KeyVisitType::Next, visited_key: Some(account_15) },
                // We should not seek to account 15, we're already at it.
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_15),
                    visited_key: Some(account_15)
                },
                // We should not advance the cursor, we know that there's no more children left,
                // because we visited the branch nodes 0 and 4.
                KeyVisit { visit_type: KeyVisitType::Next, visited_key: None },
            ],
        );
    }
}
