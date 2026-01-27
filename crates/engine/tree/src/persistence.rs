use crate::metrics::PersistenceMetrics;
use alloy_consensus::constants::MGAS_TO_GAS;
use alloy_eips::BlockNumHash;
use crossbeam_channel::Sender as CrossbeamSender;
use reth_chain_state::{ExecutedBlock, ExecutionTimingStats};
use reth_errors::ProviderError;
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::ProviderNodeTypes, BlockExecutionWriter, BlockHashReader, ChainStateBlockWriter,
    DBProvider, DatabaseProviderFactory, ProviderFactory, SaveBlocksMode,
};
use reth_prune::{PrunerError, PrunerOutput, PrunerWithFactory};
use reth_stages_api::{MetricEvent, MetricEventsSender};
use std::{
    sync::mpsc::{Receiver, SendError, Sender},
    time::{Duration, Instant},
};
use thiserror::Error;
use tracing::{debug, error, warn};

/// Writes parts of reth's in memory tree state to the database and static files.
///
/// This is meant to be a spawned service that listens for various incoming persistence operations,
/// performing those actions on disk, and returning the result in a channel.
///
/// This should be spawned in its own thread with [`std::thread::spawn`], since this performs
/// blocking I/O operations in an endless loop.
#[derive(Debug)]
pub struct PersistenceService<N>
where
    N: ProviderNodeTypes,
{
    /// The provider factory to use
    provider: ProviderFactory<N>,
    /// Incoming requests
    incoming: Receiver<PersistenceAction<N::Primitives>>,
    /// The pruner
    pruner: PrunerWithFactory<ProviderFactory<N>>,
    /// metrics
    metrics: PersistenceMetrics,
    /// Sender for sync metrics - we only submit sync metrics for persisted blocks
    sync_metrics_tx: MetricEventsSender,
    /// Optional slow block threshold. When set, blocks exceeding this threshold
    /// will be logged with detailed metrics.
    slow_block_threshold: Option<Duration>,
}

impl<N> PersistenceService<N>
where
    N: ProviderNodeTypes,
{
    /// Create a new persistence service
    pub fn new(
        provider: ProviderFactory<N>,
        incoming: Receiver<PersistenceAction<N::Primitives>>,
        pruner: PrunerWithFactory<ProviderFactory<N>>,
        sync_metrics_tx: MetricEventsSender,
        slow_block_threshold: Option<Duration>,
    ) -> Self {
        Self {
            provider,
            incoming,
            pruner,
            metrics: PersistenceMetrics::default(),
            sync_metrics_tx,
            slow_block_threshold,
        }
    }

    /// Prunes block data before the given block number according to the configured prune
    /// configuration.
    fn prune_before(&mut self, block_num: u64) -> Result<PrunerOutput, PrunerError> {
        debug!(target: "engine::persistence", ?block_num, "Running pruner");
        let start_time = Instant::now();
        // TODO: doing this properly depends on pruner segment changes
        let result = self.pruner.run(block_num);
        self.metrics.prune_before_duration_seconds.record(start_time.elapsed());
        result
    }
}

