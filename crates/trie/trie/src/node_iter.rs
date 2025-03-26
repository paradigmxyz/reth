use crate::{
    hashed_cursor::HashedCursor, metrics::TrieType, trie_cursor::TrieCursor, walker::TrieWalker,
    Nibbles,
};
use alloy_primitives::B256;
use metrics::Counter;
use reth_metrics::Metrics;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::RlpNode;
use tracing::trace;

/// Represents a branch node in the trie.
#[derive(Debug)]
pub struct TrieBranchNode {
    /// The key associated with the node.
    pub key: Nibbles,
    /// The value associated with the node.
    pub value: RlpNode,
    /// Indicates whether children are in the trie.
    pub children_are_in_trie: bool,
}

impl TrieBranchNode {
    /// Creates a new `TrieBranchNode`.
    pub const fn new(key: Nibbles, value: RlpNode, children_are_in_trie: bool) -> Self {
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

    /// The hashed cursor key to seek to.
    hashed_cursor_seek: Option<B256>,
    /// Flag indicating whether we should advance the hashed cursor by one after seeking.
    hashed_cursor_next: bool,

    /// Current hashed  entry.
    current_hashed_entry: Option<(B256, <H as HashedCursor>::Value)>,
    /// Flag indicating whether we should check the current walker key.
    current_walker_key_checked: bool,

    #[cfg(feature = "metrics")]
    metrics: TrieNodeIterMetrics,
}

impl<C, H: HashedCursor> TrieNodeIter<C, H> {
    /// Creates a new [`TrieNodeIter`].
    pub fn new(walker: TrieWalker<C>, hashed_cursor: H, trie_type: TrieType) -> Self {
        Self {
            walker,
            hashed_cursor,
            previous_hashed_key: None,
            hashed_cursor_seek: None,
            hashed_cursor_next: false,
            current_hashed_entry: None,
            current_walker_key_checked: false,
            #[cfg(feature = "metrics")]
            metrics: TrieNodeIterMetrics::new_with_labels(&[("type", trie_type.as_str())]),
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
                        let hash = self.walker.hash().unwrap();
                        let children_are_in_trie = self.walker.children_are_in_trie();
                        trace!(
                            target: "trie::node_iter",
                            ?key,
                            ?hash,
                            children_are_in_trie,
                            "Skipping current node in walker and returning branch node"
                        );
                        return Ok(Some(TrieElement::Branch(TrieBranchNode::new(
                            key.clone(),
                            hash,
                            children_are_in_trie,
                        ))))
                    }
                }
            }

            // Seek the hashed cursor if we have a key.
            if let Some(hashed_key) = self.hashed_cursor_seek.take() {
                trace!(target: "trie::node_iter", ?hashed_key, "Seeking to hashed key");
                let entry = self.hashed_cursor.seek(hashed_key)?;
                if self.hashed_cursor_next {
                    self.current_hashed_entry = self.hashed_cursor.next()?;
                    self.hashed_cursor_next = false;
                } else {
                    self.current_hashed_entry = entry;
                }

                #[cfg(feature = "metrics")]
                self.metrics.hashed_cursor_seeks_total.increment(1);
            }

            // If there's a hashed entry...
            if let Some((hashed_key, value)) = self.current_hashed_entry.take() {
                // If the walker's key is less than the unpacked hashed key,
                // reset the checked status and continue
                if self.walker.key().is_some_and(|key| key < &Nibbles::unpack(hashed_key)) {
                    self.current_walker_key_checked = false;
                    continue
                }

                trace!(target: "trie::node_iter", ?hashed_key, "Returning leaf node");

                // Set the next hashed entry as a leaf node and return
                trace!(target: "trie::node_iter", ?hashed_key, "next hashed entry");
                self.current_hashed_entry = self.hashed_cursor.next()?;
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
                    // Get the seek key and lazily set the current hashed entry based on walker's
                    // next unprocessed key
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

#[derive(Metrics)]
#[metrics(scope = "trie.node_iter")]
struct TrieNodeIterMetrics {
    /// The number of times the hashed cursor was seeked.
    pub hashed_cursor_seeks_total: Counter,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

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
        metrics::TrieType,
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
            TrieType::State,
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

        reth_tracing::init_test_tracing();

        // Extension (Key = 0x0000000000000000000000000000000000000000000000000000000000000)
        // └── Branch (`branch_node_0`)
        //     ├── 0 -> Branch (`branch_node_1`)
        //     │      ├── 0 -> Leaf (`account_1`, Key = 0x0)
        //     │      └── 1 -> Leaf (`account_2`, Key = 0x0)
        //     ├── 1 -> Branch (`branch_node_2`)
        //     │      ├── 0 -> Branch (`branch_node_3`)
        //     │      │      ├── 0 -> Leaf (`account_3`, marked as changed)
        //     │      │      └── 1 -> Leaf (`account_4`)
        //     │      └── 1 -> Leaf (`account_5`, Key = 0x0)

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

        let mock_trie_nodes = vec![
            branch_node_0.clone(),
            branch_node_1,
            branch_node_2.clone(),
            branch_node_3.clone(),
        ];
        pretty_assertions::assert_eq!(
            hash_builder_branch_nodes.into_iter().sorted().collect::<Vec<_>>(),
            mock_trie_nodes,
        );

        let trie_cursor_factory =
            MockTrieCursorFactory::new(mock_trie_nodes.into_iter().collect(), B256Map::default());

        // Mark the account 3 as changed.
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(account_3));
        let prefix_set = prefix_set.freeze();

        let walker =
            TrieWalker::new(trie_cursor_factory.account_trie_cursor().unwrap(), prefix_set);

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

        let mut iter = TrieNodeIter::new(
            walker,
            hashed_cursor_factory.hashed_account_cursor().unwrap(),
            TrieType::State,
        );

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
                    visit_type: KeyVisitType::SeekNonExact(branch_node_2.0.clone()),
                    visited_key: Some(branch_node_2.0)
                },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(branch_node_3.0.clone()),
                    visited_key: Some(branch_node_3.0)
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
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_1),
                    visited_key: Some(account_1)
                },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_3),
                    visited_key: Some(account_3)
                },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_3),
                    visited_key: Some(account_3)
                },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_3),
                    visited_key: Some(account_3)
                },
                KeyVisit { visit_type: KeyVisitType::Next, visited_key: Some(account_4) },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_3),
                    visited_key: Some(account_3)
                },
                KeyVisit { visit_type: KeyVisitType::Next, visited_key: Some(account_4) },
                KeyVisit { visit_type: KeyVisitType::Next, visited_key: Some(account_5) },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(b256!(
                        "0x0000000000000000000000000000000000000000000000000000000000000102"
                    )),
                    visited_key: Some(account_5)
                },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(b256!(
                        "0x0000000000000000000000000000000000000000000000000000000000000120"
                    )),
                    visited_key: None
                },
            ],
        );
    }
}
