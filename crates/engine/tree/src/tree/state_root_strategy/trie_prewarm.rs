//! Trie-table page prewarming driven by the block access list.
//!
//! The proof workers spend most of a chunk's wall time in major page faults on the trie tables,
//! one 4 KiB page per fault because the database is opened with `no_rdahead`. The block access
//! list names every touched address and every written slot before execution starts, which is the
//! same set of keys the proof workers will look up. This walks those keys on its own threads
//! while the block executes, so the pages are resident by the time a proof chunk needs them.
//!
//! The walk is read-only and its results are discarded: it changes no state and cannot change the
//! state root, only which pages are in the page cache.

use alloy_eip7928::bal::DecodedBal;
use alloy_primitives::{keccak256, B256};
use metrics::{Counter, Histogram};
use parking_lot::Mutex;
use reth_metrics::Metrics;
use reth_primitives_traits::FastInstant as Instant;
use reth_provider::DatabaseProviderROFactory;
use reth_trie::{
    hashed_cursor::{HashedCursor, HashedCursorFactory, HashedStorageCursor},
    trie_cursor::{TrieCursor, TrieCursorFactory, TrieStorageCursor},
    Nibbles,
};
use std::{
    ops::Range,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread::JoinHandle,
};
use tracing::debug;

/// Long-lived pool of blocking threads that warm the trie-table pages a block's proofs will read.
///
/// Owned by the [`DefaultStateRootStrategy`](super::DefaultStateRootStrategy), so the threads live
/// for the lifetime of the node and are never the proof pools or the coordinator.
#[derive(Debug)]
pub(super) struct TriePrewarmPool {
    /// Shared work queue; every worker takes one job per dispatch.
    job_tx: crossbeam_channel::Sender<TriePrewarmJob>,
    num_threads: usize,
    /// The queue of the most recently dispatched block, cancelled when the next block arrives so
    /// warming never runs into the block after the one it was meant for.
    current: Mutex<Option<Arc<TrieWarmQueue>>>,
    metrics: TriePrewarmMetrics,
    _handles: Vec<JoinHandle<()>>,
}

impl TriePrewarmPool {
    /// Spawns `num_threads` long-lived blocking threads.
    pub(super) fn new(num_threads: usize) -> Arc<Self> {
        let (job_tx, job_rx) = crossbeam_channel::unbounded::<TriePrewarmJob>();
        let handles = (0..num_threads)
            .map(|i| {
                let job_rx = job_rx.clone();
                std::thread::Builder::new()
                    .name(format!("trie-prewarm-{i:03}"))
                    .spawn(move || {
                        while let Ok(job) = job_rx.recv() {
                            job();
                        }
                    })
                    .expect("spawn trie-prewarm thread")
            })
            .collect();

        debug!(target: "engine::tree::trie_prewarm", num_threads, "Spawned trie prewarm pool");
        Arc::new(Self {
            job_tx,
            num_threads,
            current: Mutex::new(None),
            metrics: TriePrewarmMetrics::default(),
            _handles: handles,
        })
    }

