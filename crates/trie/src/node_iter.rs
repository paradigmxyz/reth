use crate::{
    hashed_cursor::{HashedAccountCursor, HashedStorageCursor},
    trie_cursor::TrieCursor,
    walker::TrieWalker,
    StateRootError, StorageRootError,
};
use reth_primitives::{trie::Nibbles, Account, StorageEntry, B256, U256};

/// Represents a branch node in the trie.
#[derive(Debug)]
pub(crate) struct TrieBranchNode {
    /// The key associated with the node.
    pub(crate) key: Nibbles,
    /// The value associated with the node.
    pub(crate) value: B256,
    /// Indicates whether children are in the trie.
    pub(crate) children_are_in_trie: bool,
}

impl TrieBranchNode {
    /// Creates a new `TrieBranchNode`.
    pub(crate) fn new(key: Nibbles, value: B256, children_are_in_trie: bool) -> Self {
        Self { key, value, children_are_in_trie }
    }
}

/// Represents a variant of an account node.
#[derive(Debug)]
pub(crate) enum AccountNode {
    /// Branch node.
    Branch(TrieBranchNode),
    /// Leaf node.
    Leaf(B256, Account),
}

/// Represents a variant of a storage node.
#[derive(Debug)]
pub(crate) enum StorageNode {
    /// Branch node.
    Branch(TrieBranchNode),
    /// Leaf node.
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
    /// Creates a new `AccountNodeIter`.
    pub(crate) fn new(walker: TrieWalker<C>, hashed_account_cursor: H) -> Self {
        Self {
            walker,
            hashed_account_cursor,
            previous_account_key: None,
            current_hashed_entry: None,
            current_walker_key_checked: false,
        }
    }

    /// Sets the last iterated account key and returns the modified `AccountNodeIter`.
    /// This is used to resume iteration from the last checkpoint.
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
            // If the walker has a key...
            if let Some(key) = self.walker.key() {
                // Check if the current walker key is unchecked and there's no previous account key
                if !self.current_walker_key_checked && self.previous_account_key.is_none() {
                    self.current_walker_key_checked = true;
                    // If it's possible to skip the current node in the walker, return a branch node
                    if self.walker.can_skip_current_node {
                        return Ok(Some(AccountNode::Branch(TrieBranchNode::new(
                            key.clone(),
                            self.walker.hash().unwrap(),
                            self.walker.children_are_in_trie(),
                        ))))
                    }
                }
            }

            // If there's a hashed address and account...
            if let Some((hashed_address, account)) = self.current_hashed_entry.take() {
                // If the walker's key is less than the unpacked hashed address, reset the checked
                // status and continue
                if self.walker.key().map_or(false, |key| key < &Nibbles::unpack(hashed_address)) {
                    self.current_walker_key_checked = false;
                    continue
                }

                // Set the next hashed entry as a leaf node and return
                self.current_hashed_entry = self.hashed_account_cursor.next()?;
                return Ok(Some(AccountNode::Leaf(hashed_address, account)))
            }

            // Handle seeking and advancing based on the previous account key
            match self.previous_account_key.take() {
                Some(account_key) => {
                    // Seek to the previous account key and get the next hashed entry
                    self.hashed_account_cursor.seek(account_key)?;
                    self.current_hashed_entry = self.hashed_account_cursor.next()?;
                }
                None => {
                    // Get the seek key and set the current hashed entry based on walker's next
                    // unprocessed key
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
    /// Creates a new instance of StorageNodeIter.
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
            // Check if there's a key in the walker.
            if let Some(key) = self.walker.key() {
                // Check if the walker key hasn't been checked yet.
                if !self.current_walker_key_checked {
                    self.current_walker_key_checked = true;
                    // Check if the current node can be skipped in the walker.
                    if self.walker.can_skip_current_node {
                        // Return a branch node based on the walker's properties.
                        return Ok(Some(StorageNode::Branch(TrieBranchNode::new(
                            key.clone(),
                            self.walker.hash().unwrap(),
                            self.walker.children_are_in_trie(),
                        ))))
                    }
                }
            }

            // Check for a current hashed storage entry.
            if let Some(StorageEntry { key: hashed_key, value }) = self.current_hashed_entry.take()
            {
                // Compare keys and proceed accordingly.
                if self.walker.key().map_or(false, |key| key < &Nibbles::unpack(hashed_key)) {
                    self.current_walker_key_checked = false;
                    continue
                }

                // Move to the next hashed storage entry and return the corresponding leaf node.
                self.current_hashed_entry = self.hashed_storage_cursor.next()?;
                return Ok(Some(StorageNode::Leaf(hashed_key, value)))
            }

            // Attempt to get the next unprocessed key from the walker.
            if let Some(seek_key) = self.walker.next_unprocessed_key() {
                // Seek and update the current hashed entry based on the new seek key.
                self.current_hashed_entry =
                    self.hashed_storage_cursor.seek(self.hashed_address, seek_key)?;
                self.walker.advance()?;
            } else {
                // No more keys to process, break the loop.
                break
            }
        }

        Ok(None) // Return None if no more nodes are available.
    }
}
