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

pub(super) fn run<N, P, Evm>(
    commands: Receiver<Command<Job<N, P, Evm>>>,
    publication: Publication,
    source: Arc<dyn Source<N>>,
    evm_config: Evm,
) where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    // Dropping the control sender is the worker's normal shutdown signal.
    let _ = run_until_disconnected(commands, publication, source, evm_config);
}

fn run_until_disconnected<N, P, Evm>(
    commands: Receiver<Command<Job<N, P, Evm>>>,
    publication: Publication,
    source: Arc<dyn Source<N>>,
    evm_config: Evm,
) -> Result<(), ChannelDisconnected>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    let mut state = WorkerState::new();

    loop {
        drain_commands(&commands, &mut state)?;
        let parent_hash = wait_for_runnable_job(&commands, &mut state)?;

        if state.best_transactions_parent_hash != Some(parent_hash) {
            let Some(opened) = source.best_transactions(parent_hash) else {
                wait_for_command(&commands, &mut state, HEAD_POLL_INTERVAL)?;
                continue
            };
            state.best_transactions = Some(opened);
            state.best_transactions_parent_hash = Some(parent_hash);
        }

        if state.cache.parent_hash() != Some(parent_hash) {
            state.cache.reset(parent_hash);
            state.cache_dirty = false;
            debug!(
                target: "engine::tree::txpool_prewarm",
                ?parent_hash,
                "started txpool prewarming"
            );
        }

        // `wait_for_runnable_job` returned this job's parent hash, and the branch above either
        // retained or opened its matching transaction iterator. No command has been processed
        // since, so both are still installed here.
        let (_, job) = state.current_job.as_ref().unwrap();
        let transactions = state.best_transactions.as_mut().unwrap();
        let outcome =
            prewarm_batch(&evm_config, parent_hash, job, &state.cache, transactions, &commands)?;

        match outcome {
            BatchOutcome::Completed { executed, end } => {
                state.cache_dirty |= executed != 0;
                match publish_if_dirty(&commands, &publication, &mut state)? {
                    PublishOutcome::Interrupted => continue,
                    PublishOutcome::Ready => {}
                }

                if end == BatchEnd::Empty {
                    wait_for_command(&commands, &mut state, REFRESH_INTERVAL)?;
                }
            }
            BatchOutcome::Interrupted { executed, command } => {
                state.cache_dirty |= executed != 0;
                state.apply_command(command);
            }
            BatchOutcome::ProviderUnavailable => {
                match publish_if_dirty(&commands, &publication, &mut state)? {
                    PublishOutcome::Interrupted => continue,
                    PublishOutcome::Ready => {}
                }
                wait_for_command(&commands, &mut state, REFRESH_INTERVAL)?;
            }
        }
    }
}

fn prewarm_batch<N, P, Evm>(
    evm_config: &Evm,
    parent_hash: B256,
    job: &Job<N, P, Evm>,
    cache: &Cache,
    transactions: &mut Transactions<N>,
    commands: &Receiver<Command<Job<N, P, Evm>>>,
) -> Result<BatchOutcome<Job<N, P, Evm>>, ChannelDisconnected>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone,
    Evm: ConfigureEvm<Primitives = N>,
{
    match commands.try_recv() {
        Ok(command) => return Ok(BatchOutcome::Interrupted { executed: 0, command }),
        Err(TryRecvError::Disconnected) => return Err(ChannelDisconnected),
        Err(TryRecvError::Empty) => {}
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
            return Ok(BatchOutcome::ProviderUnavailable)
        }
    };
    let state_provider = cache.state_provider(state_provider);
    let state_provider = StateProviderDatabase::new(state_provider);
    let mut state = State::builder().with_database(state_provider).build();
    let mut evm_env = job.evm_env.clone();
    evm_env.cfg_env.disable_nonce_check = true;
    evm_env.cfg_env.disable_balance_check = true;
    let mut evm = evm_config.evm_with_env(&mut state, evm_env);

    let deadline = Instant::now() + REFRESH_INTERVAL;
    let mut executed = 0;
    loop {
        match commands.try_recv() {
            Ok(command) => return Ok(BatchOutcome::Interrupted { executed, command }),
            Err(TryRecvError::Disconnected) => return Err(ChannelDisconnected),
            Err(TryRecvError::Empty) => {}
        }

        if Instant::now() >= deadline {
            return Ok(BatchOutcome::Completed { executed, end: BatchEnd::Deadline })
        }

        let Some(transaction) = transactions.next() else {
            return Ok(BatchOutcome::Completed { executed, end: BatchEnd::Empty })
        };
        if let Err(err) = evm.transact(transaction.transaction) {
            trace!(
                target: "engine::tree::txpool_prewarm",
                %err,
                tx_hash = ?transaction.hash,
                sender = %transaction.sender,
                "speculative txpool transaction execution failed"
            );
        }
        executed += 1;
    }
}

