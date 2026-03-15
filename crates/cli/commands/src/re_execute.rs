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
use reth_evm::{
    execute::{BlockExecutor, Executor},
    ConfigureEvm, Evm,
};
use reth_primitives_traits::{format_gas_throughput, BlockBody, GotExpected, NodePrimitives};
use reth_provider::{
    BlockNumReader, BlockReader, ChainSpecProvider, DatabaseProviderFactory, ReceiptProvider,
    StaticFileProviderFactory, TransactionVariant,
};
use reth_revm::{database::StateProviderDatabase, State};
use reth_stages::stages::calculate_gas_used_from_headers;
use revm_inspectors::tracing::{TracingInspector, TracingInspectorConfig};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
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

    /// Number of tasks to run in parallel. Defaults to the number of available CPUs.
    #[arg(long)]
    num_tasks: Option<u64>,

    /// Number of blocks each worker processes before grabbing the next chunk.
    #[arg(long, default_value = "5000")]
    blocks_per_chunk: u64,

    /// Continues with execution when an invalid block is encountered and collects these blocks.
    #[arg(long)]
    skip_invalid_blocks: bool,
}

impl<C: ChainSpecParser> Command<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

/// Traces a failed transaction at the given index within a block, producing opcode-level output.
///
/// Creates a [`BlockExecutor`] with a [`TracingInspector`]
/// attached but initially disabled. The inspector is enabled only for the target transaction,
/// keeping pre-execution changes and prior transaction replay uninstrumented.
/// The resulting trace is logged as a JSON-serialized geth-style `DefaultFrame` containing
/// opcode-level `structLogs`.
fn trace_failed_transaction<C, DB>(
    evm_config: &C,
    db: StateProviderDatabase<DB>,
    block: &reth_primitives_traits::RecoveredBlock<<C::Primitives as NodePrimitives>::Block>,
    tx_index: usize,
) where
    C: ConfigureEvm,
    DB: reth_revm::database::EvmStateProvider,
{
    let evm_env = match evm_config.evm_env(block.header()) {
        Ok(env) => env,
        Err(err) => {
            error!(%err, "Failed to create EVM env for opcode tracing");
            return;
        }
    };

    let mut state = State::builder().with_database(db).with_bundle_update().build();

    let ctx = match evm_config.context_for_block(block) {
        Ok(ctx) => ctx,
        Err(err) => {
            error!(%err, "Failed to create execution context for opcode tracing");
            return;
        }
    };

    let inspector = TracingInspector::new(TracingInspectorConfig::all());
    let evm = evm_config.evm_with_env_and_inspector(&mut state, evm_env, inspector);
    let mut executor = evm_config.create_executor(evm, ctx);

    // Disable inspector during pre-execution changes and prior tx replay.
    executor.evm_mut().disable_inspector();

    if let Err(err) = executor.apply_pre_execution_changes() {
        error!(%err, "Failed to apply pre-execution changes for opcode tracing");
        return;
    }

    for (i, tx) in block.transactions_recovered().enumerate() {
        if i >= tx_index {
            break;
        }
        if let Err(err) = executor.execute_transaction(tx) {
            error!(index = i, %err, "Failed to replay transaction before failing tx");
            return;
        }
    }

    // Enable inspector for the target transaction.
    executor.evm_mut().enable_inspector();

    let tx = match block.transactions_recovered().nth(tx_index) {
        Some(tx) => tx,
        None => {
            error!(tx_index, "Transaction index out of bounds for opcode tracing");
            return;
        }
    };

    let mut gas_used = 0;
    let mut return_value = Default::default();
    let mut execution_result = None;
    let result = executor.execute_transaction_with_result_closure(tx, |res| {
        gas_used = res.gas_used();
        return_value = res.output().cloned().unwrap_or_default();
        execution_result = Some(format!("{res:?}"));
    });

    // Build geth-style trace with opcode-level structLogs.
    let frame = executor.evm_mut().inspector_mut().geth_builder().geth_traces(
        gas_used,
        return_value,
        Default::default(),
    );

    match serde_json::to_string(&frame) {
        Ok(json) => {
            error!(
                block_number = block.number(),
                block_hash = ?block.hash(),
                tx_index,
                tx_hash = ?block.body().transactions()[tx_index].tx_hash(),
                ?result,
                execution_result,
                opcode_trace = %json,
                "Opcode-level trace for failing transaction"
            );
        }
        Err(err) => {
            error!(%err, "Failed to serialize opcode trace");
        }
    }
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + Hardforks + EthereumHardforks>> Command<C> {
    /// Execute `re-execute` command
    pub async fn execute<N>(
        self,
        components: impl CliComponentsBuilder<N>,
        runtime: reth_tasks::Runtime,
    ) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
    {
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RO, runtime)?;

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

        if min_block > max_block {
            eyre::bail!("--from ({min_block}) is beyond --to ({max_block}), nothing to re-execute");
        }

        let num_tasks = self.num_tasks.unwrap_or_else(|| {
            std::thread::available_parallelism().map(|n| n.get() as u64).unwrap_or(10)
        });

        let total_gas = calculate_gas_used_from_headers(
            &provider_factory.static_file_provider(),
            min_block..=max_block,
        )?;

        let db_at = {
            let provider_factory = provider_factory.clone();
            move |block_number: u64| {
                StateProviderDatabase(
                    provider_factory.history_by_block_number(block_number).unwrap(),
                )
            }
        };

        let skip_invalid_blocks = self.skip_invalid_blocks;
        let blocks_per_chunk = self.blocks_per_chunk;
        let (stats_tx, mut stats_rx) = mpsc::unbounded_channel();
        let (info_tx, mut info_rx) = mpsc::unbounded_channel();
        let cancellation = CancellationToken::new();
        let _guard = cancellation.drop_guard();

        // Shared counter for work stealing: workers atomically grab the next chunk of blocks.
        let next_block = Arc::new(AtomicU64::new(min_block));

        let mut tasks = JoinSet::new();
        for _ in 0..num_tasks {
            let provider_factory = provider_factory.clone();
            let evm_config = components.evm_config().clone();
            let consensus = components.consensus().clone();
            let db_at = db_at.clone();
            let stats_tx = stats_tx.clone();
            let info_tx = info_tx.clone();
            let cancellation = cancellation.clone();
            let next_block = Arc::clone(&next_block);
            tasks.spawn_blocking(move || {
                let executor_lifetime = Duration::from_secs(120);

                loop {
                    if cancellation.is_cancelled() {
                        break;
                    }

                    // Atomically grab the next chunk of blocks.
                    let chunk_start =
                        next_block.fetch_add(blocks_per_chunk, Ordering::Relaxed);
                    if chunk_start >= max_block {
                        break;
                    }
                    let chunk_end = (chunk_start + blocks_per_chunk).min(max_block);

                    let mut executor = evm_config.batch_executor(db_at(chunk_start - 1));
                    let mut executor_created = Instant::now();

                    'blocks: for block in chunk_start..chunk_end {
                        if cancellation.is_cancelled() {
                            break;
                        }

                        let block = provider_factory
                            .recovered_block(block.into(), TransactionVariant::NoHash)?
                            .unwrap();

                        let result = match executor.execute_one(&block) {
                            Ok(result) => result,
                            Err(err) => {
                                if skip_invalid_blocks {
                                    executor =
                                        evm_config.batch_executor(db_at(block.number()));
                                    let _ =
                                        info_tx.send((block, eyre::Report::new(err)));
                                    continue
                                }
                                return Err(err.into())
                            }
                        };

                        if let Err(err) = consensus
                            .validate_block_post_execution(&block, &result, None)
                            .wrap_err_with(|| {
                                format!(
                                    "Failed to validate block {} {}",
                                    block.number(),
                                    block.hash()
                                )
                            })
                        {
                            let correct_receipts = provider_factory
                                .receipts_by_block(block.number().into())?
                                .unwrap();

                            for (i, (receipt, correct_receipt)) in
                                result.receipts.iter().zip(correct_receipts.iter()).enumerate()
                            {
                                if receipt != correct_receipt {
                                    let tx_hash =
                                        block.body().transactions()[i].tx_hash();
                                    error!(
                                        ?receipt,
                                        ?correct_receipt,
                                        index = i,
                                        ?tx_hash,
                                        "Invalid receipt"
                                    );
                                    let expected_gas_used =
                                        correct_receipt.cumulative_gas_used() -
                                            if i == 0 {
                                                0
                                            } else {
                                                correct_receipts[i - 1]
                                                    .cumulative_gas_used()
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

                                        // Trace the mismatched tx at the opcode level.
                                        trace_failed_transaction(
                                            &evm_config,
                                            db_at(block.number() - 1),
                                            &block,
                                            i,
                                        );

                                        if skip_invalid_blocks {
                                            executor = evm_config
                                                .batch_executor(db_at(block.number()));
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
                            executor =
                                evm_config.batch_executor(db_at(block.number()));
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
