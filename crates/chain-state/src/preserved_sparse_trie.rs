//! Preserved sparse trie for reuse across payload validations.

use alloy_primitives::{map::B256Map, B256};
use reth_trie_sparse::SparseStateTrie;
use std::{fmt, sync::mpsc::Receiver};
use tracing::debug;

/// A preserved sparse trie that can be reused across payload validations.
pub struct PreservedSparseTrie {
    /// The finalized sparse trie and its revealed storage roots, which may still be pending.
    trie: Receiver<SparseTrieWithStorageRoots>,
    /// Hash of the block whose post-state this trie represents.
    ///
    /// Used to verify continuity: a new payload's parent hash must match this before the existing
    /// sparse trie nodes can be reused.
    block_hash: B256,
    /// Parent block hash of the earliest overlay state covered by this trie.
    anchor_hash: B256,
}

impl fmt::Debug for PreservedSparseTrie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreservedSparseTrie")
            .field("block_hash", &self.block_hash)
            .field("anchor_hash", &self.anchor_hash)
            .finish_non_exhaustive()
    }
}

impl PreservedSparseTrie {
    /// Associates a trie still being finalized with its completed block and pruning anchor.
    pub const fn pending(
        trie: Receiver<SparseTrieWithStorageRoots>,
        block_hash: B256,
        anchor_hash: B256,
    ) -> Self {
        Self { trie, block_hash, anchor_hash }
    }

    /// Returns the hash of the block whose post-state this trie represents.
    pub const fn block_hash(&self) -> B256 {
        self.block_hash
    }

    /// Returns the parent block hash of the earliest overlay state covered by this trie.
    pub const fn anchor_hash(&self) -> B256 {
        self.anchor_hash
    }

    /// Consumes self and returns the sparse trie and its revealed storage roots if reusable for
    /// the parent block.
    ///
    /// If the parent block hash does not match the preserved trie's block hash, this drops the
    /// trie and returns `None` so the caller can create a fresh sparse trie.
    pub fn into_trie_for(
        self,
        parent_hash: B256,
    ) -> Result<Option<SparseTrieWithStorageRoots>, PreservedSparseTrieError> {
        if self.block_hash == parent_hash {
            let trie = self.trie.recv().map_err(|_| PreservedSparseTrieError::ProducerDropped {
                block_hash: self.block_hash,
            })?;
            debug!(
                target: "engine::tree::payload_processor",
                block_hash = %self.block_hash,
                anchor_hash = %self.anchor_hash,
                "Reusing anchored sparse trie for continuation payload"
            );
            Ok(Some(trie))
        } else {
            debug!(
                target: "engine::tree::payload_processor",
                block_hash = %self.block_hash,
                anchor_hash = %self.anchor_hash,
                %parent_hash,
                "Dropping anchored sparse trie - parent hash mismatch"
            );
            Ok(None)
        }
    }
}

/// Error returned when consuming a preserved sparse trie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreservedSparseTrieError {
    /// The producer of a pending preserved sparse trie dropped before publishing it.
    ProducerDropped {
        /// The block hash the pending trie was expected to represent.
        block_hash: B256,
    },
}

impl fmt::Display for PreservedSparseTrieError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProducerDropped { block_hash } => {
                write!(f, "pending preserved sparse trie producer dropped for {block_hash}")
            }
        }
    }
}

impl std::error::Error for PreservedSparseTrieError {}

/// A finalized sparse trie and its revealed storage roots for the next block's proof workers.
#[derive(Debug, Default)]
pub struct SparseTrieWithStorageRoots {
    /// The sparse trie after finalization and pruning.
    pub trie: SparseStateTrie,
    /// Roots of all revealed storage tries before pruning, keyed by hashed account address.
    pub storage_roots: B256Map<B256>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn pending_trie_exposes_block_hash_before_completion() {
        let block_hash = B256::with_last_byte(1);
        let anchor_hash = B256::with_last_byte(2);
        let (tx, rx) = mpsc::channel();
        let preserved = PreservedSparseTrie::pending(rx, block_hash, anchor_hash);

        assert_eq!(preserved.block_hash(), block_hash);
        assert_eq!(preserved.anchor_hash(), anchor_hash);
        tx.send(SparseTrieWithStorageRoots::default()).unwrap();
        assert!(preserved.into_trie_for(block_hash).unwrap().is_some());
    }

    #[test]
    fn pending_trie_with_mismatched_parent_does_not_wait() {
        let block_hash = B256::with_last_byte(1);
        let other_block_hash = B256::with_last_byte(2);
        let anchor_hash = B256::with_last_byte(3);
        let (tx, rx) = mpsc::channel();
        let preserved = PreservedSparseTrie::pending(rx, block_hash, anchor_hash);

        assert!(preserved.into_trie_for(other_block_hash).unwrap().is_none());
        assert!(tx.send(SparseTrieWithStorageRoots::default()).is_err());
    }
}
