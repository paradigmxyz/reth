//! Re-execute blocks from database in parallel.

use crate::common::{
    AccessRights, CliComponentsBuilder, CliNodeComponents, CliNodeTypes, Environment,
    EnvironmentArgs,
};
use alloy_consensus::{transaction::TxHashRef, BlockHeader, TxReceipt};
use clap::Parser;
use eyre::WrapErr;
use reth_chainspec::{EthChainSpec, EthereumHardforks, Hardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_util::cancellation::CancellationToken;
use reth_consensus::FullConsensus;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{format_gas_throughput, BlockBody, GotExpected};
use reth_provider::{
    BlockNumReader, BlockReader, ChainSpecProvider, DatabaseProviderFactory, ReceiptProvider,
    StaticFileProviderFactory, TransactionVariant,
};
use reth_revm::database::StateProviderDatabase;
use reth_stages::stages::calculate_gas_used_from_headers;
use reth_trie::iter::{IntoParallelRefIterator, ParallelIterator};
use std::{
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinSet};
use tracing::*;

/// `reth re-execute` command
///
/// Re-execute blocks in parallel to verify historical sync correctness.
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    /// The height to start at.
    #[arg(long, default_value = "1")]
    from: u64,

    /// The height to end at. Defaults to the latest block.
    #[arg(long)]
    to: Option<u64>,

    /// Number of tasks to run in parallel
    #[arg(long, default_value = "10")]
    num_tasks: u64,

    /// Continues with execution when an invalid block is encountered and collects these blocks.
    #[arg(long)]
    skip_invalid_blocks: bool,

    /// Total number of blocks to execute, distributed evenly across the range.
    /// If not set, all blocks in the range are executed.
    #[arg(long)]
    total_blocks: Option<u64>,

    /// Size of contiguous block ranges to execute when using --total-blocks.
    #[arg(long, default_value = "10")]
    chunk_size: u64,
}

impl<C: ChainSpecParser> Command<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + Hardforks + EthereumHardforks>> Command<C> {
    /// Execute `re-execute` command
    pub async fn execute<N>(self, components: impl CliComponentsBuilder<N>) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
    {
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RO)?;

        let components = components(provider_factory.chain_spec());

        let min_block = self.from;
        let best_block = DatabaseProviderFactory::database_provider_ro(&provider_factory)?
            .best_block_number()?;
        let mut max_block = best_block;
        if let Some(to) = self.to {
            if to > best_block {
                warn!(
                    requested = to,
                    best_block,
                    "Requested --to is beyond available chain head; clamping to best block"
                );
            } else {
                max_block = to;
            }
        };

        let full_range_size = max_block - min_block;
        let (blocks_to_execute, chunk_size) = if let Some(total_blocks) = self.total_blocks {
            let total_blocks = total_blocks.min(full_range_size);
            let chunk_size = self.chunk_size.min(total_blocks);
            (total_blocks, chunk_size)
        } else {
            (full_range_size, full_range_size)
        };
        let blocks_per_task = blocks_to_execute / self.num_tasks;
        let segment_size = full_range_size / self.num_tasks;
        let chunks_per_task = blocks_per_task / chunk_size;

        // Pre-calculate chunk ranges for each task
        let task_ranges: Vec<Vec<RangeInclusive<u64>>> = (0..self.num_tasks)
            .map(|i| {
                let segment_start = min_block + i * segment_size;
                let segment_end =
                    if i == self.num_tasks - 1 { max_block } else { segment_start + segment_size };

                // Distance between chunk start positions
                let task_stride = if chunks_per_task > 1 {
                    (segment_end - segment_start - chunk_size) / (chunks_per_task - 1)
                } else {
                    0
                };

                (0..chunks_per_task)
                    .map(|chunk_idx| {
                        let chunk_start = segment_start + chunk_idx * task_stride;
                        let chunk_end = (chunk_start + chunk_size).min(segment_end);
                        chunk_start..=chunk_end
                    })
                    .collect()
            })
            .collect();

        info!(
            tasks = task_ranges.len(),
            ranges = task_ranges.iter().flatten().count(),
            blocks = task_ranges.iter().flatten().cloned().flatten().count(),
            "Prepared task ranges"
        );

        // Calculate total gas from headers for all chunks we'll execute
        let total_gas: u64 = task_ranges
            .par_iter()
            .flatten()
            .map(|range| {
                calculate_gas_used_from_headers(
                    &provider_factory.static_file_provider(),
                    range.clone(),
                )
                .unwrap_or(0)
            })
            .sum();

        let db_at = {
            let provider_factory = provider_factory.clone();
            move |block_number: u64| {
                StateProviderDatabase(
                    provider_factory.history_by_block_number(block_number).unwrap(),
                )
            }
        };

        let skip_invalid_blocks = self.skip_invalid_blocks;
        let (stats_tx, mut stats_rx) = mpsc::unbounded_channel();
        let (info_tx, mut info_rx) = mpsc::unbounded_channel();
        let cancellation = CancellationToken::new();
        let _guard = cancellation.drop_guard();

