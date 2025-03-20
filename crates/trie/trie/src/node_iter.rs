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
                    // Get the seek key and set the current hashed entry based on walker's next
                    // unprocessed key
                    let seek_key = match self.walker.next_unprocessed_key() {
                        Some(key) => key,
                        None => break, // no more keys
                    };
                    trace!(target: "trie::node_iter", ?seek_key, "seeking to the next unprocessed hashed entry");
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
    use std::collections::BTreeMap;

    use alloy_primitives::{b256, hex, map::B256Map};
    use alloy_trie::{BranchNodeCompact, Nibbles, TrieAccount, TrieMask};
    use reth_primitives_traits::Account;
    use reth_trie_common::{prefix_set::PrefixSetMut, BranchNode, RlpNode};

    use crate::{
        hashed_cursor::{mock::MockHashedCursorFactory, HashedCursorFactory},
        mock::{KeyVisit, KeyVisitType},
        trie_cursor::{mock::MockTrieCursorFactory, TrieCursorFactory},
        walker::TrieWalker,
    };

    use super::TrieNodeIter;

    #[test]
    fn test_trie_node_iter() {
        reth_tracing::init_test_tracing();

        // Extension (Key = 000000000000000000000000000000000000000000000000000000000000)
        // └── Branch
        //     ├── 0 -> Branch
        //     │      ├── 0 -> Leaf (Empty Account, marked as changed)
        //     │      └── 1 -> Leaf (Empty Account 2)
        //     ├── 1 -> Branch
        //     │      ├── 0 -> Leaf (Empty Account 3)
        //     │      └── 1 -> Leaf (Empty Account 4)

        let account_1 = b256!("0x0000000000000000000000000000000000000000000000000000000000000000");
        let account_2 = b256!("0x0000000000000000000000000000000000000000000000000000000000000001");
        let account_3 = b256!("0x0000000000000000000000000000000000000000000000000000000000000100");
        let account_4 = b256!("0x0000000000000000000000000000000000000000000000000000000000000101");
        let empty_account = Account::default();

        let empty_account_rlp = RlpNode::from_rlp(&alloy_rlp::encode(TrieAccount::default()));

        let child_branch_node_rlp = RlpNode::from_rlp(&alloy_rlp::encode(BranchNode::new(
            vec![empty_account_rlp.clone(), empty_account_rlp],
            TrieMask::new(0b11),
        )));
        let child_branch_node_1 = (
            Nibbles::unpack(hex!(
                "0x00000000000000000000000000000000000000000000000000000000000000"
            )),
            BranchNodeCompact::new(
                TrieMask::new(0b11),
                TrieMask::new(0b00),
                TrieMask::new(0b00),
                vec![],
                None,
            ),
        );
        let child_branch_node_2 = (
            Nibbles::unpack(hex!(
                "0x00000000000000000000000000000000000000000000000000000000000001"
            )),
            BranchNodeCompact::new(
                TrieMask::new(0b11),
                TrieMask::new(0b00),
                TrieMask::new(0b00),
                vec![],
                None,
            ),
        );

        let branch_node_rlp = RlpNode::from_rlp(&alloy_rlp::encode(BranchNode::new(
            vec![child_branch_node_rlp.clone(), child_branch_node_rlp.clone()],
            TrieMask::new(0b11),
        )));
        let branch_node = (
            Nibbles::unpack(hex!("0x000000000000000000000000000000000000000000000000000000000000")),
            BranchNodeCompact::new(
                TrieMask::new(0b11),
                // Tree mask has no bits set, because both child branch nodes have empty tree and
                // hash masks.
                TrieMask::new(0b00),
                // Hash mask bits are set, because both child nodes are branches.
                TrieMask::new(0b11),
                vec![
                    child_branch_node_rlp.as_hash().unwrap(),
                    child_branch_node_rlp.as_hash().unwrap(),
                ],
                Some(branch_node_rlp.as_hash().unwrap()),
            ),
        );

        let trie_cursor_factory = MockTrieCursorFactory::new(
            BTreeMap::from([branch_node.clone(), child_branch_node_1, child_branch_node_2]),
            B256Map::default(),
        );

        // Mark the account 1 as changed.
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(account_1));
        let prefix_set = prefix_set.freeze();

        let walker =
            TrieWalker::new(trie_cursor_factory.account_trie_cursor().unwrap(), prefix_set);

        let hashed_cursor_factory = MockHashedCursorFactory::new(
            BTreeMap::from([
                (account_1, empty_account),
                (account_2, empty_account),
                (account_3, empty_account),
                (account_4, empty_account),
            ]),
            B256Map::default(),
        );

        let mut iter =
            TrieNodeIter::new(walker, hashed_cursor_factory.hashed_account_cursor().unwrap());

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
                    visited_key: Some(branch_node.0)
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
                // Why do we seek account 1 two additional times?
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_1),
                    visited_key: Some(account_1)
                },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_1),
                    visited_key: Some(account_1)
                },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(account_1),
                    visited_key: Some(account_1)
                },
                KeyVisit { visit_type: KeyVisitType::Next, visited_key: Some(account_2) },
                KeyVisit { visit_type: KeyVisitType::Next, visited_key: Some(account_3) },
                // Why do we go to account 4 if we can just take the branch node hash?
                KeyVisit { visit_type: KeyVisitType::Next, visited_key: Some(account_4) },
                KeyVisit { visit_type: KeyVisitType::Next, visited_key: None },
                KeyVisit {
                    visit_type: KeyVisitType::SeekNonExact(b256!(
                        "0x0000000000000000000000000000000000000000000000000000000000002000"
                    )),
                    visited_key: None
                },
            ],
        );
    }
}