    /// Queues the block access list for warming and returns immediately.
    ///
    /// Each worker opens its own read-only provider and takes batches of accounts off the shared
    /// queue in block access list order, so the warming front follows the order in which the
    /// hashed-state stream hands targets to the proof workers.
    pub(super) fn dispatch<F>(&self, bal: Arc<DecodedBal>, factory: F)
    where
        F: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
            + Send
            + Sync
            + 'static,
    {
        if bal.as_bal().is_empty() {
            return
        }

        let queue = Arc::new(TrieWarmQueue::new(bal, self.num_threads));
        if let Some(previous) = self.current.lock().replace(queue.clone()) {
            previous.cancel();
        }

        let started = Instant::now();
        let metrics = self.metrics.clone();
        let job: TriePrewarmJob = Arc::new(move || {
            if !queue.is_cancelled() {
                match factory.database_provider_ro() {
                    Ok(provider) => warm_trie_pages(&provider, &queue),
                    Err(_) => {
                        queue.errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            queue.finish_worker(&metrics, started);
        });
        for _ in 0..self.num_threads {
            let _ = self.job_tx.send(job.clone());
        }
    }
}

/// One worker's share of a dispatch: opens a provider and drains the shared queue. Every worker
/// runs the same job once.
type TriePrewarmJob = Arc<dyn Fn() + Send + Sync>;

/// Number of block access list accounts a worker claims at a time.
///
/// Small enough that the warming front stays close to block access list order across all workers,
/// large enough that the claim is not a per-account atomic.
const WARM_BATCH_SIZE: usize = 4;

/// Shared, cancellable cursor into one block's access list.
#[derive(Debug)]
struct TrieWarmQueue {
    bal: Arc<DecodedBal>,
    /// Index of the next unclaimed account.
    next: AtomicUsize,
    cancelled: AtomicBool,
    /// Workers that have not yet finished this dispatch.
    active: AtomicUsize,
    accounts: AtomicUsize,
    slots: AtomicUsize,
    errors: AtomicUsize,
}

impl TrieWarmQueue {
    const fn new(bal: Arc<DecodedBal>, num_threads: usize) -> Self {
        Self {
            bal,
            next: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            active: AtomicUsize::new(num_threads),
            accounts: AtomicUsize::new(0),
            slots: AtomicUsize::new(0),
            errors: AtomicUsize::new(0),
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Claims the next batch of accounts, or `None` once the list is exhausted or cancelled.
    fn next_batch(&self, len: usize) -> Option<Range<usize>> {
        if self.is_cancelled() {
            return None
        }
        let start = self.next.fetch_add(WARM_BATCH_SIZE, Ordering::Relaxed);
        (start < len).then(|| start..(start + WARM_BATCH_SIZE).min(len))
    }

    /// Records the last worker's results for the block.
    fn finish_worker(&self, metrics: &TriePrewarmMetrics, started: Instant) {
        if self.active.fetch_sub(1, Ordering::AcqRel) != 1 {
            return
        }

        let elapsed = started.elapsed();
        let accounts = self.accounts.load(Ordering::Relaxed);
        let slots = self.slots.load(Ordering::Relaxed);
        let errors = self.errors.load(Ordering::Relaxed);
        metrics.trie_prewarm_accounts.increment(accounts as u64);
        metrics.trie_prewarm_slots.increment(slots as u64);
        metrics.trie_prewarm_errors.increment(errors as u64);
        metrics.trie_prewarm_duration_histogram.record(elapsed.as_secs_f64());
        debug!(
            target: "engine::tree::trie_prewarm",
            accounts,
            slots,
            errors,
            cancelled = self.is_cancelled(),
            ?elapsed,
            "Warmed trie pages"
        );
    }
}

/// Walks the trie and hashed tables for every account the queue hands out.
///
/// For each account: the account trie path to its leaf, its hashed account entry, its storage trie
/// root node, and for every slot the block writes, that slot's storage trie path and hashed entry.
/// Every result is discarded; only the page-cache side effect matters.
fn warm_trie_pages<P>(provider: &P, queue: &TrieWarmQueue)
where
    P: TrieCursorFactory + HashedCursorFactory,
{
    let (Ok(mut account_trie), Ok(mut hashed_accounts), Ok(mut storage_trie), Ok(mut hashed_slots)) = (
        provider.account_trie_cursor(),
        provider.hashed_account_cursor(),
        provider.storage_trie_cursor(B256::ZERO),
        provider.hashed_storage_cursor(B256::ZERO),
    ) else {
        queue.errors.fetch_add(1, Ordering::Relaxed);
        return
    };

    let bal = queue.bal.as_bal();
    let mut accounts = 0usize;
    let mut slots = 0usize;
    let mut errors = 0usize;

    // The block access list is ordered by address, and the keys these cursors take are hashed,
    // so consecutive seeks go backwards as often as forwards. Both cursor types only move
    // forward from where they last stopped and would silently skip a backwards seek, so every
    // lookup starts from a reset cursor.
    while let Some(batch) = queue.next_batch(bal.len()) {
        for changes in &bal[batch] {
            let hashed_address = keccak256(changes.address);
            account_trie.reset();
            errors += account_trie.seek(Nibbles::unpack(hashed_address)).is_err() as usize;
            hashed_accounts.reset();
            errors += hashed_accounts.seek(hashed_address).is_err() as usize;

            // Also resets the cursor.
            storage_trie.set_hashed_address(hashed_address);
            errors += storage_trie.seek(Nibbles::default()).is_err() as usize;
            accounts += 1;

            if changes.storage_changes.is_empty() {
                continue
            }

            hashed_slots.set_hashed_address(hashed_address);
            for slot in &changes.storage_changes {
                let hashed_slot = keccak256(slot.slot.to_be_bytes::<32>());
                storage_trie.reset();
                errors += storage_trie.seek(Nibbles::unpack(hashed_slot)).is_err() as usize;
                hashed_slots.reset();
                errors += hashed_slots.seek(hashed_slot).is_err() as usize;
                slots += 1;
            }
        }
    }

    queue.accounts.fetch_add(accounts, Ordering::Relaxed);
    queue.slots.fetch_add(slots, Ordering::Relaxed);
    queue.errors.fetch_add(errors, Ordering::Relaxed);
}

/// Runs the whole walk on the calling thread and returns what it touched.
#[cfg(test)]
pub(super) fn warm_trie_pages_blocking<P>(provider: &P, bal: Arc<DecodedBal>) -> WarmedPages
where
    P: TrieCursorFactory + HashedCursorFactory,
{
    let queue = TrieWarmQueue::new(bal, 1);
    warm_trie_pages(provider, &queue);
    WarmedPages {
        accounts: queue.accounts.load(Ordering::Relaxed),
        slots: queue.slots.load(Ordering::Relaxed),
        errors: queue.errors.load(Ordering::Relaxed),
    }
}

/// What one walk touched, returned by [`warm_trie_pages_blocking`].
#[cfg(test)]
#[derive(Debug)]
pub(super) struct WarmedPages {
    pub(super) accounts: usize,
    pub(super) slots: usize,
    pub(super) errors: usize,
}

/// Metrics recorded by the trie page prewarm.
#[derive(Metrics, Clone)]
#[metrics(scope = "tree.root")]
struct TriePrewarmMetrics {
    /// Number of block access list accounts whose trie paths were walked.
    trie_prewarm_accounts: Counter,
    /// Number of written storage slots whose trie paths were walked.
    trie_prewarm_slots: Counter,
    /// Number of cursor operations that returned an error.
    trie_prewarm_errors: Counter,
    /// Histogram of the wall time from dispatch to the last warming thread finishing.
    trie_prewarm_duration_histogram: Histogram,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{bal::Bal, AccountChanges, BlockAccessIndex, SlotChanges, StorageChange};
    use alloy_primitives::{Address, U256};
    use reth_db::DatabaseError;
    use reth_primitives_traits::Account;
    use reth_trie::BranchNodeCompact;

    /// Every key the walk seeks, per table.
    #[derive(Debug, Default)]
    struct SeekLog {
        account_trie: Vec<Nibbles>,
        hashed_accounts: Vec<B256>,
        storage_trie: Vec<(B256, Nibbles)>,
        hashed_slots: Vec<(B256, B256)>,
    }

    /// Cursor factory that records seeks instead of reading a database.
    #[derive(Debug, Default, Clone)]
    struct RecordingFactory {
        log: Arc<Mutex<SeekLog>>,
    }

    #[derive(Debug)]
    struct RecordingTrieCursor {
        log: Arc<Mutex<SeekLog>>,
        hashed_address: Option<B256>,
    }

    impl TrieCursor for RecordingTrieCursor {
        fn seek_exact(
            &mut self,
            key: Nibbles,
        ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            self.seek(key)
        }

        fn seek(
            &mut self,
            key: Nibbles,
        ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            let mut log = self.log.lock();
            match self.hashed_address {
                Some(hashed_address) => log.storage_trie.push((hashed_address, key)),
                None => log.account_trie.push(key),
            }
            Ok(None)
        }

        fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            Ok(None)
        }

        fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
            Ok(None)
        }

        fn reset(&mut self) {}
    }

    impl TrieStorageCursor for RecordingTrieCursor {
        fn set_hashed_address(&mut self, hashed_address: B256) {
            self.hashed_address = Some(hashed_address);
        }
    }

    #[derive(Debug)]
    struct RecordingHashedCursor<V> {
        log: Arc<Mutex<SeekLog>>,
        hashed_address: Option<B256>,
        _value: std::marker::PhantomData<V>,
    }

    impl<V: std::fmt::Debug> HashedCursor for RecordingHashedCursor<V> {
        type Value = V;

        fn seek(&mut self, key: B256) -> Result<Option<(B256, V)>, DatabaseError> {
            let mut log = self.log.lock();
            match self.hashed_address {
                Some(hashed_address) => log.hashed_slots.push((hashed_address, key)),
                None => log.hashed_accounts.push(key),
            }
            Ok(None)
        }

        fn next(&mut self) -> Result<Option<(B256, V)>, DatabaseError> {
            Ok(None)
        }

        fn reset(&mut self) {}
    }

    impl<V: std::fmt::Debug> HashedStorageCursor for RecordingHashedCursor<V> {
        fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
            Ok(false)
        }

        fn set_hashed_address(&mut self, hashed_address: B256) {
            self.hashed_address = Some(hashed_address);
        }
    }

    impl TrieCursorFactory for RecordingFactory {
        type AccountTrieCursor<'a>
            = RecordingTrieCursor
        where
            Self: 'a;
        type StorageTrieCursor<'a>
            = RecordingTrieCursor
        where
            Self: 'a;

        fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
            Ok(RecordingTrieCursor { log: self.log.clone(), hashed_address: None })
        }

        fn storage_trie_cursor(
            &self,
            hashed_address: B256,
        ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
            Ok(RecordingTrieCursor { log: self.log.clone(), hashed_address: Some(hashed_address) })
        }
    }

    impl HashedCursorFactory for RecordingFactory {
        type AccountCursor<'a>
            = RecordingHashedCursor<Account>
        where
            Self: 'a;
        type StorageCursor<'a>
            = RecordingHashedCursor<U256>
        where
            Self: 'a;

        fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
            Ok(RecordingHashedCursor {
                log: self.log.clone(),
                hashed_address: None,
                _value: Default::default(),
            })
        }

        fn hashed_storage_cursor(
            &self,
            hashed_address: B256,
        ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
            Ok(RecordingHashedCursor {
                log: self.log.clone(),
                hashed_address: Some(hashed_address),
                _value: Default::default(),
            })
        }
    }