impl<N> PersistenceService<N>
where
    N: ProviderNodeTypes,
{
    /// This is the main loop, that will listen to database events and perform the requested
    /// database actions
    pub fn run(mut self) -> Result<(), PersistenceError> {
        // If the receiver errors then senders have disconnected, so the loop should then end.
        while let Ok(action) = self.incoming.recv() {
            match action {
                PersistenceAction::RemoveBlocksAbove(new_tip_num, sender) => {
                    let result = self.on_remove_blocks_above(new_tip_num)?;
                    // send new sync metrics based on removed blocks
                    let _ =
                        self.sync_metrics_tx.send(MetricEvent::SyncHeight { height: new_tip_num });
                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(result);
                }
                PersistenceAction::SaveBlocks(blocks, sender) => {
                    let result = self.on_save_blocks(blocks)?;
                    let result_number = result.map(|r| r.number);

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(result);

                    if let Some(block_number) = result_number {
                        // send new sync metrics based on saved blocks
                        let _ = self
                            .sync_metrics_tx
                            .send(MetricEvent::SyncHeight { height: block_number });

                        if self.pruner.is_pruning_needed(block_number) {
                            // We log `PrunerOutput` inside the `Pruner`
                            let _ = self.prune_before(block_number)?;
                        }
                    }
                }
                PersistenceAction::SaveFinalizedBlock(finalized_block) => {
                    let provider = self.provider.database_provider_rw()?;
                    provider.save_finalized_block_number(finalized_block)?;
                    provider.commit()?;
                }
                PersistenceAction::SaveSafeBlock(safe_block) => {
                    let provider = self.provider.database_provider_rw()?;
                    provider.save_safe_block_number(safe_block)?;
                    provider.commit()?;
                }
            }
        }
        Ok(())
    }

    fn on_remove_blocks_above(
        &self,
        new_tip_num: u64,
    ) -> Result<Option<BlockNumHash>, PersistenceError> {
        debug!(target: "engine::persistence", ?new_tip_num, "Removing blocks");
        let start_time = Instant::now();
        let provider_rw = self.provider.database_provider_rw()?;

        let new_tip_hash = provider_rw.block_hash(new_tip_num)?;
        provider_rw.remove_block_and_execution_above(new_tip_num)?;
        provider_rw.commit()?;

        debug!(target: "engine::persistence", ?new_tip_num, ?new_tip_hash, "Removed blocks from disk");
        self.metrics.remove_blocks_above_duration_seconds.record(start_time.elapsed());
        Ok(new_tip_hash.map(|hash| BlockNumHash { hash, number: new_tip_num }))
    }

    fn on_save_blocks(
        &self,
        blocks: Vec<ExecutedBlock<N::Primitives>>,
    ) -> Result<Option<BlockNumHash>, PersistenceError> {
        let Some(first_block) = blocks.first().map(|b| b.recovered_block.num_hash()) else {
            return Ok(None)
        };
        let Some(last_block) = blocks.last().map(|b| b.recovered_block.num_hash()) else {
            return Ok(None)
        };
        let block_count = blocks.len();
        debug!(target: "engine::persistence", ?block_count, first=?first_block, last=?last_block, "Saving range of blocks");

        // Extract timing stats before consuming blocks (for slow block logging after commit)
        let timing_stats: Vec<ExecutionTimingStats> = if self.slow_block_threshold.is_some() {
            blocks.iter().filter_map(|b| b.timing_stats()).collect()
        } else {
            Vec::new()
        };

        let total_commit_duration = {
            let start = Instant::now();
            let provider_rw = self.provider.database_provider_rw()?;

            provider_rw.save_blocks(blocks, SaveBlocksMode::Full)?;
            provider_rw.commit()?;
            start.elapsed()
        };
        // We calculate an estimated commit duration per block by diving the total commit duration
        // for a batch by the number of blocks in the batch
        let commit_duration = total_commit_duration.div_f64(timing_stats.len() as f64);

        debug!(target: "engine::persistence", first=?first_block, last=?last_block, "Saved range of blocks");

        self.metrics.save_blocks_block_count.record(block_count as f64);
        self.metrics.save_blocks_duration_seconds.record(total_commit_duration);

        // Emit unified slow block logs after commit completes
        for stats in timing_stats {
            let total_duration = stats.execution_duration +
                stats.state_read_duration +
                stats.state_hash_duration +
                commit_duration;

            // Check if total time exceeds threshold
            if let Some(threshold) = self.slow_block_threshold &&
                total_duration > threshold
            {
                self.log_slow_block(stats, commit_duration, total_duration);
            }
        }

        Ok(Some(last_block))
    }

    /// Logs slow block execution in JSON format for cross-client performance analysis.
    ///
    /// This method outputs a standardized log entry when block execution
    /// exceeds the slow block threshold, including all timing phases.
    fn log_slow_block(&self, stats: ExecutionTimingStats, commit: Duration, total: Duration) {
        /// Calculates the cache hit rate as a percentage (0-100).
        fn calculate_hit_rate(hits: usize, misses: usize) -> f64 {
            let total = hits + misses;
            if total > 0 {
                (hits as f64 / total as f64) * 100.0
            } else {
                0.0
            }
        }

        let mgas_per_sec = (stats.gas_used as f64 / MGAS_TO_GAS as f64) /
            (stats.execution_duration.as_secs_f64() + stats.state_hash_duration.as_secs_f64());

        // Calculate cache hit rates
        let account_hit_rate =
            calculate_hit_rate(stats.account_cache_hits, stats.account_cache_misses);
        let storage_hit_rate =
            calculate_hit_rate(stats.storage_cache_hits, stats.storage_cache_misses);
        let code_hit_rate = calculate_hit_rate(stats.code_cache_hits, stats.code_cache_misses);

        warn!(
            target: "reth::slow_block",
            message = "Slow block",
            block.number = stats.block_number,
            block.hash = ?stats.block_hash,
            block.gas_used = stats.gas_used,
            block.tx_count = stats.tx_count,
            timing.execution_ms = stats.execution_duration.as_millis(),
            timing.state_read_ms = stats.state_read_duration.as_millis(),
            timing.state_hash_ms = stats.state_hash_duration.as_millis(),
            timing.commit_ms = commit.as_millis(),
            timing.total_ms = total.as_millis(),
            throughput.mgas_per_sec = format!("{:.2}", mgas_per_sec),
            state_reads.accounts = stats.accounts_read,
            state_reads.storage_slots = stats.storage_read,
            state_reads.code = stats.code_read,
            state_reads.code_bytes = stats.code_bytes_read,
            state_writes.accounts = stats.accounts_changed,
            state_writes.accounts_deleted = stats.accounts_deleted,
            state_writes.storage_slots = stats.storage_slots_changed,
            state_writes.storage_slots_deleted = stats.storage_slots_deleted,
            state_writes.code = stats.bytecodes_changed,
            state_writes.code_bytes = stats.code_bytes_written,
            state_writes.eip7702_delegations_set = stats.eip7702_delegations_set,
            state_writes.eip7702_delegations_cleared = stats.eip7702_delegations_cleared,
            cache.account.hits = stats.account_cache_hits,
            cache.account.misses = stats.account_cache_misses,
            cache.account.hit_rate = format!("{:.2}", account_hit_rate),
            cache.storage.hits = stats.storage_cache_hits,
            cache.storage.misses = stats.storage_cache_misses,
            cache.storage.hit_rate = format!("{:.2}", storage_hit_rate),
            cache.code.hits = stats.code_cache_hits,
            cache.code.misses = stats.code_cache_misses,
            cache.code.hit_rate = format!("{:.2}", code_hit_rate),
        );
    }
}