fn wait_for_runnable_job<J, N>(
    commands: &Receiver<Command<J>>,
    state: &mut WorkerState<J, N>,
) -> Result<B256, ChannelDisconnected>
where
    N: NodePrimitives,
{
    loop {
        if state.pause_count == 0 &&
            let Some((parent_hash, _)) = state.current_job.as_ref()
        {
            return Ok(*parent_hash)
        }

        let command = commands.recv().map_err(|_| ChannelDisconnected)?;
        state.apply_command(command);
        drain_commands(commands, state)?;
    }
}

fn wait_for_command<J, N>(
    commands: &Receiver<Command<J>>,
    state: &mut WorkerState<J, N>,
    timeout: Duration,
) -> Result<(), ChannelDisconnected>
where
    N: NodePrimitives,
{
    match commands.recv_timeout(timeout) {
        Ok(command) => {
            state.apply_command(command);
            drain_commands(commands, state)?;
            Ok(())
        }
        Err(RecvTimeoutError::Timeout) => Ok(()),
        Err(RecvTimeoutError::Disconnected) => Err(ChannelDisconnected),
    }
}

fn drain_commands<J, N>(
    commands: &Receiver<Command<J>>,
    state: &mut WorkerState<J, N>,
) -> Result<DrainOutcome, ChannelDisconnected>
where
    N: NodePrimitives,
{
    let mut result = DrainOutcome::default();
    loop {
        match commands.try_recv() {
            Ok(command) => {
                if state.apply_command(command) == DrainOutcome::Interrupted {
                    result = DrainOutcome::Interrupted;
                }
            }
            Err(TryRecvError::Empty) => return Ok(result),
            Err(TryRecvError::Disconnected) => return Err(ChannelDisconnected),
        }
    }
}

fn publish_if_dirty<J, N>(
    commands: &Receiver<Command<J>>,
    publication: &Publication,
    state: &mut WorkerState<J, N>,
) -> Result<PublishOutcome, ChannelDisconnected>
where
    N: NodePrimitives,
{
    if drain_commands(commands, state)? == DrainOutcome::Interrupted {
        return Ok(PublishOutcome::Interrupted)
    }
    if !state.cache_dirty {
        return Ok(PublishOutcome::Ready)
    }

    let snapshot = state.cache.snapshot();
    if drain_commands(commands, state)? == DrainOutcome::Interrupted {
        return Ok(PublishOutcome::Interrupted)
    }

    let parent_hash = snapshot.parent_hash();
    let (accounts, storage, bytecodes) = snapshot.entry_counts();
    *publication.write() = Some(snapshot);
    state.cache_dirty = false;
    debug!(
        target: "engine::tree::txpool_prewarm",
        ?parent_hash,
        accounts,
        storage,
        bytecodes,
        "published txpool prewarming snapshot"
    );
    Ok(PublishOutcome::Ready)
}

struct WorkerState<J, N>
where
    N: NodePrimitives,
{
    current_job: Option<(B256, J)>,
    best_transactions: Option<Transactions<N>>,
    best_transactions_parent_hash: Option<B256>,
    pause_count: u64,
    cache: Cache,
    /// Whether the cache contains reads that have not yet been published.
    cache_dirty: bool,
}

