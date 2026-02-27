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
use reth_evm::{execute::Executor, ConfigureEvm, Evm};
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

/// Finds the failing transaction within a block by re-executing transactions individually,
/// then traces it at the opcode level.
///
/// Used when `execute_one` fails for a whole block and we don't know which transaction caused it.
/// `make_db` is called to build a fresh state provider database at the parent block.
fn find_and_trace_failing_tx<C, DB>(
    evm_config: &C,
    make_db: impl Fn() -> StateProviderDatabase<DB>,
    block: &reth_primitives_traits::RecoveredBlock<<C::Primitives as NodePrimitives>::Block>,
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

    // Execute transactions one by one to find which one fails.
    let mut state = State::builder().with_database(make_db()).with_bundle_update().build();
    let mut evm = evm_config.evm_with_env(&mut state, evm_env);

    let mut failing_index = None;
    for (i, tx) in block.transactions_recovered().enumerate() {
        let tx_env = evm_config.tx_env(tx);
        if evm.transact_commit(tx_env).is_err() {
            failing_index = Some(i);
            break;
        }
    }
    drop(evm);
    drop(state);

    match failing_index {
        Some(tx_index) => {
            trace_failed_transaction(evm_config, make_db(), block, tx_index);
        }
        None => {
            error!(
                block_number = block.number(),
                "Could not locate failing transaction within block during tracing"
            );
        }
    }
}

/// Traces a failed transaction at the given index within a block, producing opcode-level output.
///
/// Builds state at the parent block, replays all prior transactions, then re-executes the
/// failing transaction with a [`TracingInspector`] attached to capture opcode-level detail.
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

    // Replay all transactions before the failing one to build up state.
    {
        let mut evm = evm_config.evm_with_env(&mut state, evm_env.clone());
        for (i, tx) in block.transactions_recovered().enumerate() {
            if i >= tx_index {
                break;
            }
            let tx_env = evm_config.tx_env(tx);
            if let Err(err) = evm.transact_commit(tx_env) {
                error!(index = i, %err, "Failed to replay transaction before failing tx");
                return;
            }
        }
    }

    // Execute the failing transaction with a tracing inspector.
    let tx = match block.transactions_recovered().nth(tx_index) {
        Some(tx) => tx,
        None => {
            error!(tx_index, "Transaction index out of bounds for opcode tracing");
            return;
        }
    };
    let tx_env = evm_config.tx_env(tx);
    let mut inspector = TracingInspector::new(TracingInspectorConfig::all());
    let result = {
        let mut evm =
            evm_config.evm_with_env_and_inspector(&mut state, evm_env, &mut inspector);
        evm.transact(tx_env)
    };

    // Build geth-style trace with opcode-level structLogs.
    let (gas_used, return_value) = match &result {
        Ok(r) => (r.result.gas_used(), r.result.output().cloned().unwrap_or_default()),
        Err(_) => (0, Default::default()),
    };

    let frame = inspector
        .into_geth_builder()
        .geth_traces(gas_used, return_value, Default::default());

    match serde_json::to_string(&frame) {
        Ok(json) => {
            error!(
                block_number = block.number(),
                block_hash = ?block.hash(),
                tx_index,
                tx_hash = ?block.body().transactions()[tx_index].tx_hash(),
                execution_result = ?result.as_ref().map(|r| &r.result),
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
                                // Find which tx failed and trace it at the opcode
                                // level. Re-execute txs one by one to locate the
                                // failing index, then trace that tx with an inspector.
                                let parent_num = block.number() - 1;
                                find_and_trace_failing_tx(
                                    &evm_config, || db_at(parent_num), &block,
                                );

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
