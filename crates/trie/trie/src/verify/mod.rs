//! Sequential comparison of persisted trie nodes against canonical updates.

use crate::{
    trie_cursor::{depth_first, depth_first::DepthFirstTrieIterator, TrieCursor},
    BranchNodeCompact, Nibbles,
};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;
use std::cmp::Ordering;
use tracing::trace;

/// Compares one trie against updates computed from its hashed state.
///
/// This type does no hashing or scheduling and works with any [`TrieCursor`]. Feed successive
/// canonical update batches to [`Self::verify_updates`], then call [`Self::finish`]. Findings are
/// delivered immediately, so even an entirely stale trie does not need to be buffered in memory.
#[derive(Debug)]
pub struct Verifier<C: TrieCursor> {
    trie: TrieKind,
    trie_iter: DepthFirstTrieIterator<C>,
    current: Option<(Nibbles, BranchNodeCompact)>,
}

impl<C: TrieCursor> Verifier<C> {
    /// Opens the comparison cursor and reads its first node.
    pub fn new(trie: TrieKind, trie_cursor: C) -> Result<Self, DatabaseError> {
        let mut trie_iter = DepthFirstTrieIterator::new(trie_cursor);
        let current = trie_iter.next().transpose()?;
        Ok(Self { trie, trie_iter, current })
    }

    /// Compares a batch of canonical updates, sorting them in depth-first order internally.
    ///
    /// Successive batches must continue the same canonical root calculation. Pass the account or
    /// storage nodes from its progress updates, which exclude non-persisted empty root paths.
    /// The callback is also called with account progress after each compared canonical node,
    /// following any inconsistencies found while reaching it. Storage tries emit no progress.
    /// A callback error stops comparison immediately; do not reuse the verifier after an error.
    pub fn verify_updates<E: From<DatabaseError>>(
        &mut self,
        updates: impl IntoIterator<Item = (Nibbles, BranchNodeCompact)>,
        output: &mut impl FnMut(Output) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut nodes = updates.into_iter().collect::<Vec<_>>();
        // HashBuilder completes descendants before their parents, including across batches.
        nodes.sort_unstable_by(|a, b| depth_first::cmp(&a.0, &b.0));
        for (path, node) in nodes {
            self.verify_node(path, node, output)?;
            if self.trie == TrieKind::Account {
                output(Output::Progress(path))?;
            }
        }
        Ok(())
    }

    /// Reports all remaining persisted nodes as extra. Call once after the final update batch.
    /// A callback error stops comparison immediately.
    pub fn finish<E: From<DatabaseError>>(
        &mut self,
        output: &mut impl FnMut(Output) -> Result<(), E>,
    ) -> Result<(), E> {
        while let Some((path, found)) = self.current.take() {
            output(Output::Inconsistency {
                trie: self.trie,
                path,
                error: NodeDifference::Extra { found },
            })?;
            self.current = self.trie_iter.next().transpose()?;
        }
        Ok(())
    }

    fn verify_node<E: From<DatabaseError>>(
        &mut self,
        path: Nibbles,
        expected: BranchNodeCompact,
        output: &mut impl FnMut(Output) -> Result<(), E>,
    ) -> Result<(), E> {
        loop {
            // Once the persisted cursor is exhausted, every further canonical node is missing.
            let Some((current_path, found)) = &self.current else {
                return output(Output::Inconsistency {
                    trie: self.trie,
                    path,
                    error: NodeDifference::Missing { expected },
                });
            };
            trace!(target: "trie::verify", trie = ?self.trie, ?current_path, ?path, "Current cursor node");

            match depth_first::cmp(&path, current_path) {
                Ordering::Less => {
                    // A canonical path before the cursor's path was not produced by the cursor.
                    return output(Output::Inconsistency {
                        trie: self.trie,
                        path,
                        error: NodeDifference::Missing { expected },
                    });
                }
                Ordering::Equal => {
                    // Matching paths consume the persisted node even when its value is wrong.
                    if *found != expected {
                        output(Output::Inconsistency {
                            trie: self.trie,
                            path,
                            error: NodeDifference::Wrong { expected, found: found.clone() },
                        })?;
                    }
                    self.current = self.trie_iter.next().transpose()?;
                    return Ok(());
                }
                Ordering::Greater => {
                    // A persisted path before the canonical path is extra. Advance and compare
                    // the same canonical node against the next persisted node.
                    output(Output::Inconsistency {
                        trie: self.trie,
                        path: *current_path,
                        error: NodeDifference::Extra { found: found.clone() },
                    })?;
                    self.current = self.trie_iter.next().transpose()?;
                }
            }
        }
    }
}

/// Identifies the trie containing an inconsistent node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrieKind {
    /// The account trie.
    Account,
    /// The storage trie for a hashed account address.
    Storage {
        /// Hashed address identifying the storage trie.
        hashed_address: B256,
    },
}

/// Difference between a persisted node and its canonical value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeDifference {
    /// A node exists only in the persisted trie.
    Extra {
        /// Persisted node to remove.
        found: BranchNodeCompact,
    },
    /// A node exists only in the canonical trie.
    Missing {
        /// Canonical node to insert.
        expected: BranchNodeCompact,
    },
    /// A persisted node differs from its canonical value.
    Wrong {
        /// Canonical replacement.
        expected: BranchNodeCompact,
        /// Persisted node.
        found: BranchNodeCompact,
    },
}

/// A comparison finding or account-trie progress indicator. Hashed state is the source of truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// An inconsistent persisted node.
    Inconsistency {
        /// Trie containing the node.
        trie: TrieKind,
        /// Node path within the trie.
        path: Nibbles,
        /// Difference from the canonical trie.
        error: NodeDifference,
    },
    /// Path of the last compared canonical account node, not an inconsistency.
    Progress(Nibbles),
}