    fn decoded_bal(accounts: Vec<AccountChanges>) -> Arc<DecodedBal> {
        Arc::new(DecodedBal::new(Bal::from(accounts), Default::default()))
    }

    fn account_changes(address: Address, written_slots: &[u64]) -> AccountChanges {
        let mut changes = AccountChanges::new(address);
        changes.storage_changes = written_slots
            .iter()
            .map(|slot| SlotChanges {
                slot: U256::from(*slot),
                changes: vec![StorageChange {
                    block_access_index: BlockAccessIndex(0),
                    new_value: U256::from(1),
                }],
            })
            .collect();
        changes
    }

    #[test]
    fn warms_trie_tables_for_every_bal_account_and_written_slot() {
        let with_storage = Address::repeat_byte(0x11);
        let without_storage = Address::repeat_byte(0x22);
        let bal = decoded_bal(vec![
            account_changes(with_storage, &[1, 2]),
            account_changes(without_storage, &[]),
        ]);

        let factory = RecordingFactory::default();
        let queue = TrieWarmQueue::new(bal, 1);
        warm_trie_pages(&factory, &queue);

        let log = factory.log.lock();
        let hashed_with_storage = keccak256(with_storage);
        let hashed_without_storage = keccak256(without_storage);
        let hashed_slot_1 = keccak256(U256::from(1).to_be_bytes::<32>());
        let hashed_slot_2 = keccak256(U256::from(2).to_be_bytes::<32>());

        assert_eq!(
            log.account_trie,
            vec![Nibbles::unpack(hashed_with_storage), Nibbles::unpack(hashed_without_storage)]
        );
        assert_eq!(log.hashed_accounts, vec![hashed_with_storage, hashed_without_storage]);
        assert_eq!(
            log.storage_trie,
            vec![
                (hashed_with_storage, Nibbles::default()),
                (hashed_with_storage, Nibbles::unpack(hashed_slot_1)),
                (hashed_with_storage, Nibbles::unpack(hashed_slot_2)),
                (hashed_without_storage, Nibbles::default()),
            ]
        );
        assert_eq!(
            log.hashed_slots,
            vec![(hashed_with_storage, hashed_slot_1), (hashed_with_storage, hashed_slot_2)]
        );

        assert_eq!(queue.accounts.load(Ordering::Relaxed), 2);
        assert_eq!(queue.slots.load(Ordering::Relaxed), 2);
        assert_eq!(queue.errors.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cancelled_queue_warms_nothing() {
        let bal = decoded_bal(vec![account_changes(Address::repeat_byte(0x11), &[1])]);

        let factory = RecordingFactory::default();
        let queue = TrieWarmQueue::new(bal, 1);
        queue.cancel();
        warm_trie_pages(&factory, &queue);

        assert!(factory.log.lock().account_trie.is_empty());
        assert_eq!(queue.accounts.load(Ordering::Relaxed), 0);
    }
}
