use crate::{hashed_cursor::HashedCursor, trie_cursor::TrieCursor, walker::TrieWalker, Nibbles};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;

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
                return Ok(Some(TrieElement::Leaf(hashed_key, value)))
            }

            // Handle seeking and advancing based on the previous hashed key
            match self.previous_hashed_key.take() {
                Some(hashed_key) => {
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
    use alloy_trie::{BranchNodeCompact, Nibbles, TrieMask};
    use reth_primitives_traits::Account;
    use reth_trie_common::prefix_set::PrefixSetMut;

    use crate::{
        hashed_cursor::{mock::MockHashedCursorFactory, HashedCursorFactory},
        mock::KeyVisitType,
        trie_cursor::{mock::MockTrieCursorFactory, TrieCursorFactory},
        walker::TrieWalker,
    };

    use super::TrieNodeIter;

    #[test]
    fn test_trie_node_iter() {
        reth_tracing::init_test_tracing();

        let account_1 = b256!("0x0000000000000000000000000000000000000000000000000000000000000000");
        let account_2 = b256!("0x0000000000000000000000000000000000000000000000000000000000000001");
        let account_3 = b256!("0x0000000000000000000000000000000000000000000000000000000000000100");
        let hashed_accounts = BTreeMap::from([
            (account_1, Account::default()),
            (account_2, Account::default()),
            (account_3, Account::default()),
        ]);

        let root_branch_node = (
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
        let child_branch_node = (
            Nibbles::unpack(hex!("0x000000000000000000000000000000000000000000000000000000000000")),
            BranchNodeCompact::new(
                TrieMask::new(0b11),
                TrieMask::new(0b01),
                TrieMask::new(0b01),
                vec![b256!("0x0000000000000000000000000000000000000000000000000000000000000000")],
                None,
            ),
        );

        let trie_nodes = BTreeMap::from([root_branch_node.clone(), child_branch_node.clone()]);

        let trie_cursor_factory = MockTrieCursorFactory::new(trie_nodes, B256Map::default());

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::unpack(account_1));

        let walker = TrieWalker::new(
            trie_cursor_factory.account_trie_cursor().unwrap(),
            prefix_set.freeze(),
        );
        println!("{walker:?}");

        let hashed_cursor_factory =
            MockHashedCursorFactory::new(hashed_accounts, B256Map::default());

        let mut iter =
            TrieNodeIter::new(walker, hashed_cursor_factory.hashed_account_cursor().unwrap());

        while iter.try_next().unwrap().is_some() {}

        assert_eq!(
            *trie_cursor_factory.visited_account_keys(),
            vec![
                KeyVisitType::SeekNonExact(child_branch_node.0),
                KeyVisitType::SeekNonExact(root_branch_node.0),
            ]
        );
        assert_eq!(
            *hashed_cursor_factory.visited_account_keys(),
            vec![
                KeyVisitType::SeekExact(account_1),
                KeyVisitType::SeekExact(account_1),
                KeyVisitType::SeekExact(account_1),
                KeyVisitType::Next(account_2),
                KeyVisitType::Next(account_3),
            ]
        );
    }
}
