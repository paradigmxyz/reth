//! Re-execute blocks from database in parallel.

use crate::common::{
    AccessRights, CliComponentsBuilder, CliNodeComponents, CliNodeTypes, Environment,
    EnvironmentArgs,
};
use alloy_consensus::{transaction::TxHashRef, BlockHeader, TxReceipt};
use alloy_primitives::{Address, B256, U256};
use clap::Parser;
use eyre::WrapErr;
use reth_chainspec::{EthChainSpec, EthereumHardforks, Hardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_util::cancellation::CancellationToken;
use reth_consensus::FullConsensus;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{format_gas_throughput, Account, BlockBody, GotExpected};
use reth_provider::{
    BlockNumReader, BlockReader, ChainSpecProvider, ChangeSetReader, DatabaseProviderFactory,
    ReceiptProvider, StaticFileProviderFactory, StorageChangeSetReader, TransactionVariant,
};
use reth_revm::{
    database::StateProviderDatabase,
    revm::database::states::{reverts::AccountInfoRevert, RevertToSlot},
};
use reth_stages::stages::calculate_gas_used_from_headers;
use std::{
    collections::BTreeMap,
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

    /// Compare execution state outputs against stored changesets.
    ///
    /// For each block, compares the account and storage changes produced by execution
    /// against the changesets stored in the database. Uses per-block execution instead
    /// of batching when enabled.
    #[arg(long)]
    compare_changesets: bool,
}

impl<C: ChainSpecParser> Command<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
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
        let compare_changesets = self.compare_changesets;
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

                        if compare_changesets {
                            // Use per-block execution to get the BundleState for comparison.
                            let block_executor = evm_config.executor(db_at(block.number() - 1));
                            let output = match block_executor.execute(&block) {
                                Ok(output) => output,
                                Err(err) => {
                                    if skip_invalid_blocks {
                                        let _ =
                                            info_tx.send((block, eyre::Report::new(err)));
                                        continue
                                    }
                                    return Err(err.into())
                                }
                            };

                            if let Err(err) = consensus
                                .validate_block_post_execution(&block, &output.result, None)
                                .wrap_err_with(|| {
                                    format!(
                                        "Failed to validate block {} {}",
                                        block.number(),
                                        block.hash()
                                    )
                                })
                            {
                                if skip_invalid_blocks {
                                    let _ = info_tx.send((block, err));
                                    continue
                                }
                                return Err(err);
                            }

                            // Compare state changes against stored changesets.
                            let provider =
                                DatabaseProviderFactory::database_provider_ro(&provider_factory)?;

                            if let Err(err) =
                                compare_state_with_changesets(&output.state, block.number(), &provider)
                            {
                                error!(
                                    number = block.number(),
                                    %err,
                                    "Changeset mismatch"
                                );
                                if skip_invalid_blocks {
                                    let _ = info_tx.send((
                                        block,
                                        eyre::eyre!("Changeset mismatch: {err}"),
                                    ));
                                    continue
                                }
                                return Err(eyre::eyre!(
                                    "Changeset mismatch at block {}: {err}",
                                    block.number()
                                ));
                            }

                            let _ = stats_tx.send(block.gas_used());
                            continue;
                        }

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

/// Compares the execution state output (`BundleState`) against the changesets stored in the
/// database for the given block number.
///
/// The `BundleState` reverts represent the "previous" values before the block was applied,
/// which should match exactly what the database stores as changesets.
fn compare_state_with_changesets(
    bundle: &reth_revm::revm::database::BundleState,
    block_number: u64,
    provider: &impl ProviderWithChangesets,
) -> eyre::Result<()> {
    let reverts = &bundle.reverts;
    if reverts.len() != 1 {
        eyre::bail!(
            "expected exactly 1 revert frame from single-block execution, got {}",
            reverts.len()
        );
    }

    let block_reverts = &reverts[0];

    // --- Account changeset comparison ---
    let db_account_changesets = provider
        .account_block_changeset(block_number)
        .wrap_err("failed to read account changesets from database")?;

    // Build normalized maps: Address -> Option<Account>
    // DoNothing means account info wasn't changed, so no entry in the changeset.
    // DeleteIt means the account was created in this block; reverting means removing it (None).
    // RevertTo(info) means the account existed before with the given info.
    let mut exec_accounts: BTreeMap<Address, Option<Account>> = BTreeMap::new();
    for (address, revert) in block_reverts {
        match &revert.account {
            AccountInfoRevert::DoNothing => {}
            AccountInfoRevert::DeleteIt => {
                exec_accounts.insert(*address, None);
            }
            AccountInfoRevert::RevertTo(info) => {
                exec_accounts.insert(*address, Some(info.into()));
            }
        }
    }

    let mut db_accounts: BTreeMap<Address, Option<Account>> = BTreeMap::new();
    for changeset in &db_account_changesets {
        db_accounts.insert(changeset.address, changeset.info);
    }

    if exec_accounts != db_accounts {
        let only_in_exec: Vec<_> =
            exec_accounts.keys().filter(|k| !db_accounts.contains_key(*k)).collect();
        let only_in_db: Vec<_> =
            db_accounts.keys().filter(|k| !exec_accounts.contains_key(*k)).collect();
        let mismatched: Vec<_> = exec_accounts
            .iter()
            .filter(|(k, v)| db_accounts.get(*k).is_some_and(|db_v| db_v != *v))
            .map(|(k, _)| k)
            .collect();

        eyre::bail!(
            "account changeset mismatch: \
             only_in_execution={only_in_exec:?}, \
             only_in_db={only_in_db:?}, \
             value_mismatch={mismatched:?}"
        );
    }

    // --- Storage changeset comparison ---
    let db_storage_changesets = provider
        .storage_changeset(block_number)
        .wrap_err("failed to read storage changesets from database")?;

    // Build normalized map: (Address, B256 key) -> U256 value
    let mut exec_storage: BTreeMap<(Address, B256), U256> = BTreeMap::new();
    for (address, revert) in block_reverts {
        for (slot, value) in &revert.storage {
            let key = B256::from(slot.to_be_bytes());
            let prev_value = match value {
                RevertToSlot::Some(v) => *v,
                RevertToSlot::Destroyed => U256::ZERO,
            };
            exec_storage.insert((*address, key), prev_value);
        }
    }

    let mut db_storage: BTreeMap<(Address, B256), U256> = BTreeMap::new();
    for (block_number_address, entry) in &db_storage_changesets {
        db_storage.insert((block_number_address.address(), entry.key), entry.value);
    }

    if exec_storage != db_storage {
        let only_in_exec: Vec<_> =
            exec_storage.keys().filter(|k| !db_storage.contains_key(*k)).collect();
        let only_in_db: Vec<_> =
            db_storage.keys().filter(|k| !exec_storage.contains_key(*k)).collect();
        let mismatched: Vec<_> = exec_storage
            .iter()
            .filter(|(k, v)| db_storage.get(*k).is_some_and(|db_v| db_v != *v))
            .map(|(k, _)| k)
            .collect();

        eyre::bail!(
            "storage changeset mismatch: \
             only_in_execution={only_in_exec:?}, \
             only_in_db={only_in_db:?}, \
             value_mismatch={mismatched:?}"
        );
    }

    Ok(())
}

/// Helper trait combining the changeset reader traits needed for comparison.
trait ProviderWithChangesets: ChangeSetReader + StorageChangeSetReader {}
impl<T: ChangeSetReader + StorageChangeSetReader> ProviderWithChangesets for T {}
