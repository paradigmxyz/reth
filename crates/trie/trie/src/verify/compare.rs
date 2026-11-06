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
    curr: Option<(Nibbles, BranchNodeCompact)>,
}

impl<C: TrieCursor> SingleVerifier<DepthFirstTrieIterator<C>> {
    pub(super) fn new(account: Option<B256>, trie_cursor: C) -> Result<Self, DatabaseError> {
        let mut trie_iter = DepthFirstTrieIterator::new(trie_cursor);
        let curr = trie_iter.next().transpose()?;
        Ok(Self { account, trie_iter, curr })
    }
}

impl<I> SingleVerifier<I>
where
    I: Iterator<Item = Result<(Nibbles, BranchNodeCompact), DatabaseError>>,
{
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
    /// emit findings immediately if walking the trie cursor produces inconsistent data.
    /// Returning `false` from the callback cancels comparison before advancing the cursor again.
    ///
    /// `next` must be called with paths in depth-first order. Returns `true` on completion and
    /// `false` on cancellation. The verifier must not be reused after cancellation.
    pub(super) fn next(
        &mut self,
        emit: &mut impl FnMut(Output) -> bool,
        path: Nibbles,
        node: BranchNodeCompact,
    ) -> Result<bool, DatabaseError> {
        loop {
            // `curr` is None only if the end of the iterator has been reached. Any further nodes
            // found must be considered missing.
            if self.curr.is_none() {
                return Ok(emit(self.output_missing(path, node)))
            }

            let (curr_path, curr_node) = self.curr.as_ref().expect("not None");
            trace!(target: "trie::verify", account=?self.account, ?curr_path, ?path, "Current cursor node");

            // Use depth-first ordering for comparison
            match depth_first::cmp(&path, curr_path) {
                Ordering::Less => {
                    // If the given path comes before the cursor's current path in depth-first
                    // order, then the given path was not produced by the cursor.
                    return Ok(emit(self.output_missing(path, node)))
                }
                Ordering::Equal => {
                    // If the current path matches the given one (happy path) but the nodes
                    // aren't equal then we produce a wrong node. Either way we want to move the
                    // iterator forward.
                    if *curr_node != node && !emit(self.output_wrong(path, node, curr_node.clone()))
                    {
                        return Ok(false)
                    }
                    self.curr = self.trie_iter.next().transpose()?;
                    return Ok(true);
                }
                Ordering::Greater => {
                    // If the given path comes after the current path in depth-first order,
                    // it means the cursor's path was not found by the caller (otherwise it would
                    // have hit the equal case) and so is extraneous.
                    if !emit(self.output_extra(*curr_path, curr_node.clone())) {
                        return Ok(false);
                    }
                    self.curr = self.trie_iter.next().transpose()?;
                    // back to the top of the loop to check the latest `self.curr` value against the
                    // given path/node.
                }
            }
        }
    }

    /// Must be called once there are no more calls to `next` to be made. All further nodes produced
    /// by the iterator will be considered extraneous and emitted individually. Returning `false`
    /// from the callback stops comparison immediately; the verifier must not be reused after
    /// cancellation. Returns `true` on completion and `false` on cancellation.
    pub(super) fn finalize(
        &mut self,
        emit: &mut impl FnMut(Output) -> bool,
    ) -> Result<bool, DatabaseError> {
        loop {
            if let Some((curr_path, curr_node)) = self.curr.take() {
                if !emit(self.output_extra(curr_path, curr_node)) {
                    return Ok(false);
                }
                self.curr = self.trie_iter.next().transpose()?;
            } else {
                return Ok(true);
            }
        }
    }
}
