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

/// Cursors for one parallel storage verification thread of a [`Verifier`](super::Verifier).
///
/// Both cursors may start at any hashed address. `tx` is the transaction backing both cursors; it
/// is held without being read and dropped after them.
#[derive(Debug)]
pub struct StorageWorker<T, H, TX> {
    /// Cursor over the storage trie table.
    pub trie: T,
    /// Cursor over the hashed storage table.
    pub hashed: H,
    /// Transaction backing `trie` and `hashed`.
    pub tx: TX,
}

impl<T, H, TX> StorageWorker<T, H, TX>
where
    T: TrieStorageCursor,
    H: HashedStorageCursor<Value = U256>,
{
    /// Verifies the storage of each address received from `jobs`, sending roots of non-empty
    /// storages to `roots` and findings through `context`. Returns once `jobs` is exhausted or
    /// verification is cancelled.
    pub(super) fn run(
        mut self,
        jobs: Receiver<StorageJob>,
        roots: &Sender<(B256, B256)>,
        context: &WorkerContext,
    ) -> Result<(), StateRootError> {
        loop {
            let Some(job) = context.receive(&jobs) else { return Ok(()) };
            let address = job.address;
            self.hashed.set_hashed_address(address);
            self.trie.set_hashed_address(address);
            // The account side assumes `EMPTY_ROOT_HASH` for empty storage and doesn't wait for a
            // root, so only leftover trie nodes are reported.
            if self.hashed.is_storage_empty()? {
                let mut verifier = SingleVerifier::new(Some(address), &mut self.trie)?;
                if !verifier.finalize(&mut |output| context.report_one(output))? {
                    return Ok(());
                }
                if job.root_required {
                    context.root_window.release();
                }
            } else {
                let Some(root) = self.verify_storage(address, context)? else { return Ok(()) };
                if job.root_required && !context.send(roots, (address, root)) {
                    return Ok(());
                }
            }
        }
    }

    /// Recomputes the storage trie of `address` from hashed state alone and compares it against
    /// the storage trie table. Work is done in bounded rounds so memory stays bounded for large
    /// storages and cancellation is observed between rounds.
    ///
    /// Returns `None` if verification was cancelled.
    fn verify_storage(
        &mut self,
        address: B256,
        context: &WorkerContext,
    ) -> Result<Option<B256>, StateRootError> {
        let mut verifier = SingleVerifier::new(Some(address), &mut self.trie)?;
        let mut state = None;
        loop {
            if context.is_cancelled() {
                return Ok(None);
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
            let mut emit = |output| context.report_one(output);
            for (path, node) in nodes {
                if context.is_cancelled() {
                    return Ok(None);
                }
                if !verifier.next(&mut emit, path, node)? {
                    return Ok(None);
                }
            }
            if let Some(root) = root {
                if !verifier.finalize(&mut emit)? {
                    return Ok(None);
                }
                return Ok(Some(root));
            }
        }
    }
}

/// Storage to verify, including whether the account walk needs its root.
#[derive(Debug)]
pub struct StorageJob {
    /// Hashed address of the storage to verify.
    pub address: B256,
    /// Whether to return the computed root to the account walk. False still verifies the storage
    /// trie, but discards its root and does not reserve space in the root backlog. Hashed storage
    /// needs a root even without a hashed account row; only stale trie-only storage may omit it.
    pub root_required: bool,
}
