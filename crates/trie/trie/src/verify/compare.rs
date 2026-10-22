//! Depth-first comparison of persisted trie nodes against recomputed nodes.

use super::Output;
use crate::{
    trie_cursor::{
        depth_first::{self, DepthFirstTrieIterator},
        TrieCursor,
    },
    Nibbles,
};
use alloy_primitives::B256;
use alloy_trie::BranchNodeCompact;
use reth_storage_errors::db::DatabaseError;
use std::cmp::Ordering;
use tracing::trace;

/// Verifies the contents of a trie table against some other data source which is able to produce
/// stored trie nodes.
#[derive(Debug)]
pub(super) struct SingleVerifier<I> {
    account: Option<B256>, // None for accounts trie
    trie_iter: I,
    pub(super) curr: Option<(Nibbles, BranchNodeCompact)>,
}

impl<C: TrieCursor> SingleVerifier<DepthFirstTrieIterator<C>> {
    pub(super) fn new(account: Option<B256>, trie_cursor: C) -> Result<Self, DatabaseError> {
        let mut trie_iter = DepthFirstTrieIterator::new(trie_cursor);
        let curr = trie_iter.next().transpose()?;
        Ok(Self { account, trie_iter, curr })
    }

    const fn output_extra(&self, path: Nibbles, node: BranchNodeCompact) -> Output {
        if let Some(account) = self.account {
            Output::StorageExtra(account, path, node)
        } else {
            Output::AccountExtra(path, node)
        }
    }

    const fn output_wrong(
        &self,
        path: Nibbles,
        expected: BranchNodeCompact,
        found: BranchNodeCompact,
    ) -> Output {
        if let Some(account) = self.account {
            Output::StorageWrong { account, path, expected, found }
        } else {
            Output::AccountWrong { path, expected, found }
        }
    }

    const fn output_missing(&self, path: Nibbles, node: BranchNodeCompact) -> Output {
        if let Some(account) = self.account {
            Output::StorageMissing(account, path, node)
        } else {
            Output::AccountMissing(path, node)
        }
    }

    /// Called with the next path and node in the canonical sequence of stored trie nodes. Will
    /// append to the given `outputs` Vec if walking the trie cursor produces data
    /// inconsistent with that given.
    ///
    /// `next` must be called with paths in depth-first order.
    pub(super) fn next(
        &mut self,
        outputs: &mut Vec<Output>,
        path: Nibbles,
        node: BranchNodeCompact,
    ) -> Result<(), DatabaseError> {
        loop {
            // `curr` is None only if the end of the iterator has been reached. Any further nodes
            // found must be considered missing.
            if self.curr.is_none() {
                outputs.push(self.output_missing(path, node));
                return Ok(())
            }

            let (curr_path, curr_node) = self.curr.as_ref().expect("not None");
            trace!(target: "trie::verify", account=?self.account, ?curr_path, ?path, "Current cursor node");

            // Use depth-first ordering for comparison
            match depth_first::cmp(&path, curr_path) {
                Ordering::Less => {
                    // If the given path comes before the cursor's current path in depth-first
                    // order, then the given path was not produced by the cursor.
                    outputs.push(self.output_missing(path, node));
                    return Ok(())
                }
                Ordering::Equal => {
                    // If the current path matches the given one (happy path) but the nodes
                    // aren't equal then we produce a wrong node. Either way we want to move the
                    // iterator forward.
                    if *curr_node != node {
                        outputs.push(self.output_wrong(path, node, curr_node.clone()))
                    }
                    self.curr = self.trie_iter.next().transpose()?;
                    return Ok(())
                }
                Ordering::Greater => {
                    // If the given path comes after the current path in depth-first order,
                    // it means the cursor's path was not found by the caller (otherwise it would
                    // have hit the equal case) and so is extraneous.
                    outputs.push(self.output_extra(*curr_path, curr_node.clone()));
                    self.curr = self.trie_iter.next().transpose()?;
                    // back to the top of the loop to check the latest `self.curr` value against the
                    // given path/node.
                }
            }
        }
    }

    /// Must be called once there are no more calls to `next` to made. All further nodes produced
    /// by the iterator will be considered extraneous.
    pub(super) fn finalize(&mut self, outputs: &mut Vec<Output>) -> Result<(), DatabaseError> {
        loop {
            if let Some((curr_path, curr_node)) = self.curr.take() {
                outputs.push(self.output_extra(curr_path, curr_node));
                self.curr = self.trie_iter.next().transpose()?;
            } else {
                return Ok(())
            }
        }
    }
}
