//! Parallel verification of persisted account and storage tries.

use self::{
    compare::SingleVerifier,
    pipeline::{StoragePipeline, WorkerContext},
};
use crate::{
    hashed_cursor::{HashedCursor, HashedStorageCursor},
    trie_cursor::{
        depth_first::{self, DepthFirstTrieIterator},
        TrieCursor, TrieStorageCursor,
    },
    Nibbles,
};
use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use alloy_trie::BranchNodeCompact;
use crossbeam_channel::{Receiver, Sender};
use reth_execution_errors::StateRootError;
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use std::{
    sync::Arc,
    thread::{self, JoinHandle},
};

mod compare;
mod pipeline;
mod storage;
pub use storage::{StorageJob, StorageWorker};

/// Compares the persisted account and storage trie tables against tries recomputed from the hashed
/// state tables, yielding an [`Output`] for every inconsistency found.
///
/// The hashed tables are the source of truth. Storage tries are recomputed in parallel, so storage
/// findings are not ordered relative to account findings or to [`Output::Progress`]. Iteration
/// stops after the first error. Only hashed account rows contribute account leaves; orphan storage
/// tries are still verified independently.
///
/// Storage work ahead of the account walk is bounded by the configured storage lookahead.
/// Findings use a bounded channel shared by account and storage producers, so a slow consumer
/// applies backpressure without buffering an entire storage trie while waiting for its root.
#[derive(Debug)]
pub struct Verifier<T: TrieCursor, H> {
    events: Receiver<Result<Output, StateRootError>>,
    // The sole sender is dropped to broadcast cancellation to every producer.
    cancel: Option<Sender<()>>,
    // Retain account cursors until all producers have joined, before backing transactions drop.
    background: Option<JoinHandle<AccountVerifier<T, H>>>,
}

const FINDINGS_CAPACITY: usize = 1024;

const ACCOUNT_UPDATE_THRESHOLD: usize = 1024;

/// Default maximum number of storage roots computed or in flight ahead of the account walk.
pub const DEFAULT_STORAGE_LOOKAHEAD: usize = 1_000_000;

impl<T, H> Verifier<T, H>
where
    T: TrieCursor + Send + 'static,
    H: HashedCursor<Value = Account> + Send + 'static,
{
    /// Starts verification and its background storage root computation.
    ///
    /// - `trie` reads the account trie table.
    /// - `accounts` reads the hashed accounts table in ascending address order.
    /// - `storage_addresses` yields a job for each address present in hashed storage or stored
    ///   storage tries exactly once, in ascending address order, including addresses without an
    ///   account row. Jobs with hashed storage must request a root even without an account row. It
    ///   is walked on a background thread to schedule storage work ahead of `accounts`.
    /// - `storage_lookahead` bounds storage jobs whose requested roots have not been consumed,
    ///   including storage without a hashed account row.
    /// - `storage_workers` sets the storage parallelism, one thread per worker.
    ///
    /// Storage cursors may start at any hashed address. All cursors must read the same database
    /// version, and any transactions backing them must stay open until the verifier is dropped.
    /// The account walk runs on a producer thread. Account and storage findings share a bounded
    /// channel, consumed by this iterator independently of storage root waits. Account cursors
    /// must be transferable to that thread. Dropping the verifier cancels and joins all work.
    ///
    /// # Errors
    ///
    /// Returns an error if the account trie cannot be read or the account thread cannot be
    /// spawned. Errors starting storage threads are yielded by the iterator.
    ///
    /// # Panics
    ///
    /// Panics if `storage_workers` is empty or `storage_lookahead` is zero.
    pub fn new<L, W, C, TX>(
        trie: T,
        accounts: H,
        storage_addresses: L,
        storage_workers: Vec<StorageWorker<W, C, TX>>,
        storage_lookahead: usize,
    ) -> Result<Self, DatabaseError>
    where
        L: Iterator<Item = Result<StorageJob, DatabaseError>> + Send + 'static,
        W: TrieStorageCursor + Send + 'static,
        C: HashedStorageCursor<Value = U256> + Send + 'static,
        TX: Send + 'static,
    {
        let account_verifier = SingleVerifier::new(None, trie)?;
        assert!(!storage_workers.is_empty(), "verification requires storage workers");
        assert!(storage_lookahead > 0, "storage lookahead must be positive");
        let (events_tx, events) = crossbeam_channel::bounded(FINDINGS_CAPACITY);
        let (cancel, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx);
        let mut account = AccountVerifier {
            accounts,
            account_verifier,
            hash_builder: crate::HashBuilder::default().with_updates(true),
            account_rlp: Vec::with_capacity(crate::TRIE_ACCOUNT_RLP_MAX_SIZE),
        };
        let span = tracing::Span::current();
        let background = thread::Builder::new()
            .name("trie-accounts".into())
            .spawn(move || {
                let _entered = span.enter();
                match StoragePipeline::spawn(
                    storage_addresses,
                    storage_workers,
                    context.clone(),
                    storage_lookahead,
                ) {
                    Ok(mut pipeline) => {
                        // Report failures before joining: the consumer cancels all producers on
                        // error.
                        context.run(|| account.run(&mut pipeline, &context));
                        pipeline.finish();
                    }
                    Err(error) => context.run(|| Err(error.into())),
                }
                account
            })
            .map_err(|error| DatabaseError::Custom(Arc::new(error)))?;
        Ok(Self { events, cancel: Some(cancel), background: Some(background) })
    }
}

