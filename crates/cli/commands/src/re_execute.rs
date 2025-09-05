//! Re-execute blocks from database in parallel.

use crate::common::{
    AccessRights, CliComponentsBuilder, CliNodeComponents, CliNodeTypes, Environment,
    EnvironmentArgs,
};
use alloy_consensus::{BlockHeader, TxReceipt};
use clap::Parser;
use eyre::WrapErr;
use reth_chainspec::{EthChainSpec, EthereumHardforks, Hardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_consensus::FullConsensus;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{format_gas_throughput, BlockBody, GotExpected, SignedTransaction};
use reth_provider::{
    BlockNumReader, BlockReader, ChainSpecProvider, DatabaseProviderFactory, ReceiptProvider,
    StaticFileProviderFactory, TransactionVariant,
};
use reth_revm::database::StateProviderDatabase;
use reth_stages::stages::calculate_gas_used_from_headers;
use std::{
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

        let provider = provider_factory.database_provider_ro()?;
        let components = components(provider_factory.chain_spec());

        let min_block = self.from;
        let max_block = self.to.unwrap_or(provider.best_block_number()?);

        let total_blocks = max_block - min_block;
        let total_gas = calculate_gas_used_from_headers(
            &provider_factory.static_file_provider(),
            min_block..=max_block,
        )?;
        let blocks_per_task = total_blocks / self.num_tasks;

        let db_at = {
            let provider_factory = provider_factory.clone();
            move |block_number: u64| {
                StateProviderDatabase(
                    provider_factory.history_by_block_number(block_number).unwrap(),
                )
            }
        };

        let (stats_tx, mut stats_rx) = mpsc::unbounded_channel();

        let mut tasks = JoinSet::new();
        for i in 0..self.num_tasks {
            let start_block = min_block + i * blocks_per_task;
            let end_block =
                if i == self.num_tasks - 1 { max_block } else { start_block + blocks_per_task };

            // Spawn thread executing blocks
            let provider_factory = provider_factory.clone();
            let evm_config = components.evm_config().clone();
            let consensus = components.consensus().clone();
            let db_at = db_at.clone();
            let stats_tx = stats_tx.clone();
            tasks.spawn_blocking(move || {
                let mut executor = evm_config.batch_executor(db_at(start_block - 1));
                for block in start_block..end_block {
                    let block = provider_factory
                        .recovered_block(block.into(), TransactionVariant::NoHash)?
                        .unwrap();
                    let result = executor.execute_one(&block)?;

                    if let Err(err) = consensus
                        .validate_block_post_execution(&block, &result)
                        .wrap_err_with(|| format!("Failed to validate block {}", block.number()))
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
                                    return Err(err);
                                }
                            } else {
                                continue;
                            }
                        }

                        return Err(err);
                    }
                    let _ = stats_tx.send(block.gas_used());

                    // Reset DB once in a while to avoid OOM
                    if executor.size_hint() > 1_000_000 {
                        executor = evm_config.batch_executor(db_at(block.number()));
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

        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                Some(gas_used) = stats_rx.recv() => {
                    total_executed_blocks += 1;
                    total_executed_gas += gas_used;
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

        info!(
            start_block = min_block,
            end_block = max_block,
            throughput=?format_gas_throughput(total_executed_gas, instant.elapsed()),
            "Re-executed successfully"
        );

        Ok(())
    }
}
