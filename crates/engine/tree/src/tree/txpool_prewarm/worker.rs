use super::{
    control::{Command, Publication},
    Job, Source, Transactions,
};
use crate::tree::{StateProviderDatabase, TxPoolPrewarmCacheSnapshot as Snapshot};
use alloy_evm::Evm;
use alloy_primitives::B256;
use crossbeam_channel::{Receiver, RecvTimeoutError, TryRecvError};
use reth_evm::ConfigureEvm;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{BlockReader, StateProviderFactory, StateReader};
use reth_revm::{cached::CachedReads, db::State};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{debug, trace};

/// Maximum interval between snapshot publications and delay when no transaction is ready.
const REFRESH_INTERVAL: Duration = Duration::from_millis(100);

/// Delay while waiting for pool maintenance to advance to the state being warmed.
const HEAD_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The txpool prewarming worker.
///
/// A long-lived loop that speculatively executes the pool's best transactions on top of the
/// current canonical state, recording every state read in a [`CachedReads`]. Roughly every
/// [`REFRESH_INTERVAL`] it publishes an immutable snapshot of that cache, which block validation
/// and payload building use to seed their own caches.
///
/// The worker is driven by [`Command`]s: `Start` points it at a new parent state, and
/// `Pause`/`Resume` bracket cache-sensitive work elsewhere. Commands are only applied between
/// batches, never while an EVM or state provider is alive.
pub(super) struct Worker<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    /// Control commands from the [`Handle`](super::Handle).
    commands: Receiver<Command<Job<N, P, Evm>>>,
    /// Shared slot the latest snapshot is published into.
    publication: Publication,
    /// The txpool view transactions are drawn from.
    source: Arc<dyn Source<N>>,
    /// Configures the EVM used for speculative execution.
    evm_config: Evm,
    /// The parent state to warm, from the most recent `Start` command.
    job: Option<(B256, Job<N, P, Evm>)>,
    /// Outstanding pauses; the worker only warms while this is zero.
    pauses: u64,
    /// Read-through cache filled by execution; replaced whenever the warmed parent changes.
    cache: CachedReads,
    /// Parent whose state the `cache` reads were collected against.
    cache_parent: Option<B256>,
    /// Cache entry counts as of the last publication. The cache only ever grows, so a change
    /// means it holds unpublished reads.
    published_entries: (usize, usize, usize),
    /// Live best-transactions iterator, tagged with the parent it was opened for.
    transactions: Option<(B256, Transactions<N>)>,
}