impl<T: TrieCursor, H> Verifier<T, H> {
    fn stop(&mut self) {
        self.cancel.take();
        if let Some(background) = self.background.take() &&
            let Err(panic) = background.join() &&
            !thread::panicking()
        {
            std::panic::resume_unwind(panic);
        }
    }
}

impl<T: TrieCursor, H> Iterator for Verifier<T, H> {
    type Item = Result<Output, StateRootError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.cancel.as_ref()?;
        match self.events.recv() {
            Ok(Ok(output)) => Some(Ok(output)),
            Ok(Err(error)) => {
                self.stop();
                Some(Err(error))
            }
            Err(_) => {
                self.stop();
                None
            }
        }
    }
}

impl<T: TrieCursor, H> Drop for Verifier<T, H> {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Account producer state, returned from its thread so cursor lifetimes remain tied to `Verifier`.
#[derive(Debug)]
struct AccountVerifier<T: TrieCursor, H> {
    accounts: H,
    account_verifier: SingleVerifier<DepthFirstTrieIterator<T>>,
    hash_builder: crate::HashBuilder,
    account_rlp: Vec<u8>,
}

impl<T, H> AccountVerifier<T, H>
where
    T: TrieCursor,
    H: HashedCursor<Value = Account>,
{
    fn run(
        &mut self,
        pipeline: &mut StoragePipeline,
        context: &WorkerContext,
    ) -> Result<(), StateRootError> {
        let mut next_account = self.accounts.seek(B256::ZERO)?;
        loop {
            if context.is_cancelled() {
                return Ok(());
            }
            // The scheduler announces storage addresses in order before dispatching their jobs.
            let next_root = pipeline.next_root(next_account.as_ref().map(|(address, _)| *address));
            if context.is_cancelled() {
                return Ok(());
            }
            let (address, root) = match (next_account.as_ref(), next_root) {
                (_, Some(storage)) => storage,
                (Some((address, _)), None) => (*address, alloy_consensus::EMPTY_ROOT_HASH),
                (None, None) => break,
            };
            let account = match next_account.take() {
                Some((key, account)) if key == address => {
                    next_account = self.accounts.next()?;
                    account
                }
                next => {
                    next_account = next;
                    // Canonical state roots only include leaves present in hashed accounts.
                    // Orphan storage is still verified, but cannot create an account leaf.
                    continue;
                }
            };
            self.account_rlp.clear();
            account.into_trie_account(root).encode(&mut self.account_rlp);
            self.hash_builder.add_leaf(Nibbles::unpack(address), &self.account_rlp);
            if self.hash_builder.updates_len() >= ACCOUNT_UPDATE_THRESHOLD &&
                !self.verify_account_updates(context)?
            {
                return Ok(());
            }
        }
        // Computing the root flushes the remaining branch node updates.
        self.hash_builder.root();
        if !self.verify_account_updates(context)? {
            return Ok(());
        }
        self.account_verifier.finalize(&mut |output| context.report_one(output))?;
        Ok(())
    }

    fn verify_account_updates(&mut self, context: &WorkerContext) -> Result<bool, StateRootError> {
        let (builder, updates) = std::mem::take(&mut self.hash_builder).split();
        self.hash_builder = builder.with_updates(true);
        // The account trie root is calculated but never persisted. Updates are sorted depth-first
        // because that is the order in which the `HashBuilder` completes branch nodes, so each
        // batch continues where the previous one ended.
        let mut nodes =
            updates.into_iter().filter(|(path, _)| !path.is_empty()).collect::<Vec<_>>();
        nodes.sort_unstable_by(|a, b| depth_first::cmp(&a.0, &b.0));
        for (path, node) in nodes {
            if !self.account_verifier.next(&mut |output| context.report_one(output), path, node)? ||
                !context.report_one(Output::Progress(path))
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Output describes an inconsistency found when comparing the hashed state tables
/// ([`HashedCursor`]) with that of the trie tables ([`TrieCursor`]). The hashed
/// tables are considered the source of truth; outputs are on the part of the trie tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// An extra account node was found.
    AccountExtra(Nibbles, BranchNodeCompact),
    /// An extra storage node was found.
    StorageExtra(B256, Nibbles, BranchNodeCompact),
    /// An account node had the wrong value.
    AccountWrong {
        /// Path of the node
        path: Nibbles,
        /// The node's expected value.
        expected: BranchNodeCompact,
        /// The node's found value.
        found: BranchNodeCompact,
    },
    /// A storage node had the wrong value.
    StorageWrong {
        /// The account the storage trie belongs to.
        account: B256,
        /// Path of the node
        path: Nibbles,
        /// The node's expected value.
        expected: BranchNodeCompact,
        /// The node's found value.
        found: BranchNodeCompact,
    },
    /// An account node was missing.
    AccountMissing(Nibbles, BranchNodeCompact),
    /// A storage node was missing.
    StorageMissing(B256, Nibbles, BranchNodeCompact),
    /// Progress indicator with the path of the last verified account trie node.
    Progress(Nibbles),
}

/// Returns the default number of [`StorageWorker`]s to pass to [`Verifier::new`]: one per available
/// CPU, or one if the CPU count cannot be determined.
pub fn default_storage_workers() -> usize {
    thread::available_parallelism().map_or(1, usize::from)
}
