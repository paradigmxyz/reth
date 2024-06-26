use crate::ExExNotification;
use reth_config::Config;
use reth_evm::execute::{BatchExecutor, BlockExecutionError, BlockExecutorProvider};
use reth_node_api::FullNodeComponents;
use reth_primitives::BlockNumber;
use reth_provider::{
    BlockReader, Chain, DatabaseProviderFactory, HeaderProvider, HistoricalStateProviderRef,
    ProviderError, StaticFileProviderFactory, TransactionVariant,
};
use reth_prune_types::PruneModes;
use reth_revm::database::StateProviderDatabase;
use reth_stages_types::ExecutionStageThresholds;
use reth_tracing::tracing::{debug, trace};
use std::{
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct BackfillJobFactory<Node: FullNodeComponents> {
    components: Node,
    config: Config,
    thresholds: ExecutionStageThresholds,
}

impl<Node: FullNodeComponents> BackfillJobFactory<Node> {
    pub fn new(components: Node, config: Config, thresholds: ExecutionStageThresholds) -> Self {
        Self { components, config, thresholds }
    }

    pub fn backfill(&self, range: RangeInclusive<BlockNumber>) -> BackfillJob<Node> {
        BackfillJob {
            components: self.components.clone(),
            prune_modes: self.config.prune.clone().unwrap_or_default().segments,
            range,
            thresholds: self.thresholds.clone(),
        }
    }
}

#[derive(Debug)]
pub struct BackfillJob<Node: FullNodeComponents> {
    components: Node,
    prune_modes: PruneModes,
    range: RangeInclusive<BlockNumber>,
    thresholds: ExecutionStageThresholds,
}

impl<Node: FullNodeComponents> Iterator for BackfillJob<Node> {
    type Item = Result<ExExNotification, BlockExecutionError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.range.is_empty() {
            return None
        }

        Some(self.execute_range())
    }
}

impl<Node: FullNodeComponents> BackfillJob<Node> {
    fn execute_range(&mut self) -> Result<ExExNotification, BlockExecutionError> {
        let provider = self.components.provider();
        let provider_ro = provider.database_provider_ro()?.disable_long_read_transaction_safety();

        let mut executor = self.components.block_executor().batch_executor(
            StateProviderDatabase(HistoricalStateProviderRef::new(
                provider_ro.tx_ref(),
                *self.range.start(),
                provider.static_file_provider(),
            )),
            self.prune_modes.clone(),
        );

        let mut fetch_block_duration = Duration::default();
        let mut execution_duration = Duration::default();
        let mut cumulative_gas = 0;
        let batch_start = Instant::now();

        let mut blocks = Vec::new();
        for block_number in self.range.clone() {
            // Fetch the block
            let fetch_block_start = Instant::now();

            let td = provider
                .header_td_by_number(block_number)?
                .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

            // we need the block's transactions but we don't need the transaction hashes
            let block = provider
                .block_with_senders(block_number.into(), TransactionVariant::NoHash)?
                .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

            fetch_block_duration += fetch_block_start.elapsed();

            cumulative_gas += block.gas_used;

            // Configure the executor to use the current state.
            trace!(target: "exex::backfill", number = block_number, txs = block.body.len(), "Executing block");

            // Execute the block
            let execute_start = Instant::now();

            executor.execute_and_verify_one((&block, td).into())?;
            execution_duration += execute_start.elapsed();

            // // Gas metrics
            // if let Some(metrics_tx) = &mut self.metrics_tx {
            //     let _ =
            //         metrics_tx.send(MetricEvent::ExecutionStageGas { gas: block.header.gas_used
            // }); }

            blocks.push(block.seal_slow());

            // Check if we should commit now
            let bundle_size_hint = executor.size_hint().unwrap_or_default() as u64;
            if self.thresholds.is_end_of_batch(
                block_number - *self.range.start(),
                bundle_size_hint,
                cumulative_gas,
                batch_start.elapsed(),
            ) {
                break;
            }
        }

        if let Some(last_block) = blocks.last() {
            let last_block_number = last_block.number;

            debug!(
                target: "exex::backfill",
                range = ?*self.range.start()..=last_block_number,
                block_fetch = ?fetch_block_duration,
                execution = ?execution_duration,
                // throughput = format_gas_throughput(cumulative_gas, execution_duration),
                "Finished executing block range"
            );

            self.range = last_block_number + 1..=*self.range.end();
        }

        let chain = Chain::new(blocks, executor.finalize(), None);
        Ok(ExExNotification::ChainCommitted { new: Arc::new(chain) })
    }
}