impl<J, N> WorkerState<J, N>
where
    N: NodePrimitives,
{
    fn new() -> Self {
        Self {
            current_job: None,
            best_transactions: None,
            best_transactions_parent_hash: None,
            pause_count: 0,
            cache: Cache::default(),
            cache_dirty: false,
        }
    }

    /// Applies `command` while no EVM or state provider is active.
    ///
    /// Returns whether the command interrupts a batch being considered for publication.
    fn apply_command(&mut self, command: Command<J>) -> DrainOutcome {
        match command {
            Command::Start { parent_hash, job } => {
                if self.best_transactions_parent_hash != Some(parent_hash) {
                    self.best_transactions = None;
                    self.best_transactions_parent_hash = None;
                }
                self.current_job = Some((parent_hash, job));
                DrainOutcome::Interrupted
            }
            Command::Pause(acknowledge) => {
                self.pause_count =
                    self.pause_count.checked_add(1).expect("txpool prewarm pause count overflow");
                let _ = acknowledge.send(());
                DrainOutcome::Interrupted
            }
            Command::Resume => {
                self.pause_count = self
                    .pause_count
                    .checked_sub(1)
                    .expect("txpool prewarm resumed without a matching pause");
                DrainOutcome::Ready
            }
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum DrainOutcome {
    #[default]
    Ready,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChannelDisconnected;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchEnd {
    Deadline,
    Empty,
}

enum BatchOutcome<J> {
    Completed { executed: usize, end: BatchEnd },
    Interrupted { executed: usize, command: Command<J> },
    ProviderUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublishOutcome {
    Ready,
    Interrupted,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::{bounded, unbounded};
    use parking_lot::RwLock;
    use reth_ethereum_primitives::EthPrimitives;

    type TestJob = u64;

    fn state() -> WorkerState<TestJob, EthPrimitives> {
        WorkerState::new()
    }

    fn publication() -> Publication {
        Arc::new(RwLock::new(None))
    }

    #[test]
    fn newest_job_wins() {
        let (sender, receiver) = unbounded();
        let mut state = state();
        let first = B256::repeat_byte(0x01);
        let second = B256::repeat_byte(0x02);
        sender.send(Command::Start { parent_hash: first, job: 1 }).unwrap();
        sender.send(Command::Start { parent_hash: second, job: 2 }).unwrap();

        let drained = drain_commands(&receiver, &mut state).unwrap();
        assert_eq!(drained, DrainOutcome::Interrupted);
        assert_eq!(state.current_job, Some((second, 2)));
    }

    #[test]
    fn overlapping_pauses_resume_after_last_command() {
        let (sender, receiver) = unbounded();
        let mut state = state();
        let (first_ack, first_acknowledged) = bounded(1);
        let (second_ack, second_acknowledged) = bounded(1);
        sender.send(Command::Pause(first_ack)).unwrap();
        sender.send(Command::Pause(second_ack)).unwrap();

        drain_commands(&receiver, &mut state).unwrap();
        first_acknowledged.recv().unwrap();
        second_acknowledged.recv().unwrap();
        assert_eq!(state.pause_count, 2);

        sender.send(Command::Resume).unwrap();
        drain_commands(&receiver, &mut state).unwrap();
        assert_eq!(state.pause_count, 1);

        sender.send(Command::Resume).unwrap();
        drain_commands(&receiver, &mut state).unwrap();
        assert_eq!(state.pause_count, 0);
    }

    #[test]
    fn start_while_paused_replaces_pending_job() {
        let (sender, receiver) = unbounded();
        let mut state = state();
        let (acknowledge, acknowledged) = bounded(1);
        sender.send(Command::Pause(acknowledge)).unwrap();
        sender.send(Command::Start { parent_hash: B256::repeat_byte(0x01), job: 1 }).unwrap();
        sender.send(Command::Start { parent_hash: B256::repeat_byte(0x02), job: 2 }).unwrap();

        drain_commands(&receiver, &mut state).unwrap();
        acknowledged.recv().unwrap();
        assert_eq!(state.current_job, Some((B256::repeat_byte(0x02), 2)));
        assert_eq!(state.pause_count, 1);
    }

    #[test]
    fn runnable_wait_returns_current_parent() {
        let (sender, receiver) = unbounded();
        let mut state = state();
        let parent_hash = B256::repeat_byte(0x01);
        sender.send(Command::Start { parent_hash, job: 1 }).unwrap();

        assert_eq!(wait_for_runnable_job(&receiver, &mut state), Ok(parent_hash));
    }

    #[test]
    fn dirty_cache_is_published_without_new_transactions() {
        let (_sender, receiver) = unbounded();
        let publication = publication();
        let mut state = state();
        let parent_hash = B256::repeat_byte(0x01);
        state.cache.reset(parent_hash);
        state.cache_dirty = true;

        assert_eq!(
            publish_if_dirty(&receiver, &publication, &mut state),
            Ok(PublishOutcome::Ready)
        );
        assert!(!state.cache_dirty);
        assert_eq!(
            publication.read().as_ref().map(|snapshot| snapshot.parent_hash()),
            Some(parent_hash)
        );
    }

    #[test]
    fn new_job_suppresses_dirty_publication() {
        let (sender, receiver) = unbounded();
        let publication = publication();
        let mut state = state();
        let parent_hash = B256::repeat_byte(0x01);
        state.cache.reset(parent_hash);
        state.cache_dirty = true;
        sender.send(Command::Start { parent_hash: B256::repeat_byte(0x02), job: 2 }).unwrap();

        assert_eq!(
            publish_if_dirty(&receiver, &publication, &mut state),
            Ok(PublishOutcome::Interrupted)
        );
        assert!(state.cache_dirty);
        assert!(publication.read().is_none());
    }

    #[test]
    fn disconnected_channel_is_explicit() {
        let (sender, receiver) = unbounded();
        let mut state = state();
        drop(sender);

        assert!(matches!(drain_commands(&receiver, &mut state), Err(ChannelDisconnected)));
    }
}
