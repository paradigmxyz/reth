//! Storage verification using one worker's long-lived transaction and reusable cursors.

use super::{compare::SingleVerifier, pipeline::WorkerContext};
use crate::{
    hashed_cursor::HashedStorageCursor,
    trie::{StorageRootCalculation, DEFAULT_INTERMEDIATE_THRESHOLD},
    trie_cursor::{depth_first, noop::NoopStorageTrieCursor, TrieStorageCursor},
    StorageRootProgress,
};
use alloy_primitives::{B256, U256};
use crossbeam_channel::{Receiver, Sender};
use reth_execution_errors::StateRootError;

/// A storage worker's reusable cursors and the transaction keeping them alive.
///
/// Fields are dropped in order, so the transaction outlives both cursors.
#[derive(Debug)]
pub struct StorageWorker<T, H, TX> {
    /// Cursor over persisted storage trie nodes, repositioned for each account.
    pub trie: T,
    /// Cursor over hashed storage entries, repositioned for each account.
    pub hashed: H,
    /// Independent long-lived read transaction, retained until this worker exits.
    pub transaction: TX,
}

impl<T, H, TX> StorageWorker<T, H, TX>
where
    T: TrieStorageCursor,
    H: HashedStorageCursor<Value = U256>,
{
    pub(super) fn run(
        mut self,
        jobs: Receiver<B256>,
        roots: &Sender<(B256, B256)>,
        context: &WorkerContext,
    ) -> Result<(), StateRootError> {
        loop {
            let Ok(address) = context.receive(&jobs) else { return Ok(()) };
            self.hashed.set_hashed_address(address);
            self.trie.set_hashed_address(address);
            if self.hashed.is_storage_empty()? {
                let mut verifier = SingleVerifier::new(Some(address), &mut self.trie)?;
                let mut found = Vec::new();
                verifier.finalize(&mut found)?;
                if !context.report(found) {
                    return Ok(())
                }
            } else {
                let Some(root) = self.verify_storage(address, context)? else { return Ok(()) };
                if !context.send(roots, (address, root)) {
                    return Ok(())
                }
            }
        }
    }

    fn verify_storage(
        &mut self,
        address: B256,
        context: &WorkerContext,
    ) -> Result<Option<B256>, StateRootError> {
        let mut verifier = SingleVerifier::new(Some(address), &mut self.trie)?;
        let mut state = None;
        loop {
            if context.is_cancelled() {
                return Ok(None)
            }
            let result = StorageRootCalculation {
                hashed_address: address,
                prefix_set: Default::default(),
                previous_state: state.take(),
                walk_all_changed_branch_children: false,
                threshold: DEFAULT_INTERMEDIATE_THRESHOLD,
                retain_updates: true,
            }
            .calculate(
                &mut NoopStorageTrieCursor::default(),
                &mut self.hashed,
                #[cfg(feature = "metrics")]
                &Default::default(),
            )?;
            let (root, updates) = match result {
                StorageRootProgress::Complete(root, _, updates) => (Some(root), updates),
                StorageRootProgress::Progress(next, _, updates) => {
                    state = Some(*next);
                    (None, updates)
                }
            };
            let mut nodes = updates.storage_nodes.into_iter().collect::<Vec<_>>();
            nodes.sort_unstable_by(|a, b| depth_first::cmp(&a.0, &b.0));
            let mut found = Vec::new();
            for (path, node) in nodes {
                verifier.next(&mut found, path, node)?;
            }
            if root.is_some() {
                verifier.finalize(&mut found)?;
            }
            if !context.report(found) {
                return Ok(None)
            }
            if root.is_some() {
                return Ok(root)
            }
        }
    }
}
