use crate::{
    hashed_cursor::{HashedAccountCursor, HashedStorageCursor},
    trie_cursor::TrieCursor,
    walker::TrieWalker,
    StateRootError, StorageRootError,
};
use reth_primitives::{trie::Nibbles, Account, StorageEntry, B256, U256};

#[derive(Debug)]
pub(crate) struct TrieBranchNode {
    pub(crate) key: Nibbles,
    pub(crate) value: B256,
    pub(crate) children_are_in_trie: bool,
}

impl TrieBranchNode {
    pub(crate) fn new(key: Nibbles, value: B256, children_are_in_trie: bool) -> Self {
        Self { key, value, children_are_in_trie }
    }
}

#[derive(Debug)]
pub(crate) enum AccountNode {
    Branch(TrieBranchNode),
    Leaf(B256, Account),
}

#[derive(Debug)]
pub(crate) enum StorageNode {
    Branch(TrieBranchNode),
    Leaf(B256, U256),
}

/// An iterator over existing intermediate branch nodes and updated leaf nodes.
#[derive(Debug)]
pub(crate) struct AccountNodeIter<C, H> {
    /// Underlying walker over intermediate nodes.
    pub(crate) walker: TrieWalker<C>,
    /// The cursor for the hashed account entries.
    pub(crate) hashed_account_cursor: H,
    /// The previous account key. If the iteration was previously interrupted, this value can be
    /// used to resume iterating from the last returned leaf node.
    previous_account_key: Option<B256>,

    /// Current hashed account entry.
    current_hashed_entry: Option<(B256, Account)>,
    /// Flag indicating whether we should check the current walker key.
    current_walker_key_checked: bool,
}

impl<C, H> AccountNodeIter<C, H> {
    pub(crate) fn new(walker: TrieWalker<C>, hashed_account_cursor: H) -> Self {
        Self {
            walker,
            hashed_account_cursor,
            previous_account_key: None,
            current_hashed_entry: None,
            current_walker_key_checked: false,
        }
    }

    pub(crate) fn with_last_account_key(mut self, previous_account_key: B256) -> Self {
        self.previous_account_key = Some(previous_account_key);
        self
    }
}

impl<C, H> AccountNodeIter<C, H>
where
    C: TrieCursor,
    H: HashedAccountCursor,
{
    /// Return the next account trie node to be added to the hash builder.
    ///
    /// Returns the nodes using this algorithm:
    /// 1. Return the current intermediate branch node if it hasn't been updated.
    /// 2. Advance the trie walker to the next intermediate branch node and retrieve next
    ///    unprocessed key.
    /// 3. Reposition the hashed account cursor on the next unprocessed key.
    /// 4. Return every hashed account entry up to the key of the current intermediate branch node.
    /// 5. Repeat.
    ///
    /// NOTE: The iteration will start from the key of the previous hashed entry if it was supplied.
    pub(crate) fn try_next(&mut self) -> Result<Option<AccountNode>, StateRootError> {
        loop {
            if let Some(key) = self.walker.key() {
                if !self.current_walker_key_checked && self.previous_account_key.is_none() {
                    self.current_walker_key_checked = true;
                    if self.walker.can_skip_current_node {
                        return Ok(Some(AccountNode::Branch(TrieBranchNode::new(
                            key,
                            self.walker.hash().unwrap(),
                            self.walker.children_are_in_trie(),
                        ))))
                    }
                }
            }

            if let Some((hashed_address, account)) = self.current_hashed_entry.take() {
                if self.walker.key().map_or(false, |key| key < Nibbles::unpack(hashed_address)) {
                    self.current_walker_key_checked = false;
                    continue
                }

                self.current_hashed_entry = self.hashed_account_cursor.next()?;
                return Ok(Some(AccountNode::Leaf(hashed_address, account)))
            }

            match self.previous_account_key.take() {
                Some(account_key) => {
                    self.hashed_account_cursor.seek(account_key)?;
                    self.current_hashed_entry = self.hashed_account_cursor.next()?;
                }
                None => {
                    let seek_key = match self.walker.next_unprocessed_key() {
                        Some(key) => key,
                        None => break, // no more keys
                    };
                    self.current_hashed_entry = self.hashed_account_cursor.seek(seek_key)?;
                    self.walker.advance()?;
                }
            }
        }

        Ok(None)
    }
}

#[derive(Debug)]
pub(crate) struct StorageNodeIter<C, H> {
    /// Underlying walker over intermediate nodes.
    pub(crate) walker: TrieWalker<C>,
    /// The cursor for the hashed storage entries.
    pub(crate) hashed_storage_cursor: H,
    /// The hashed address this storage trie belongs to.
    hashed_address: B256,

    /// Current hashed storage entry.
    current_hashed_entry: Option<StorageEntry>,
    /// Flag indicating whether we should check the current walker key.
    current_walker_key_checked: bool,
}

impl<C, H> StorageNodeIter<C, H> {
    pub(crate) fn new(
        walker: TrieWalker<C>,
        hashed_storage_cursor: H,
        hashed_address: B256,
    ) -> Self {
        Self {
            walker,
            hashed_storage_cursor,
            hashed_address,
            current_walker_key_checked: false,
            current_hashed_entry: None,
        }
    }
}

impl<C, H> StorageNodeIter<C, H>
where
    C: TrieCursor,
    H: HashedStorageCursor,
{
    /// Return the next storage trie node to be added to the hash builder.
    ///
    /// Returns the nodes using this algorithm:
    /// 1. Return the current intermediate branch node if it hasn't been updated.
    /// 2. Advance the trie walker to the next intermediate branch node and retrieve next
    ///    unprocessed key.
    /// 3. Reposition the hashed storage cursor on the next unprocessed key.
    /// 4. Return every hashed storage entry up to the key of the current intermediate branch node.
    /// 5. Repeat.
    pub(crate) fn try_next(&mut self) -> Result<Option<StorageNode>, StorageRootError> {
        loop {
            if let Some(key) = self.walker.key() {
                if !self.current_walker_key_checked {
                    self.current_walker_key_checked = true;
                    if self.walker.can_skip_current_node {
                        return Ok(Some(StorageNode::Branch(TrieBranchNode::new(
                            key,
                            self.walker.hash().unwrap(),
                            self.walker.children_are_in_trie(),
                        ))))
                    }
                }
            }

            if let Some(StorageEntry { key: hashed_key, value }) = self.current_hashed_entry.take()
            {
                if self.walker.key().map_or(false, |key| key < Nibbles::unpack(hashed_key)) {
                    self.current_walker_key_checked = false;
                    continue
                }

                self.current_hashed_entry = self.hashed_storage_cursor.next()?;
                return Ok(Some(StorageNode::Leaf(hashed_key, value)))
            }

            let Some(seek_key) = self.walker.next_unprocessed_key() else { break };
            self.current_hashed_entry =
                self.hashed_storage_cursor.seek(self.hashed_address, seek_key)?;
            self.walker.advance()?;
        }

        Ok(None)
    }
}
