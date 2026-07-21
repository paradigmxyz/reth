use super::{
    cache::Cache,
    control::{Command, Publication},
    Job, Source, Transactions,
};
use crate::tree::StateProviderDatabase;
use alloy_evm::Evm;
use alloy_primitives::B256;
use crossbeam_channel::{Receiver, RecvTimeoutError, TryRecvError};
use reth_evm::ConfigureEvm;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{BlockReader, StateProviderFactory, StateReader};
use reth_revm::db::State;
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
/// current canonical state, recording every state read in a reusable [`Cache`]. Roughly every
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
    /// Read-through cache filled by execution. Retained across heads so map capacity is reused;
    /// [`Cache::reset`] re-keys it when the parent changes.
    cache: Cache,
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
            cache: Cache::default(),
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

            if self.cache.parent_hash() != Some(parent_hash) {
                self.cache.reset(parent_hash);
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
        let state_provider = StateProviderDatabase::new(self.cache.state_provider(state_provider));
        let mut state = State::builder().with_database(state_provider).build();
        let mut evm_env = job.evm_env.clone();
        evm_env.cfg_env.disable_nonce_check = true;
        evm_env.cfg_env.disable_balance_check = true;
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
    fn publish_snapshot_if_dirty(&self) {
        if !self.cache.is_dirty() {
            return
        }

        let snapshot = self.cache.snapshot();
        let parent_hash = snapshot.parent_hash();
        let (accounts, storage, bytecodes) = snapshot.entry_counts();
        *self.publication.write() = Some(snapshot);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::StateProviderBuilder;
    use alloy_primitives::Address;
    use crossbeam_channel::{bounded, unbounded, Sender};
    use parking_lot::RwLock;
    use reth_ethereum_primitives::EthPrimitives;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_provider::test_utils::MockEthProvider;
    use reth_revm::database::EvmStateProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type TestJob = Job<EthPrimitives, MockEthProvider, EthEvmConfig>;
    type TestWorker = Worker<EthPrimitives, MockEthProvider, EthEvmConfig>;

    /// A pool stub that counts how often an iterator is opened.
    #[derive(Debug, Default)]
    struct PoolStub(AtomicUsize);

    impl Source<EthPrimitives> for PoolStub {
        fn best_transactions(&self, _parent_hash: B256) -> Option<Transactions<EthPrimitives>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Some(Box::new(std::iter::empty()))
        }
    }

    fn worker() -> (Sender<Command<TestJob>>, Arc<PoolStub>, TestWorker) {
        let (sender, commands) = unbounded();
        let source = Arc::new(PoolStub::default());
        let worker = Worker::new(
            commands,
            Arc::new(RwLock::new(None)),
            source.clone(),
            EthEvmConfig::mainnet(),
        );
        (sender, source, worker)
    }

    fn start(parent_hash: B256) -> Command<TestJob> {
        Command::Start {
            parent_hash,
            job: Job {
                evm_env: Default::default(),
                provider_builder: StateProviderBuilder::new(
                    MockEthProvider::default(),
                    parent_hash,
                    None,
                ),
            },
        }
    }

    fn current_parent(worker: &TestWorker) -> Option<B256> {
        worker.job.as_ref().map(|(parent_hash, _)| *parent_hash)
    }

    #[test]
    fn newest_job_wins() {
        let (sender, _pool, mut worker) = worker();
        sender.send(start(B256::repeat_byte(0x01))).unwrap();
        sender.send(start(B256::repeat_byte(0x02))).unwrap();

        worker.apply_pending_commands().unwrap();
        assert_eq!(current_parent(&worker), Some(B256::repeat_byte(0x02)));
    }

    #[test]
    fn overlapping_pauses_resume_after_last_command() {
        let (sender, _pool, mut worker) = worker();
        let (first_ack, first_acknowledged) = bounded(1);
        let (second_ack, second_acknowledged) = bounded(1);
        sender.send(Command::Pause(first_ack)).unwrap();
        sender.send(Command::Pause(second_ack)).unwrap();

        worker.apply_pending_commands().unwrap();
        first_acknowledged.recv().unwrap();
        second_acknowledged.recv().unwrap();
        assert_eq!(worker.pauses, 2);

        sender.send(Command::Resume).unwrap();
        worker.apply_pending_commands().unwrap();
        assert_eq!(worker.pauses, 1);

        sender.send(Command::Resume).unwrap();
        worker.apply_pending_commands().unwrap();
        assert_eq!(worker.pauses, 0);
    }

    #[test]
    fn start_while_paused_replaces_pending_job() {
        let (sender, _pool, mut worker) = worker();
        let (acknowledge, acknowledged) = bounded(1);
        sender.send(Command::Pause(acknowledge)).unwrap();
        sender.send(start(B256::repeat_byte(0x01))).unwrap();
        sender.send(start(B256::repeat_byte(0x02))).unwrap();

        worker.apply_pending_commands().unwrap();
        acknowledged.recv().unwrap();
        assert_eq!(current_parent(&worker), Some(B256::repeat_byte(0x02)));
        assert_eq!(worker.pauses, 1);
    }

    #[test]
    fn runnable_wait_returns_current_parent() {
        let (sender, _pool, mut worker) = worker();
        let parent_hash = B256::repeat_byte(0x01);
        sender.send(start(parent_hash)).unwrap();

        assert_eq!(worker.wait_until_runnable(), Ok(parent_hash));
    }

    #[test]
    fn transactions_iterator_is_reused_per_parent() {
        let (_sender, pool, mut worker) = worker();
        let first = B256::repeat_byte(0x01);
        let second = B256::repeat_byte(0x02);

        assert!(worker.open_transactions(first));
        assert!(worker.open_transactions(first));
        assert_eq!(pool.0.load(Ordering::Relaxed), 1);

        assert!(worker.open_transactions(second));
        assert_eq!(pool.0.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn publishes_only_when_cache_gained_reads() {
        let (_sender, _pool, mut worker) = worker();
        let parent_hash = B256::repeat_byte(0x01);
        worker.cache.reset(parent_hash);

        worker.publish_snapshot_if_dirty();
        assert!(worker.publication.read().is_none());

        // A read-through miss is what marks the cache dirty.
        let state_provider = MockEthProvider::default().latest().unwrap();
        worker.cache.state_provider(state_provider).basic_account(&Address::ZERO).unwrap();

        worker.publish_snapshot_if_dirty();
        assert_eq!(
            worker.publication.read().as_ref().map(|snapshot| snapshot.parent_hash()),
            Some(parent_hash)
        );
        assert!(!worker.cache.is_dirty());
    }

    #[test]
    fn disconnected_channel_is_explicit() {
        let (sender, _pool, mut worker) = worker();
        drop(sender);

        assert!(matches!(worker.apply_pending_commands(), Err(ChannelDisconnected)));
    }
}
