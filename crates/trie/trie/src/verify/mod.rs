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
/// stops after the first error.
///
/// Storage work ahead of the account walk is bounded by the configured storage root limit.
/// Findings use a bounded channel shared by account and storage producers, so a slow consumer
/// applies backpressure without buffering an entire storage trie while waiting for its root.
#[derive(Debug)]
pub struct Verifier<T: TrieCursor, H, S> {
    events: Receiver<Result<Output, StateRootError>>,
    // The sole sender is dropped to broadcast cancellation to every producer.
    cancel: Option<Sender<()>>,
    // Retain account cursors until all producers have joined, before backing transactions drop.
    background: Option<JoinHandle<AccountVerifier<T, H, S>>>,
}

const FINDINGS_CAPACITY: usize = 1024;

const ACCOUNT_UPDATE_THRESHOLD: usize = 1024;

/// Default maximum number of storage roots computed or in flight ahead of the account walk.
pub const DEFAULT_STORAGE_ROOT_LIMIT: usize = 25_000_000;

impl<T, H, S> Verifier<T, H, S>
where
    T: TrieCursor + Send + 'static,
    H: HashedCursor<Value = Account> + Send + 'static,
    S: HashedStorageCursor<Value = U256> + Send + 'static,
{
    /// Starts verification and its background storage root computation.
    ///
    /// - `trie` reads the account trie table.
    /// - `accounts` yields account leaves in ascending address order, including default accounts
    ///   for addresses with hashed storage but no account row.
    /// - `storage_addresses` yields a job for each address present in hashed storage or stored
    ///   storage tries exactly once, in ascending address order, including addresses without an
    ///   account row. Jobs with hashed storage must request a root even without an account row. It
    ///   is walked on a background thread to schedule storage work ahead of `accounts`.
    /// - `storage` reads the hashed storage table.
    /// - `storage_root_limit` bounds account-backed storage jobs whose roots have not been
    ///   consumed.
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
    /// Panics if `storage_workers` is empty or `storage_root_limit` is zero.
    pub fn new<L, W, C, TX>(
        trie: T,
        accounts: H,
        storage: S,
        storage_addresses: L,
        storage_workers: Vec<StorageWorker<W, C, TX>>,
        storage_root_limit: usize,
    ) -> Result<Self, DatabaseError>
    where
        L: Iterator<Item = Result<StorageJob, DatabaseError>> + Send + 'static,
        W: TrieStorageCursor + Send + 'static,
        C: HashedStorageCursor<Value = U256> + Send + 'static,
        TX: Send + 'static,
    {
        let account_verifier = SingleVerifier::new(None, trie)?;
        assert!(!storage_workers.is_empty(), "verification requires storage workers");
        let (events_tx, events) = crossbeam_channel::bounded(FINDINGS_CAPACITY);
        let (cancel, cancel_rx) = crossbeam_channel::bounded(0);
        let context = WorkerContext::new(events_tx, cancel_rx, storage_root_limit);
        let mut account = AccountVerifier {
            accounts,
            storage,
            account_verifier,
            hash_builder: crate::HashBuilder::default().with_updates(true),
            account_rlp: Vec::with_capacity(crate::TRIE_ACCOUNT_RLP_MAX_SIZE),
        };
        let span = tracing::Span::current();
        let background = thread::Builder::new()
            .name("trie-accounts".into())
            .spawn(move || {
                let _entered = span.enter();
                match StoragePipeline::spawn(storage_addresses, storage_workers, context.clone()) {
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

impl<T: TrieCursor, H, S> Verifier<T, H, S> {
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

impl<T: TrieCursor, H, S> Iterator for Verifier<T, H, S> {
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

impl<T: TrieCursor, H, S> Drop for Verifier<T, H, S> {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Account producer state, returned from its thread so cursor lifetimes remain tied to `Verifier`.
#[derive(Debug)]
struct AccountVerifier<T: TrieCursor, H, S> {
    accounts: H,
    storage: S,
    account_verifier: SingleVerifier<DepthFirstTrieIterator<T>>,
    hash_builder: crate::HashBuilder,
    account_rlp: Vec<u8>,
}

impl<T, H, S> AccountVerifier<T, H, S>
where
    T: TrieCursor,
    H: HashedCursor<Value = Account>,
    S: HashedStorageCursor<Value = U256>,
{
    fn run(
        &mut self,
        pipeline: &mut StoragePipeline,
        context: &WorkerContext,
    ) -> Result<(), StateRootError> {
        let mut next = self.accounts.seek(B256::ZERO)?;
        while let Some((address, account)) = next {
            if context.is_cancelled() {
                return Ok(());
            }
            // Workers only send roots for non-empty storage.
            self.storage.set_hashed_address(address);
            let root = if self.storage.is_storage_empty()? {
                alloy_consensus::EMPTY_ROOT_HASH
            } else {
                let Some(root) = pipeline.root(address)? else { return Ok(()) };
                root
            };
            self.account_rlp.clear();
            account.into_trie_account(root).encode(&mut self.account_rlp);
            self.hash_builder.add_leaf(Nibbles::unpack(address), &self.account_rlp);
            if self.hash_builder.updates_len() >= ACCOUNT_UPDATE_THRESHOLD &&
                !self.verify_account_updates(context)?
            {
                return Ok(());
            }
            next = self.accounts.next()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashed_cursor::{
            mock::MockHashedCursorFactory, noop::NoopHashedCursor, HashedCursorFactory,
        },
        trie_cursor::{
            mock::MockTrieCursorFactory, noop::NoopStorageTrieCursor, TrieCursorFactory,
        },
    };
    use alloy_primitives::keccak256;
    use std::{
        collections::BTreeMap,
        time::{Duration, Instant},
    };

    #[test]
    fn account_producer_is_bounded_and_drop_unblocks_it() {
        let mut builder = crate::HashBuilder::default().with_updates(true);
        let leaves = (0u64..32768)
            .map(|n| (keccak256(n.to_be_bytes()), vec![1]))
            .collect::<BTreeMap<_, _>>();
        for (key, value) in leaves {
            builder.add_leaf(Nibbles::unpack(key), &value);
        }
        builder.root();
        let (_, nodes) = builder.split();
        let node_count = nodes.len();
        assert!(node_count > FINDINGS_CAPACITY * 2);
        let factory = MockTrieCursorFactory::new(nodes.into_iter().collect(), Default::default());
        let verifier = Verifier::new(
            factory.account_trie_cursor().unwrap(),
            NoopHashedCursor::<Account>::default(),
            NoopHashedCursor::<U256>::default(),
            std::iter::empty(),
            vec![StorageWorker {
                trie: NoopStorageTrieCursor::default(),
                hashed: NoopHashedCursor::<U256>::default(),
                tx: (),
            }],
            1,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while verifier.events.len() != FINDINGS_CAPACITY {
            assert!(Instant::now() < deadline, "producer did not fill the findings channel");
            thread::yield_now();
        }
        // The depth-first cursor may read ahead, but must not walk the entire stale trie.
        assert!(factory.visited_account_keys().len() < node_count / 2);
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let dropping = thread::spawn(move || {
            drop(verifier);
            done_tx.send(()).unwrap();
        });
        done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        dropping.join().unwrap();
    }

    #[test]
    fn storage_error_and_panic_stop_root_wait_and_fuse_iterator() {
        for panic in [false, true] {
            let (done_tx, done_rx) = crossbeam_channel::bounded(1);
            let task = thread::spawn(move || {
                let accounts = MockHashedCursorFactory::new(
                    BTreeMap::from([(B256::ZERO, Account::default())]),
                    Default::default(),
                );
                let mut verifier = Verifier::new(
                    crate::trie_cursor::noop::NoopAccountTrieCursor::default(),
                    accounts.hashed_account_cursor().unwrap(),
                    FailingStorage { panic },
                    std::iter::once(Ok(StorageJob { address: B256::ZERO, root_required: true })),
                    vec![StorageWorker {
                        trie: NoopStorageTrieCursor::default(),
                        hashed: FailingStorage { panic },
                        tx: (),
                    }],
                    1,
                )
                .unwrap();
                let error = verifier.next().unwrap().unwrap_err();
                assert_eq!(
                    error.to_string(),
                    if panic {
                        "trie verification thread panicked: storage failed"
                    } else {
                        "storage failed"
                    }
                );
                assert!(verifier.next().is_none());
                assert!(verifier.next().is_none());
                done_tx.send(()).unwrap();
            });
            done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            task.join().unwrap();
        }
    }

    struct FailingStorage {
        panic: bool,
    }

    impl HashedCursor for FailingStorage {
        type Value = U256;

        fn seek(&mut self, _key: B256) -> Result<Option<(B256, U256)>, DatabaseError> {
            assert!(!self.panic, "storage failed");
            Err(DatabaseError::Other("storage failed".into()))
        }

        fn next(&mut self) -> Result<Option<(B256, U256)>, DatabaseError> {
            unreachable!("storage seek fails first")
        }

        fn reset(&mut self) {}
    }

    impl HashedStorageCursor for FailingStorage {
        fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
            Ok(false)
        }

        fn set_hashed_address(&mut self, _address: B256) {}
    }
}
