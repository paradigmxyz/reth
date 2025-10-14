use std::collections::VecDeque;

use alloy_primitives::BlockNumber;
use reth_codecs::Compact;
use reth_config::config::EtlConfig;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    tables::{self, FilterMapMetaKey},
    transaction::{DbTx, DbTxMut},
    DbTxUnwindExt, RawKey, RawValue,
};
use reth_etl::Collector;
use reth_log_index::{
    indexer::LogIndexer,
    utils::{count_log_values_in_block, log_values_from_receipts},
};
use reth_log_index_common::{
    FilterMapColumns, FilterMapMeta, FilterMapParams, LogValueIndex, MapRowIndex,
    ProcessBatchResult,
};

use reth_provider::{
    BlockReader, DBProvider, LogIndexProvider, PruneCheckpointReader, PruneCheckpointWriter,
    ReceiptProvider, StageCheckpointReader, StageCheckpointWriter,
};
use reth_prune::PruneMode;
use reth_stages_api::{
    EntitiesCheckpoint, ExecInput, ExecOutput, IndexLogsCheckpoint, Stage, StageCheckpoint,
    StageError, StageId, UnwindInput, UnwindOutput,
};
use tracing::{info, trace};

/// Log indexing stage, following EIP-7745 (<https://eips.ethereum.org/EIPS/eip-7745>)
///
/// This stage indexes logs from blocks and stores them in a separate table for efficient querying.
/// It processes blocks in batches, extracts logs, and inserts them into db.
#[derive(Debug, Clone)]
pub struct IndexLogsStage {
    /// Maximum number of blocks to process before committing
    pub commit_threshold: u64,
    /// ETL configuration for buffering
    pub etl_config: EtlConfig,
    /// `FilterMap` parameters
    pub params: FilterMapParams,
    /// Prune mode configuration
    pub prune_mode: Option<PruneMode>,
}

impl IndexLogsStage {
    /// Creates a new instance of the log indexing stage with the given parameters.
    pub const fn new(
        commit_threshold: u64,
        etl_config: EtlConfig,
        params: FilterMapParams,
        prune_mode: Option<PruneMode>,
    ) -> Self {
        Self { commit_threshold, etl_config, params, prune_mode }
    }

    /// Gets the log indexing checkpoint (like Merkle does)
    fn get_execution_checkpoint<P: StageCheckpointReader>(
        &self,
        provider: &P,
    ) -> Result<Option<IndexLogsCheckpoint>, StageError> {
        let buf = provider.get_stage_checkpoint_progress(StageId::IndexLogs)?.unwrap_or_default();
        if buf.is_empty() {
            return Ok(None);
        }
        let (checkpoint, _) = IndexLogsCheckpoint::from_compact(&buf, buf.len());
        Ok(Some(checkpoint))
    }

    /// Saves the log indexing checkpoint
    fn save_execution_checkpoint<P: StageCheckpointWriter>(
        &self,
        provider: &P,
        checkpoint: Option<IndexLogsCheckpoint>,
    ) -> Result<(), StageError> {
        let mut buf = vec![];
        if let Some(checkpoint) = checkpoint {
            checkpoint.to_compact(&mut buf);
        }
        Ok(provider.save_stage_checkpoint_progress(StageId::IndexLogs, buf)?)
    }

    /// Writes to db and updates metadata
    fn write_indices(
        &self,
        provider: &impl DBProvider<Tx: DbTxMut>,
        row_collector: &mut Collector<MapRowIndex, FilterMapColumns>,
        boundary_collector: &mut Collector<BlockNumber, LogValueIndex>,
        meta: FilterMapMeta,
    ) -> Result<(), StageError> {
        info!(
            target: "sync::stages::index_logs",
            total_rows = row_collector.len(),
            total_boundaries = boundary_collector.len(),
            "Writing to database"
        );
        // Write to database
        let mut filter_rows_cursor =
            provider.tx_ref().cursor_write::<tables::RawTable<tables::FilterMapRows>>()?;
        let mut log_indices_cursor =
            provider.tx_ref().cursor_write::<tables::RawTable<tables::LogValueIndices>>()?;

        // Write filter map rows
        for entry in row_collector.iter()? {
            let (map_row_index_bytes, columns_bytes) = entry?;
            filter_rows_cursor.insert(
                RawKey::<MapRowIndex>::from_vec(map_row_index_bytes),
                &RawValue::<FilterMapColumns>::from_vec(columns_bytes),
            )?;
        }

        for entry in boundary_collector.iter()? {
            let (block_num, log_value_idx) = entry?;
            log_indices_cursor.insert(
                RawKey::<BlockNumber>::from_vec(block_num),
                &RawValue::<LogValueIndex>::from_vec(log_value_idx),
            )?;
        }

        provider.tx_ref().put::<tables::FilterMapMeta>(FilterMapMetaKey, meta)?;
        Ok(())
    }
}