/// One of the errors that can happen when using the persistence service.
#[derive(Debug, Error)]
pub enum PersistenceError {
    /// A pruner error
    #[error(transparent)]
    PrunerError(#[from] PrunerError),

    /// A provider error
    #[error(transparent)]
    ProviderError(#[from] ProviderError),
}

/// A signal to the persistence service that part of the tree state can be persisted.
#[derive(Debug)]
pub enum PersistenceAction<N: NodePrimitives = EthPrimitives> {
    /// The section of tree state that should be persisted. These blocks are expected in order of
    /// increasing block number.
    ///
    /// First, header, transaction, and receipt-related data should be written to static files.
    /// Then the execution history-related data will be written to the database.
    SaveBlocks(Vec<ExecutedBlock<N>>, CrossbeamSender<Option<BlockNumHash>>),

    /// Removes block data above the given block number from the database.
    ///
    /// This will first update checkpoints from the database, then remove actual block data from
    /// static files.
    RemoveBlocksAbove(u64, CrossbeamSender<Option<BlockNumHash>>),

    /// Update the persisted finalized block on disk
    SaveFinalizedBlock(u64),

    /// Update the persisted safe block on disk
    SaveSafeBlock(u64),
}

/// A handle to the persistence service
#[derive(Debug, Clone)]
pub struct PersistenceHandle<N: NodePrimitives = EthPrimitives> {
    /// The channel used to communicate with the persistence service
    sender: Sender<PersistenceAction<N>>,
}

impl<T: NodePrimitives> PersistenceHandle<T> {
    /// Create a new [`PersistenceHandle`] from a [`Sender<PersistenceAction>`].
    pub const fn new(sender: Sender<PersistenceAction<T>>) -> Self {
        Self { sender }
    }

    /// Create a new [`PersistenceHandle`], and spawn the persistence service.
    pub fn spawn_service<N>(
        provider_factory: ProviderFactory<N>,
        pruner: PrunerWithFactory<ProviderFactory<N>>,
        sync_metrics_tx: MetricEventsSender,
        slow_block_threshold: Option<Duration>,
    ) -> PersistenceHandle<N::Primitives>
    where
        N: ProviderNodeTypes,
    {
        // create the initial channels
        let (db_service_tx, db_service_rx) = std::sync::mpsc::channel();

        // construct persistence handle
        let persistence_handle = PersistenceHandle::new(db_service_tx);

        // spawn the persistence service
        let db_service = PersistenceService::new(
            provider_factory,
            db_service_rx,
            pruner,
            sync_metrics_tx,
            slow_block_threshold,
        );
        std::thread::Builder::new()
            .name("Persistence Service".to_string())
            .spawn(|| {
                if let Err(err) = db_service.run() {
                    error!(target: "engine::persistence", ?err, "Persistence service failed");
                }
            })
            .unwrap();

        persistence_handle
    }

    /// Sends a specific [`PersistenceAction`] in the contained channel. The caller is responsible
    /// for creating any channels for the given action.
    pub fn send_action(
        &self,
        action: PersistenceAction<T>,
    ) -> Result<(), SendError<PersistenceAction<T>>> {
        self.sender.send(action)
    }

    /// Tells the persistence service to save a certain list of finalized blocks. The blocks are
    /// assumed to be ordered by block number.
    ///
    /// This returns the latest hash that has been saved, allowing removal of that block and any
    /// previous blocks from in-memory data structures. This value is returned in the receiver end
    /// of the sender argument.
    ///
    /// If there are no blocks to persist, then `None` is sent in the sender.
    pub fn save_blocks(
        &self,
        blocks: Vec<ExecutedBlock<T>>,
        tx: CrossbeamSender<Option<BlockNumHash>>,
    ) -> Result<(), SendError<PersistenceAction<T>>> {
        self.send_action(PersistenceAction::SaveBlocks(blocks, tx))
    }

    /// Persists the finalized block number on disk.
    pub fn save_finalized_block_number(
        &self,
        finalized_block: u64,
    ) -> Result<(), SendError<PersistenceAction<T>>> {
        self.send_action(PersistenceAction::SaveFinalizedBlock(finalized_block))
    }

    /// Persists the safe block number on disk.
    pub fn save_safe_block_number(
        &self,
        safe_block: u64,
    ) -> Result<(), SendError<PersistenceAction<T>>> {
        self.send_action(PersistenceAction::SaveSafeBlock(safe_block))
    }

    /// Tells the persistence service to remove blocks above a certain block number. The removed
    /// blocks are returned by the service.
    ///
    /// When the operation completes, the new tip hash is returned in the receiver end of the sender
    /// argument.
    pub fn remove_blocks_above(
        &self,
        block_num: u64,
        tx: CrossbeamSender<Option<BlockNumHash>>,
    ) -> Result<(), SendError<PersistenceAction<T>>> {
        self.send_action(PersistenceAction::RemoveBlocksAbove(block_num, tx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_chain_state::test_utils::TestBlockBuilder;
    use reth_exex_types::FinishedExExHeight;
    use reth_provider::test_utils::create_test_provider_factory;
    use reth_prune::Pruner;
    use tokio::sync::mpsc::unbounded_channel;

    fn default_persistence_handle() -> PersistenceHandle<EthPrimitives> {
        let provider = create_test_provider_factory();

        let (_finished_exex_height_tx, finished_exex_height_rx) =
            tokio::sync::watch::channel(FinishedExExHeight::NoExExs);

        let pruner =
            Pruner::new_with_factory(provider.clone(), vec![], 5, 0, None, finished_exex_height_rx);

        let (sync_metrics_tx, _sync_metrics_rx) = unbounded_channel();
        PersistenceHandle::<EthPrimitives>::spawn_service(provider, pruner, sync_metrics_tx, None)
    }

    #[test]
    fn test_save_blocks_empty() {
        reth_tracing::init_test_tracing();
        let persistence_handle = default_persistence_handle();

        let blocks = vec![];
        let (tx, rx) = crossbeam_channel::bounded(1);

        persistence_handle.save_blocks(blocks, tx).unwrap();

        let hash = rx.recv().unwrap();
        assert_eq!(hash, None);
    }

    #[test]
    fn test_save_blocks_single_block() {
        reth_tracing::init_test_tracing();
        let persistence_handle = default_persistence_handle();
        let block_number = 0;
        let mut test_block_builder = TestBlockBuilder::eth();
        let executed =
            test_block_builder.get_executed_block_with_number(block_number, B256::random());
        let block_hash = executed.recovered_block().hash();

        let blocks = vec![executed];
        let (tx, rx) = crossbeam_channel::bounded(1);

        persistence_handle.save_blocks(blocks, tx).unwrap();

        let BlockNumHash { hash: actual_hash, number: _ } = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("test timed out")
            .expect("no hash returned");

        assert_eq!(block_hash, actual_hash);
    }

    #[test]
    fn test_save_blocks_multiple_blocks() {
        reth_tracing::init_test_tracing();
        let persistence_handle = default_persistence_handle();

        let mut test_block_builder = TestBlockBuilder::eth();
        let blocks = test_block_builder.get_executed_blocks(0..5).collect::<Vec<_>>();
        let last_hash = blocks.last().unwrap().recovered_block().hash();
        let (tx, rx) = crossbeam_channel::bounded(1);

        persistence_handle.save_blocks(blocks, tx).unwrap();
        let BlockNumHash { hash: actual_hash, number: _ } = rx.recv().unwrap().unwrap();
        assert_eq!(last_hash, actual_hash);
    }

    #[test]
    fn test_save_blocks_multiple_calls() {
        reth_tracing::init_test_tracing();
        let persistence_handle = default_persistence_handle();

        let ranges = [0..1, 1..2, 2..4, 4..5];
        let mut test_block_builder = TestBlockBuilder::eth();
        for range in ranges {
            let blocks = test_block_builder.get_executed_blocks(range).collect::<Vec<_>>();
            let last_hash = blocks.last().unwrap().recovered_block().hash();
            let (tx, rx) = crossbeam_channel::bounded(1);

            persistence_handle.save_blocks(blocks, tx).unwrap();

            let BlockNumHash { hash: actual_hash, number: _ } = rx.recv().unwrap().unwrap();
            assert_eq!(last_hash, actual_hash);
        }
    }
}
