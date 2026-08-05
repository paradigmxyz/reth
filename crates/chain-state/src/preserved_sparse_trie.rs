//! Preserved sparse trie for reuse across payload validations.

use alloy_primitives::B256;
use reth_trie_sparse::SparseStateTrie;
use std::{
    fmt,
    sync::mpsc::{self, Receiver, Sender},
};
use tracing::debug;

/// Type alias for the sparse trie type used in preservation.
pub type SparseTrie = SparseStateTrie;

/// A preserved sparse trie that can be reused across payload validations.
pub struct PreservedSparseTrie {
    /// The preserved sparse state trie, or a handle to wait for it.
    trie: PreservedSparseTrieInner,
    /// The state root this trie represents.
    ///
    /// Used to verify continuity: a new payload's `parent_state_root` must match this before the
    /// existing sparse trie nodes can be reused.
    state_root: B256,
    /// Parent block hash of the earliest overlay state covered by this trie.
    anchor_hash: B256,
}

impl fmt::Debug for PreservedSparseTrie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreservedSparseTrie")
            .field("state_root", &self.state_root)
            .field("anchor_hash", &self.anchor_hash)
            .finish_non_exhaustive()
    }
}

impl PreservedSparseTrie {
    /// Creates a new anchored preserved trie.
    ///
    /// The `state_root` is the computed state root from the trie. The `anchor_hash` is the parent
    /// block hash of the earliest overlay state covered by the trie.
    pub const fn anchored(trie: SparseTrie, state_root: B256, anchor_hash: B256) -> Self {
        Self { trie: PreservedSparseTrieInner::Ready(trie), state_root, anchor_hash }
    }

    /// Creates a pending preserved trie and a completer that will publish the trie later.
    pub fn pending(state_root: B256, anchor_hash: B256) -> (Self, PreservedSparseTrieCompleter) {
        let (tx, rx) = mpsc::channel();
        (
            Self { trie: PreservedSparseTrieInner::Pending(rx), state_root, anchor_hash },
            PreservedSparseTrieCompleter { tx },
        )
    }

    /// Returns the state root this trie is anchored to.
    pub const fn state_root(&self) -> B256 {
        self.state_root
    }

    /// Returns the parent block hash of the earliest overlay state covered by this trie.
    pub const fn anchor_hash(&self) -> B256 {
        self.anchor_hash
    }

    /// Consumes self and returns the trie if it can be reused for the parent state root.
    ///
    /// If the parent state root does not match the preserved trie's state root, this drops the trie
    /// and returns `None` so the caller can create a fresh sparse trie.
    pub fn into_trie_for(
        self,
        parent_state_root: B256,
    ) -> Result<Option<SparseTrie>, PreservedSparseTrieError> {
        if self.state_root == parent_state_root {
            let trie = match self.trie {
                PreservedSparseTrieInner::Ready(trie) => trie,
                PreservedSparseTrieInner::Pending(rx) => match rx.recv() {
                    Ok(trie) => trie,
                    Err(_) => {
                        return Err(PreservedSparseTrieError::ProducerDropped {
                            state_root: self.state_root,
                        })
                    }
                },
            };
            debug!(
                target: "engine::tree::payload_processor",
                state_root = %self.state_root,
                anchor_hash = %self.anchor_hash,
                "Reusing anchored sparse trie for continuation payload"
            );
            Ok(Some(trie))
        } else {
            debug!(
                target: "engine::tree::payload_processor",
                anchor_root = %self.state_root,
                anchor_hash = %self.anchor_hash,
                %parent_state_root,
                "Dropping anchored sparse trie - parent state root mismatch"
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
        /// The state root the pending trie was expected to represent.
        state_root: B256,
    },
}

impl fmt::Display for PreservedSparseTrieError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProducerDropped { state_root } => {
                write!(f, "pending preserved sparse trie producer dropped for {state_root}")
            }
        }
    }
}

impl std::error::Error for PreservedSparseTrieError {}

#[allow(clippy::large_enum_variant)]
enum PreservedSparseTrieInner {
    Ready(SparseTrie),
    Pending(Receiver<SparseTrie>),
}

/// Completes a pending preserved sparse trie.
#[derive(Debug)]
pub struct PreservedSparseTrieCompleter {
    tx: Sender<SparseTrie>,
}

impl PreservedSparseTrieCompleter {
    /// Publishes the trie for a pending preserved sparse trie.
    pub fn complete(self, trie: SparseTrie) -> Result<(), SparseTrie> {
        self.tx.send(trie).map_err(|err| err.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_trie_exposes_state_root_before_completion() {
        let state_root = B256::with_last_byte(1);
        let anchor_hash = B256::with_last_byte(2);
        let (preserved, completer) = PreservedSparseTrie::pending(state_root, anchor_hash);

        assert_eq!(preserved.state_root(), state_root);
        assert_eq!(preserved.anchor_hash(), anchor_hash);
        completer.complete(SparseTrie::default()).unwrap();
        assert!(preserved.into_trie_for(state_root).unwrap().is_some());
    }

    #[test]
    fn pending_trie_with_mismatched_root_does_not_wait() {
        let state_root = B256::with_last_byte(1);
        let other_state_root = B256::with_last_byte(2);
        let anchor_hash = B256::with_last_byte(3);
        let (preserved, completer) = PreservedSparseTrie::pending(state_root, anchor_hash);

        assert!(preserved.into_trie_for(other_state_root).unwrap().is_none());
        assert!(completer.complete(SparseTrie::default()).is_err());
    }
}