impl Default for IndexLogsStage {
    fn default() -> Self {
        Self {
            commit_threshold: 1000,
            etl_config: EtlConfig::default(),
            params: FilterMapParams::default(),
            prune_mode: None,
        }
    }
}

impl<Provider> Stage<Provider> for IndexLogsStage
where
    Provider: DBProvider<Tx: DbTxMut>
        + StageCheckpointReader
        + StageCheckpointWriter
        + BlockReader
        + ReceiptProvider
        + LogIndexProvider
        + PruneCheckpointReader
        + PruneCheckpointWriter,
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::IndexLogs
    }

    /// Execute the stage.
    fn execute(
        &mut self,
        provider: &Provider,
        mut input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        // TODO: implement pruning logic

        let range = input.next_block_range();

        let meta = match provider.get_metadata() {
            Ok(Some(meta)) => meta,
            Ok(None) => Default::default(),
            Err(e) => return Err(StageError::Fatal(Box::new(e))),
        };

        let is_first_run = meta.last_log_value_index == 0;

        let mut checkpoint = self.get_execution_checkpoint(provider)?;

        // if first run and checkpoint exists, then we need to delete the checkpoint as it is
        // corrupted
        if is_first_run && checkpoint.is_some() {
            self.save_execution_checkpoint(provider, None)?;
            checkpoint = None;
        }

        let mut entities_checkpoint = input
            .checkpoint()
            .entities_stage_checkpoint()
            .unwrap_or(EntitiesCheckpoint { processed: 0, total: 0 });

        // Create the log indexer with checkpoint recovery if needed
        let starting_index = if is_first_run {
            // First time indexing, start from 0
            0
        } else {
            // Resume from the next log value after the last indexed one
            meta.last_log_value_index.saturating_add(1)
        };

        // Create the log indexer with checkpoint recovery if needed
        let mut log_indexer = if let Some(cp) = checkpoint.as_ref() {
            info!(
                target: "sync::stages::index_logs",
                "Restored from checkpoint: {} pending rows, {} pending boundaries,
            current map {}",
                cp.pending_rows.len(),
                cp.pending_boundaries.len(),
                cp.current_map
            );

            LogIndexer::from_checkpoint(
                starting_index,
                self.params.clone(),
                cp.pending_rows
                    .iter()
                    .map(|row| (row.map_row_index, row.columns.clone()))
                    .collect(),
            )
        } else {
            LogIndexer::new(starting_index, self.params.clone())
        };
        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()))
        }

        // Restore block boundaries from checkpoint
        let mut block_boundaries: VecDeque<(BlockNumber, LogValueIndex)> =
            if let Some(cp) = checkpoint {
                cp.pending_boundaries
                    .into_iter()
                    .map(|b| (b.block_number, b.log_value_index))
                    .filter(|&(block_num, _)| block_num < *range.end())
                    .collect()
            } else {
                VecDeque::new()
            };
        // print the latest block boundary if any
        if let Some((bn, lvi)) = block_boundaries.back() {
            info!(
                target: "sync::stages::index_logs",
                block_number = bn,
                log_value_index = lvi,
                "Latest restored block boundary"
            );
        }

        info!(
            target: "sync::stages::index_logs",
            ?range,
            checkpoint = ?input.checkpoint().block_number,
            target = ?input.target(),
            "Starting IndexLogsStage execution"
        );

        info!(
            target: "sync::stages::index_logs",
            ?meta,
            "Loaded metadata"
        );

        // ETL collectors for buffering filter map rows and block boundaries
        let mut row_collector: Collector<MapRowIndex, FilterMapColumns> =
            Collector::new(self.etl_config.file_size, self.etl_config.dir.clone());

        let mut boundary_collector: Collector<BlockNumber, LogValueIndex> =
            Collector::new(self.etl_config.file_size, self.etl_config.dir.clone());

        info!(
            target: "sync::stages::index_logs",
            "Indexing logs from block {:?}",
            range
        );

        let end_block = *range.end(); // Track what we've actually written to storage
        let mut last_written_block = meta.last_indexed_block;
        let mut last_written_log_value = Some(meta.last_log_value_index);

        info!(
            target: "sync::stages::index_logs",
            "Starting index loop"
        );

        loop {
            let (block_range, is_final_range) =
                input.next_block_range_with_threshold(self.commit_threshold);

            let end_block = *block_range.end();

            let receipts = provider.receipts_by_block_range(block_range.clone())?;

            info!(
                target: "sync::stages::index_logs",
                ?block_range,
                receipts_count = receipts.len(),
                is_final_range,
                "Processing block batch"
            );

            // Pre-calculate block boundaries and collect log values
            //
            let mut log_value_index_start = log_indexer.current_index();

            for (block_num, block_receipts) in block_range.clone().zip(&receipts) {
                block_boundaries.push_back((block_num, log_value_index_start));

                let log_value_count = count_log_values_in_block(block_receipts);
                log_value_index_start += log_value_count;
                entities_checkpoint.total += log_value_count;
            }

            // Process log values with the indexer
            let log_values = receipts.iter().flat_map(|r| log_values_from_receipts(r));
            let process_result = log_indexer
                .process_batch(log_values)
                .map_err(|e| StageError::Fatal(Box::new(e)))?;

            match process_result {
                ProcessBatchResult::MapsCompleted { completed_maps, values_processed } => {
                    entities_checkpoint.processed += values_processed as u64;
                    info!(
                        target: "sync::stages::index_logs",
                        maps_completed = completed_maps.len(),
                        entities_checkpoint = ?entities_checkpoint,
                        "Completed map(s), flushing rows"
                    );

                    // Write completed maps to collector
                    for completed_map in completed_maps {
                        for (map_row_index, columns) in completed_map.rows {
                            row_collector
                                .insert(map_row_index, columns)
                                .map_err(|e| StageError::Fatal(Box::new(e)))?;
                        }

                        // Track for metadata update
                        last_written_log_value = Some(completed_map.end_log_value_index);
                    }
                }
                ProcessBatchResult::NoMapsCompleted { values_processed } => {
                    entities_checkpoint.processed += values_processed as u64;
                    trace!(
                        target: "sync::stages::index_logs",
                        entities_checkpoint = ?entities_checkpoint,
                        "No complete maps in this batch"
                    );
                }
            }

            // Write block boundaries for completed maps only
            let cutoff = log_indexer.completed_maps_cutoff();
            info!(
                target: "sync::stages::index_logs",
                cutoff,
                boundaries_pending = block_boundaries.len(),
                "Flushing block boundaries"
            );
            while let Some((_, lv_index)) = block_boundaries.front() {
                if *lv_index < cutoff {
                    let (bn, lvi) = block_boundaries.pop_front().unwrap();
                    boundary_collector
                        .insert(bn, lvi)
                        .map_err(|e| StageError::Fatal(Box::new(e)))?;

                    // Track for metadata update
                    last_written_block = bn;
                } else {
                    break;
                }
            }
            input.checkpoint = Some(StageCheckpoint::new(end_block));

            if is_final_range {
                let meta = FilterMapMeta {
                    first_indexed_block: meta.first_indexed_block,
                    last_indexed_block: last_written_block,
                    is_last_indexed_block_complete: end_block <= last_written_block,
                    first_map_index: 0,
                    last_map_index: if let Some(lv) = last_written_log_value {
                        (lv >> self.params.log_values_per_map) as u32
                    } else {
                        meta.last_map_index
                    },
                    last_log_value_index: last_written_log_value
                        .unwrap_or(meta.last_log_value_index),
                    oldest_epoch_map_count: meta.oldest_epoch_map_count,
                };
                self.write_indices(provider, &mut row_collector, &mut boundary_collector, meta)?;
                // Check if we need to save checkpoint
                if !log_indexer.pending_rows().is_empty() || !block_boundaries.is_empty() {
                    let checkpoint = IndexLogsCheckpoint::new(
                        log_indexer.current_map(),
                        log_indexer.current_index(),
                        log_indexer.pending_rows().iter().map(|(k, v)| (*k, v.clone())).collect(),
                        block_boundaries.clone().into(),
                    );
                    self.save_execution_checkpoint(provider, Some(checkpoint))?;
                } else {
                    // Clear checkpoint when everything is complete
                    self.save_execution_checkpoint(provider, None)?;
                }
                info!(
                    target: "sync::stages::index_logs",
                    meta = ?meta,
                    "Updated metadata and committing"
                );
                break;
            }
        }
        info!(
            target: "sync::stages::index_logs",
            total_indexed_blocks = last_written_block - meta.first_indexed_block,
            entities_checkpoint = ?entities_checkpoint,
            "Completed log indexing"
        );

        Ok(ExecOutput {
            checkpoint: StageCheckpoint::new(end_block)
                .with_entities_stage_checkpoint(entities_checkpoint),
            done: true,
        })
    }

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let (_, unwind_to, _) = input.unwind_block_range_with_threshold(self.commit_threshold);
        let mut meta = match provider.get_metadata() {
            Ok(Some(meta)) => meta,
            Ok(None) => Default::default(),
            Err(e) => return Err(StageError::Fatal(Box::new(e))),
        };

        info!(
            target: "sync::stages::index_logs",
            "Unwinding logs index from block {} to {}",
            meta.last_indexed_block,
            unwind_to
        );

        // Get the starting block number of the map containing the target block
        // Get the log value index for the unwind target block
        let unwind_to_log_value_index = if unwind_to == 0 {
            0
        } else {
            provider
                .get_log_value_indices_range(unwind_to..=unwind_to)
                .map_err(|e| StageError::Fatal(Box::new(e)))?
                .first()
                .map(|b| b.log_value_index)
                .unwrap_or_else(|| {
                    // Block has no logs, get the previous block's log value index
                    provider
                        .get_log_value_indices_range(..unwind_to)
                        .ok()
                        .and_then(|indices| indices.last().map(|b| b.log_value_index))
                        .unwrap_or(0)
                })
        };
        let unwind_to_map = (unwind_to_log_value_index >> self.params.log_values_per_map) as u32;
        let current_map =
            ((meta.last_log_value_index + 1) >> self.params.log_values_per_map) as u32;

        // Find the block that starts this map (or the closest block before it)
        let mut effective_unwind_to = unwind_to;

        // Walk backwards to find the last block that belongs to the previous complete map
        let mut cursor = provider.tx_ref().cursor_read::<tables::LogValueIndices>()?;

        if unwind_to_map > 0 {
            // We want to keep the last complete map (unwind_to_map - 1)
            let target_map_boundary = (unwind_to_map as u64) << self.params.log_values_per_map;

            // Find the last block of the previous map
            if let Some((block, lv_idx)) = cursor.seek_exact(unwind_to)? {
                let mut current_block = block;
                let mut current_lv = lv_idx;

                // Walk backwards until we find a block that ends before the map boundary
                while current_lv >= target_map_boundary && current_block > 0 {
                    if let Some((prev_block, prev_lv)) = cursor.prev()? {
                        current_block = prev_block;
                        current_lv = prev_lv;
                        effective_unwind_to = current_block;
                    } else {
                        effective_unwind_to = 0;
                        break;
                    }
                }
            }
        } else {
            // Unwinding to map 0, so we unwind everything
            effective_unwind_to = 0;
        }

        // unwind LogValueIndices till the first block of the map containing unwind_to
        provider.tx_ref().unwind_table_by_num::<tables::LogValueIndices>(effective_unwind_to)?;

        // Delete FilterMapRows for all maps that will be unwound
        if unwind_to_map < current_map {
            let mut filter_rows_cursor =
                provider.tx_ref().cursor_write::<tables::FilterMapRows>()?;

            // Delete all rows for maps >= unwind_to_map
            for map_to_delete in unwind_to_map..=current_map {
                // Calculate all row indices for this map
                for row_idx in 0..self.params.map_height() {
                    let map_row_index = self.params.map_row_index(map_to_delete, row_idx);

                    if filter_rows_cursor.seek_exact(map_row_index)?.is_some() {
                        filter_rows_cursor.delete_current()?;
                    }
                }
            }
        }
        // Update metadata to reflect the state after unwind
        if effective_unwind_to == 0 {
            // Complete unwind to genesis
            meta.last_indexed_block = 0;
            meta.last_log_value_index = 0;
            meta.last_map_index = 0;
            meta.first_map_index = 0;
            meta.is_last_indexed_block_complete = true;
        } else {
            meta.last_log_value_index = if unwind_to_map > 0 {
                ((unwind_to_map as u64) << self.params.log_values_per_map).saturating_sub(1)
            } else {
                0
            };
            meta.last_map_index = unwind_to_map.saturating_sub(1);
        }
        meta.is_last_indexed_block_complete = true; // Since we unwind to map boundaries

        // Save updated metadata
        provider
            .tx_ref()
            .put::<tables::FilterMapMeta>(FilterMapMetaKey, meta)
            .map_err(|e| StageError::Fatal(Box::new(e)))?;

        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(effective_unwind_to) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        ExecuteStageTestRunner, StageTestRunner, StorageKind, TestRunnerError, TestStageDB,
        UnwindStageTestRunner,
    };
    use alloy_primitives::B256;
    use assert_matches::assert_matches;

    use reth_ethereum_primitives::Block;
    use reth_primitives_traits::{InMemorySize, SealedBlock};
    use reth_stages_api::{EntitiesCheckpoint, StageUnitCheckpoint};
    use reth_testing_utils::generators::{
        self, random_block, random_block_range, random_receipt, BlockParams, BlockRangeParams,
    };

    #[tokio::test]
    async fn execute_index_logs_range() {
        let (previous_stage, stage_progress) = (500, 0);
        let mut rng = generators::rng();
        let tx_count: u64 = 190; // Number of transactions in a non-empty block
        let logs_count: u64 = 1;
        let topic_count: u64 = 3;

        // Set up the runner
        let runner = IndexLogsTestRunner::default();
        let input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        let blocks = (stage_progress + 1..=input.target())
            .map(|number| {
                random_block(
                    &mut rng,
                    number,
                    BlockParams { tx_count: Some(tx_count as u8), ..Default::default() },
                )
            })
            .collect::<Vec<_>>();
        runner
            .db
            .insert_blocks(blocks.iter(), StorageKind::Static)
            .expect("failed to insert blocks");

        let mut receipts = Vec::new();

        let mut total_log_values = 0;
        for block in &blocks {
            receipts.reserve_exact(block.body().size());
            for transaction in &block.body().transactions {
                let receipt = random_receipt(
                    &mut rng,
                    transaction,
                    Some(logs_count as u8),
                    Some(topic_count as u8),
                );
                receipts.push((receipts.len() as u64, receipt.clone()));
                let log_value_count = count_log_values_in_block(&[receipt]);
                total_log_values += log_value_count;
            }
        }

        runner.db.insert_receipts(receipts).expect("insert receipts");

        let total_values = total_log_values;
        let expected_maps = total_values / runner.params.values_per_map();
        let total_values_in_complete_maps = expected_maps * runner.params.values_per_map();

        info!("total_values : {}", total_values);
        info!("expected_maps : {}", expected_maps);
        info!("total_values_in_complete_maps : {}", total_values_in_complete_maps);

        let rx = runner.execute(input);

        // Assert the successful result
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number,
                    stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                        processed,
                        total
                    }))
                },
                done: true
            }) if block_number == previous_stage && processed == total &&
                total == total_values
        );
        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    struct IndexLogsTestRunner {
        db: TestStageDB,
        commit_threshold: u64,
        etl_config: EtlConfig,
        prune_mode: Option<PruneMode>,
        params: FilterMapParams,
    }

    impl Default for IndexLogsTestRunner {
        fn default() -> Self {
            Self {
                db: TestStageDB::default(),
                commit_threshold: 100,
                etl_config: EtlConfig::default(),
                prune_mode: None,
                params: FilterMapParams::default(),
            }
        }
    }

    impl StageTestRunner for IndexLogsTestRunner {
        type S = IndexLogsStage;

        fn db(&self) -> &TestStageDB {
            &self.db
        }

        fn stage(&self) -> Self::S {
            IndexLogsStage {
                commit_threshold: self.commit_threshold,
                etl_config: self.etl_config.clone(),
                prune_mode: self.prune_mode,
                params: self.params.clone(),
            }
        }
    }

    impl ExecuteStageTestRunner for IndexLogsTestRunner {
        type Seed = Vec<SealedBlock<Block>>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_progress = input.checkpoint().block_number;
            let end = input.target();
            let mut rng = generators::rng();

            let blocks = random_block_range(
                &mut rng,
                stage_progress + 1..=end,
                BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..2, ..Default::default() },
            );
            self.db.insert_blocks(blocks.iter(), StorageKind::Static)?;
            Ok(blocks)
        }

        fn validate_execution(
            &self,
            _input: ExecInput,
            _output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            // TODO: Implement
            Ok(())
        }
    }

    impl UnwindStageTestRunner for IndexLogsTestRunner {
        fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
            // TODO: Implement
            Ok(())
        }
    }
}