impl<N, P, Evm> Worker<N, P, Evm>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone,
    Evm: ConfigureEvm<Primitives = N>,
{
    pub(super) fn new(
        commands: Receiver<Command<Job<N, P, Evm>>>,
        publication: Publication,
        source: Arc<dyn Source<N>>,
        evm_config: Evm,
    ) -> Self {
        Self {
            commands,
            publication,
            source,
            evm_config,
            job: None,
            pauses: 0,
            cache: CachedReads::default(),
            cache_parent: None,
            published_entries: (0, 0, 0),
            transactions: None,
        }
    }

    /// Runs until the control side is dropped, which is the worker's shutdown signal.
    pub(super) fn run(mut self) {
        let _ = self.run_until_disconnected();
    }

    fn run_until_disconnected(&mut self) -> Result<(), ChannelDisconnected> {
        loop {
            let parent_hash = self.wait_until_runnable()?;

            // The pool tracks canonical heads on its own schedule; check back shortly if it is
            // not tracking this parent yet.
            if !self.open_transactions(parent_hash) {
                self.idle(HEAD_POLL_INTERVAL)?;
                continue
            }

            if self.cache_parent != Some(parent_hash) {
                self.cache = CachedReads::default();
                self.cache_parent = Some(parent_hash);
                self.published_entries = (0, 0, 0);
                debug!(
                    target: "engine::tree::txpool_prewarm",
                    ?parent_hash,
                    "started txpool prewarming"
                );
            }

            let batch = self.warm_one_batch();

            // A pending command may pause the worker or point it at a new parent: apply it (at
            // the top of the loop) before spending time on publication.
            if !self.commands.is_empty() {
                continue
            }
            self.publish_snapshot_if_dirty();
            if batch == BatchEnd::Rest {
                self.idle(REFRESH_INTERVAL)?;
            }
        }
    }

    /// Blocks until the worker holds a job and no pauses are outstanding, applying every command
    /// that arrives in the meantime. Returns the parent hash to warm.
    fn wait_until_runnable(&mut self) -> Result<B256, ChannelDisconnected> {
        loop {
            self.apply_pending_commands()?;

            if self.pauses == 0 &&
                let Some((parent_hash, _)) = self.job.as_ref()
            {
                return Ok(*parent_hash)
            }

            let command = self.commands.recv().map_err(|_| ChannelDisconnected)?;
            self.apply(command);
        }
    }

    /// Ensures the transaction iterator matches `parent_hash`, opening a fresh one when the head
    /// changed. Returns `false` while the pool is not yet tracking that parent.
    fn open_transactions(&mut self, parent_hash: B256) -> bool {
        if self.transactions.as_ref().is_none_or(|(parent, _)| *parent != parent_hash) {
            // Release the stale iterator before asking the pool for a new one.
            self.transactions = None;
            self.transactions = self
                .source
                .best_transactions(parent_hash)
                .map(|transactions| (parent_hash, transactions));
        }
        self.transactions.is_some()
    }

    /// Speculatively executes pool transactions against the parent state for at most
    /// [`REFRESH_INTERVAL`], filling the cache with every state read.
    ///
    /// Stops early once the pool has no transaction ready or a command arrives. Commands are
    /// never consumed here: a pending command merely ends the batch and is applied by the main
    /// loop after the EVM and state provider built here are dropped.
    fn warm_one_batch(&mut self) -> BatchEnd {
        let (_, job) = self.job.as_ref().expect("wait_until_runnable installed a job");
        let (parent_hash, transactions) =
            self.transactions.as_mut().expect("open_transactions installed an iterator");

        // Building a state provider opens a database transaction; don't bother under a pending
        // command.
        if !self.commands.is_empty() {
            return BatchEnd::GoAgain
        }

        let state_provider = match job.provider_builder.build() {
            Ok(provider) => provider,
            Err(err) => {
                trace!(
                    target: "engine::tree::txpool_prewarm",
                    %err,
                    ?parent_hash,
                    "failed to build txpool prewarming state provider"
                );
                return BatchEnd::Rest
            }
        };
        let mut state = State::builder()
            .with_database(self.cache.as_db_mut(StateProviderDatabase::new(state_provider)))
            .build();
        // The environment is the head block's own, not a predicted next-block one, and execution
        // is out of context by design: transaction viability is the pool's business, so nonce,
        // balance and (one-block-stale) basefee checks must not gate which state gets warmed.
        let mut evm_env = job.evm_env.clone();
        evm_env.cfg_env.disable_nonce_check = true;
        evm_env.cfg_env.disable_balance_check = true;
        evm_env.cfg_env.disable_base_fee = true;
        let mut evm = self.evm_config.evm_with_env(&mut state, evm_env);

        let deadline = Instant::now() + REFRESH_INTERVAL;
        while self.commands.is_empty() && Instant::now() < deadline {
            let Some(transaction) = transactions.next() else { return BatchEnd::Rest };
            if let Err(err) = evm.transact(transaction.transaction) {
                trace!(
                    target: "engine::tree::txpool_prewarm",
                    %err,
                    tx_hash = ?transaction.hash,
                    sender = %transaction.sender,
                    "speculative txpool transaction execution failed"
                );
            }
        }
        BatchEnd::GoAgain
    }

    /// Publishes a fresh snapshot if the cache gained reads since the last publication.
    fn publish_snapshot_if_dirty(&mut self) {
        let entries = entry_counts(&self.cache);
        if entries == self.published_entries {
            return
        }

        let parent_hash = self.cache_parent.expect("reads only accumulate after a cache reset");
        *self.publication.write() = Some(Snapshot::new(parent_hash, Arc::new(self.cache.clone())));
        self.published_entries = entries;
        let (accounts, storage, bytecodes) = entries;
        debug!(
            target: "engine::tree::txpool_prewarm",
            ?parent_hash,
            accounts,
            storage,
            bytecodes,
            "published txpool prewarming snapshot"
        );
    }

    /// Rests until a command arrives (applying it) or `timeout` elapses, whichever comes first.
    fn idle(&mut self, timeout: Duration) -> Result<(), ChannelDisconnected> {
        match self.commands.recv_timeout(timeout) {
            Ok(command) => {
                self.apply(command);
                Ok(())
            }
            Err(RecvTimeoutError::Timeout) => Ok(()),
            Err(RecvTimeoutError::Disconnected) => Err(ChannelDisconnected),
        }
    }

    /// Applies every command already sitting in the channel, without blocking.
    fn apply_pending_commands(&mut self) -> Result<(), ChannelDisconnected> {
        loop {
            match self.commands.try_recv() {
                Ok(command) => self.apply(command),
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => return Err(ChannelDisconnected),
            }
        }
    }

    /// Applies a single command.
    ///
    /// Only called while no EVM or state provider is alive, so acknowledging a pause here
    /// guarantees the worker holds no execution resources.
    fn apply(&mut self, command: Command<Job<N, P, Evm>>) {
        match command {
            Command::Start { parent_hash, job } => self.job = Some((parent_hash, job)),
            Command::Pause(acknowledge) => {
                self.pauses =
                    self.pauses.checked_add(1).expect("txpool prewarm pause count overflow");
                let _ = acknowledge.send(());
            }
            Command::Resume => {
                self.pauses = self
                    .pauses
                    .checked_sub(1)
                    .expect("txpool prewarm resumed without a matching pause");
            }
        }
    }
}