        let mut tasks = JoinSet::new();
        for chunk_ranges in task_ranges {
            // Spawn thread executing blocks
            let provider_factory = provider_factory.clone();
            let evm_config = components.evm_config().clone();
            let consensus = components.consensus().clone();
            let db_at = db_at.clone();
            let stats_tx = stats_tx.clone();
            let info_tx = info_tx.clone();
            let cancellation = cancellation.clone();
            tasks.spawn_blocking(move || {
                'chunks: for chunk_range in chunk_ranges {
                    let chunk_start = *chunk_range.start();
                    let chunk_end = *chunk_range.end();

                    let mut executor = evm_config.batch_executor(db_at(chunk_start - 1));
                    let mut executor_created = Instant::now();
                    let executor_lifetime = Duration::from_secs(120);

                    'blocks: for block in chunk_start..chunk_end {
                        if cancellation.is_cancelled() {
                            // exit if the program is being terminated
                            break 'chunks
                        }

                        let block = provider_factory
                            .recovered_block(block.into(), TransactionVariant::NoHash)?
                            .unwrap();

                        let result = match executor.execute_one(&block) {
                            Ok(result) => result,
                            Err(err) => {
                                if skip_invalid_blocks {
                                    executor = evm_config.batch_executor(db_at(block.number()));
                                    let _ = info_tx.send((block, eyre::Report::new(err)));
                                    continue
                                }
                                return Err(err.into())
                            }
                        };

                        if let Err(err) = consensus
                            .validate_block_post_execution(&block, &result)
                            .wrap_err_with(|| {
                                format!("Failed to validate block {} {}", block.number(), block.hash())
                            })
                        {
                            let correct_receipts =
                                provider_factory.receipts_by_block(block.number().into())?.unwrap();

                            for (i, (receipt, correct_receipt)) in
                                result.receipts.iter().zip(correct_receipts.iter()).enumerate()
                            {
                                if receipt != correct_receipt {
                                    let tx_hash = block.body().transactions()[i].tx_hash();
                                    error!(
                                        ?receipt,
                                        ?correct_receipt,
                                        index = i,
                                        ?tx_hash,
                                        "Invalid receipt"
                                    );
                                    let expected_gas_used = correct_receipt.cumulative_gas_used() -
                                        if i == 0 {
                                            0
                                        } else {
                                            correct_receipts[i - 1].cumulative_gas_used()
                                        };
                                    let got_gas_used = receipt.cumulative_gas_used() -
                                        if i == 0 {
                                            0
                                        } else {
                                            result.receipts[i - 1].cumulative_gas_used()
                                        };
                                    if got_gas_used != expected_gas_used {
                                        let mismatch = GotExpected {
                                            expected: expected_gas_used,
                                            got: got_gas_used,
                                        };

                                        error!(number=?block.number(), ?mismatch, "Gas usage mismatch");
                                        if skip_invalid_blocks {
                                            executor = evm_config.batch_executor(db_at(block.number()));
                                            let _ = info_tx.send((block, err));
                                            continue 'blocks;
                                        }
                                        return Err(err);
                                    }
                                } else {
                                    continue;
                                }
                            }

                            return Err(err);
                        }
                        let _ = stats_tx.send(block.gas_used());

                        // Reset DB once in a while to avoid OOM or read tx timeouts
                        if executor.size_hint() > 1_000_000 ||
                            executor_created.elapsed() > executor_lifetime
                        {
                            executor = evm_config.batch_executor(db_at(block.number()));
                            executor_created = Instant::now();
                        }
                    }
                }

                eyre::Ok(())
            });
        }

        let instant = Instant::now();
        let mut total_executed_blocks = 0;
        let mut total_executed_gas = 0;

        let mut last_logged_gas = 0;
        let mut last_logged_blocks = 0;
        let mut last_logged_time = Instant::now();
        let mut invalid_blocks = Vec::new();

        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                Some(gas_used) = stats_rx.recv() => {
                    total_executed_blocks += 1;
                    total_executed_gas += gas_used;
                }
                Some((block, err)) = info_rx.recv() => {
                    error!(?err, block=?block.num_hash(), "Invalid block");
                    invalid_blocks.push(block.num_hash());
                }
                result = tasks.join_next() => {
                    if let Some(result) = result {
                        if matches!(result, Err(_) | Ok(Err(_))) {
                            error!(?result);
                            return Err(eyre::eyre!("Re-execution failed: {result:?}"));
                        }
                    } else {
                        break;
                    }
                }
                _ = interval.tick() => {
                    let blocks_executed = total_executed_blocks - last_logged_blocks;
                    let gas_executed = total_executed_gas - last_logged_gas;

                    if blocks_executed > 0 {
                        let progress = 100.0 * total_executed_gas as f64 / total_gas as f64;
                        info!(
                            throughput=?format_gas_throughput(gas_executed, last_logged_time.elapsed()),
                            progress=format!("{progress:.2}%"),
                            "Executed {blocks_executed} blocks"
                        );
                    }

                    last_logged_blocks = total_executed_blocks;
                    last_logged_gas = total_executed_gas;
                    last_logged_time = Instant::now();
                }
            }
        }

        if invalid_blocks.is_empty() {
            info!(
                start_block = min_block,
                end_block = max_block,
                %total_executed_blocks,
                throughput=?format_gas_throughput(total_executed_gas, instant.elapsed()),
                "Re-executed successfully"
            );
        } else {
            info!(
                start_block = min_block,
                end_block = max_block,
                %total_executed_blocks,
                invalid_block_count = invalid_blocks.len(),
                ?invalid_blocks,
                throughput=?format_gas_throughput(total_executed_gas, instant.elapsed()),
                "Re-executed with invalid blocks"
            );
        }

        Ok(())
    }
}