/// What to do after a warming batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchEnd {
    /// Batch again right away: the refresh deadline passed or a command interrupted the batch
    /// while transactions may still be ready.
    GoAgain,
    /// Idle until something changes: the pool had no transaction ready, or no state provider
    /// could be built.
    Rest,
}

/// The control channel closed: every sender is dropped and the worker shuts down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChannelDisconnected;

/// Returns `(accounts, storage slots, bytecodes)` cached in `reads`.
fn entry_counts(reads: &CachedReads) -> (usize, usize, usize) {
    (
        reads.accounts.len(),
        reads.accounts.values().map(|account| account.storage.len()).sum(),
        reads.contracts.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::{super::Transaction as PoolTransaction, *};
    use crate::tree::StateProviderBuilder;
    use alloy_consensus::{transaction::Recovered, Signed, TxLegacy};
    use alloy_primitives::{Address, Signature, TxKind, U256};
    use crossbeam_channel::{bounded, unbounded, Sender};
    use parking_lot::{Mutex, RwLock};
    use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_provider::test_utils::MockEthProvider;
    use std::{
        collections::{HashMap, VecDeque},
        sync::atomic::{AtomicUsize, Ordering},
        thread::{self, JoinHandle},
    };

    /// Upper bound on any single wait; failures surface as panics well before CI timeouts.
    const WAIT_LIMIT: Duration = Duration::from_secs(5);
    const POLL_INTERVAL: Duration = Duration::from_millis(2);

    type TestJob = Job<EthPrimitives, MockEthProvider, EthEvmConfig>;

    /// Drives a live worker thread through its public seams only: commands in, the publication
    /// slot and the scripted pool out.
    struct Harness {
        commands: Sender<Command<TestJob>>,
        publication: Publication,
        pool: Arc<ScriptedPool>,
        worker: Option<JoinHandle<()>>,
    }

    impl Harness {
        fn spawn() -> Self {
            let (commands, receiver) = unbounded();
            let publication: Publication = Arc::new(RwLock::new(None));
            let pool = Arc::new(ScriptedPool::default());
            let worker = thread::spawn({
                let publication = Arc::clone(&publication);
                let source: Arc<dyn Source<EthPrimitives>> = pool.clone();
                move || Worker::new(receiver, publication, source, EthEvmConfig::mainnet()).run()
            });
            Self { commands, publication, pool, worker: Some(worker) }
        }

        /// Drops the control channel and waits for the worker thread to exit.
        fn shutdown(mut self) {
            let (disconnected, _) = unbounded();
            self.commands = disconnected;
            let worker = self.worker.take().expect("worker already joined");
            wait_until("the worker thread exits", || worker.is_finished());
            worker.join().unwrap();
        }

        /// Points the worker at `parent_hash`, as [`Handle::start`](super::super::Handle) does.
        fn start(&self, parent_hash: B256) {
            let job = Job {
                evm_env: Default::default(),
                provider_builder: StateProviderBuilder::new(
                    MockEthProvider::default(),
                    parent_hash,
                    None,
                ),
            };
            self.commands.send(Command::Start { parent_hash, job }).unwrap();
        }

        /// Pauses the worker and waits for the acknowledgement. Once this returns the worker is
        /// quiescent until [`Self::resume`].
        fn pause(&self) {
            let (acknowledge, acknowledged) = bounded(1);
            self.commands.send(Command::Pause(acknowledge)).unwrap();
            acknowledged.recv_timeout(WAIT_LIMIT).expect("pause was not acknowledged");
        }

        fn resume(&self) {
            self.commands.send(Command::Resume).unwrap();
        }

        /// Blocks until a published snapshot satisfies `accept`.
        fn published(&self, accept: impl Fn(&Snapshot) -> bool) -> Snapshot {
            let deadline = Instant::now() + WAIT_LIMIT;
            loop {
                let snapshot = self.publication.read().as_ref().cloned();
                if let Some(snapshot) = snapshot &&
                    accept(&snapshot)
                {
                    return snapshot
                }
                assert!(Instant::now() < deadline, "timed out waiting for a matching snapshot");
                thread::sleep(POLL_INTERVAL);
            }
        }

        fn published_for(&self, parent_hash: B256) -> Snapshot {
            self.published(|snapshot| snapshot.parent_hash() == parent_hash)
        }

        fn published_entry_counts(&self) -> Option<(usize, usize, usize)> {
            self.publication.read().as_ref().map(|snapshot| snapshot.entry_counts())
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            // Disconnect and reap the worker thread so it cannot outlive the test.
            let (disconnected, _) = unbounded();
            self.commands = disconnected;
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    /// A pool the tests script per parent: [`Self::push`] hands a transaction to the iterator
    /// opened for that parent, and unknown parents read as "not tracking this head yet".
    #[derive(Debug, Default)]
    struct ScriptedPool {
        queues: Arc<Mutex<HashMap<B256, VecDeque<PoolTransaction<EthPrimitives>>>>>,
        /// Iterators handed out; the worker is expected to open exactly one per parent.
        opened: AtomicUsize,
        /// Polls answered with "not tracking this head yet".
        not_ready: AtomicUsize,
    }

    impl ScriptedPool {
        fn push(&self, parent_hash: B256, transaction: PoolTransaction<EthPrimitives>) {
            self.queues.lock().entry(parent_hash).or_default().push_back(transaction);
        }
    }

    impl Source<EthPrimitives> for ScriptedPool {
        fn best_transactions(&self, parent_hash: B256) -> Option<Transactions<EthPrimitives>> {
            if !self.queues.lock().contains_key(&parent_hash) {
                self.not_ready.fetch_add(1, Ordering::Relaxed);
                return None
            }
            self.opened.fetch_add(1, Ordering::Relaxed);
            let queues = Arc::clone(&self.queues);
            Some(Box::new(std::iter::from_fn(move || {
                queues.lock().get_mut(&parent_hash)?.pop_front()
            })))
        }
    }

    /// A signed transfer to `recipient`. The signature is a dummy: the worker executes with the
    /// attached sender and disabled nonce/balance checks, so it is never recovered or validated.
    fn transfer(recipient: u8) -> PoolTransaction<EthPrimitives> {
        let transaction = TxLegacy {
            gas_limit: 21_000,
            to: TxKind::Call(Address::repeat_byte(recipient)),
            value: U256::from(1),
            ..Default::default()
        };
        let hash = B256::repeat_byte(recipient);
        let signed = TransactionSigned::Legacy(Signed::new_unchecked(
            transaction,
            Signature::test_signature(),
            hash,
        ));
        let sender = Address::repeat_byte(0xAA);
        PoolTransaction { hash, sender, transaction: Recovered::new_unchecked(signed, sender) }
    }

    fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + WAIT_LIMIT;
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            thread::sleep(POLL_INTERVAL);
        }
    }

    #[test]
    fn warms_pool_transactions_into_a_published_snapshot() {
        let harness = Harness::spawn();
        let parent_hash = B256::repeat_byte(0x01);

        // Start before the pool tracks the head, covering the poll-and-retry path.
        harness.start(parent_hash);
        wait_until("the untracked head is polled", || {
            harness.pool.not_ready.load(Ordering::Relaxed) >= 1
        });
        harness.pool.push(parent_hash, transfer(0xB0));

        let snapshot = harness.published_for(parent_hash);
        let (accounts, _, _) = snapshot.entry_counts();
        assert!(accounts >= 1, "speculative execution should cache account reads");
    }

    #[test]
    fn pause_quiesces_the_worker_until_resume() {
        let harness = Harness::spawn();
        let parent_hash = B256::repeat_byte(0x01);
        harness.start(parent_hash);
        harness.pool.push(parent_hash, transfer(0xB0));
        let before = harness.published_for(parent_hash).entry_counts();

        harness.pause();
        harness.pool.push(parent_hash, transfer(0xB1));
        thread::sleep(REFRESH_INTERVAL * 2);
        assert_eq!(
            harness.published_entry_counts(),
            Some(before),
            "a paused worker must not publish"
        );

        harness.resume();
        harness.published(|snapshot| snapshot.entry_counts() != before);
    }

    #[test]
    fn overlapping_pauses_require_matching_resumes() {
        let harness = Harness::spawn();
        let parent_hash = B256::repeat_byte(0x01);
        harness.start(parent_hash);
        harness.pool.push(parent_hash, transfer(0xB0));
        let before = harness.published_for(parent_hash).entry_counts();

        harness.pause();
        harness.pause();
        harness.pool.push(parent_hash, transfer(0xB1));
        harness.resume();
        thread::sleep(REFRESH_INTERVAL * 2);
        assert_eq!(
            harness.published_entry_counts(),
            Some(before),
            "one resume must not release two pauses"
        );

        harness.resume();
        harness.published(|snapshot| snapshot.entry_counts() != before);
    }

    #[test]
    fn reuses_the_iterator_per_head_and_reopens_on_switch() {
        let harness = Harness::spawn();
        let first = B256::repeat_byte(0x01);
        let second = B256::repeat_byte(0x02);

        harness.start(first);
        harness.pool.push(first, transfer(0xB0));
        let before = harness.published_for(first).entry_counts();
        harness.pool.push(first, transfer(0xB1));
        harness.published(|snapshot| snapshot.entry_counts() != before);
        assert_eq!(harness.pool.opened.load(Ordering::Relaxed), 1, "one iterator per head");

        harness.start(second);
        harness.pool.push(second, transfer(0xB2));
        harness.published_for(second);
        assert_eq!(harness.pool.opened.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn newest_start_wins() {
        let harness = Harness::spawn();
        let stale = B256::repeat_byte(0x01);
        let newest = B256::repeat_byte(0x02);

        harness.start(stale);
        harness.start(newest);
        harness.pool.push(newest, transfer(0xB0));

        harness.published_for(newest);
    }

    #[test]
    fn shuts_down_when_control_is_dropped() {
        let harness = Harness::spawn();
        let parent_hash = B256::repeat_byte(0x01);
        harness.start(parent_hash);
        harness.pool.push(parent_hash, transfer(0xB0));
        harness.published_for(parent_hash);

        harness.shutdown();
    }
}
