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
use reth_evm::{database::StateProviderDatabase, execute::Executor, ConfigureEvm};
use reth_execution_types::{BlockReverts, RevertAccount};
use reth_node_core::args::JitArgs;
use reth_primitives_traits::{format_gas_throughput, Account, BlockBody, GotExpected};
use reth_provider::{
    BlockNumReader, BlockReader, ChainSpecProvider, DatabaseProviderFactory, ReceiptProvider,
    StaticFileProviderFactory, TransactionVariant,
};
use reth_stages::stages::calculate_gas_used_from_headers;
use reth_storage_api::{ChangeSetReader, DBProvider, StorageChangeSetReader};
use std::{
    collections::HashMap,
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

    #[command(flatten)]
    pub jit: JitArgs,
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
        mut self,
        components: impl CliComponentsBuilder<N>,
        runtime: reth_tasks::Runtime,
    ) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
    {
        // Default to 4GB RocksDB block cache for re-execute unless explicitly set.
        if self.env.db.rocksdb_block_cache_size.is_none() {
            self.env.db.rocksdb_block_cache_size = Some(4 << 30);
        }

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
            let stats_tx = stats_tx.clone();
            let info_tx = info_tx.clone();
            let cancellation = cancellation.clone();
            let next_block = Arc::clone(&next_block);
            tasks.spawn_blocking(move || {
                let evm_config = evm_config.with_jit_support();
                let executor_lifetime = Duration::from_secs(600);
                let provider = provider_factory.database_provider_ro()?.disable_long_read_transaction_safety();

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

                    let database = StateProviderDatabase::new(
                        provider.history_by_block_number(chunk_start.saturating_sub(1))?,
                    );
                    let mut executor = evm_config.batch_executor(database);
                    let mut last_executed_block = None;
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
                                    drop(executor);
                                    let database = StateProviderDatabase::new(
                                        provider.history_by_block_number(block.number())?,
                                    );
                                    executor = evm_config.batch_executor(database);
                                    executor_created = Instant::now();
                                    let _ = info_tx.send((block, eyre::Report::new(err)));
                                    continue
                                }
                                return Err(eyre::Report::new(err))
                            }
                        };
                        if let Err(err) = consensus
                            .validate_block_post_execution(&block, &result, None,None)
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
                                            drop(executor);
                                            let database = StateProviderDatabase::new(
                                                provider.history_by_block_number(block.number())?,
                                            );
                                            executor = evm_config.batch_executor(database);
                                            executor_created = Instant::now();
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
                        let _ = stats_tx.send((block.number(), block.gas_used()));
                        last_executed_block = Some(block.number());

                        // Verify and drop accumulated state once in a while to avoid OOM.
                        if executor.size_hint() > 5_000_000 ||
                            executor_created.elapsed() > executor_lifetime
                        {
                            let last_block = block.number();
                            let database = StateProviderDatabase::new(
                                provider.history_by_block_number(last_block)?,
                            );
                            let next_executor = evm_config.batch_executor(database);
                            let old_executor = std::mem::replace(&mut executor, next_executor);
                            let state = old_executor.into_state();
                            verify_reverts_against_changesets(
                                &provider,
                                state.block_reverts(),
                                last_block,
                            )?;
                            executor_created = Instant::now();
                        }
                    }

                    // Full verification at chunk end for remaining unverified blocks
                    let state = executor.into_state();
                    if !state.block_reverts().is_empty() {
                        let last_block = last_executed_block.unwrap_or(chunk_end.saturating_sub(1));
                        verify_reverts_against_changesets(
                            &provider,
                            state.block_reverts(),
                            last_block,
                        )?;
                    }
                }

                eyre::Ok(())
            });
        }

        let instant = Instant::now();
        let mut total_executed_blocks = 0;
        let mut total_executed_gas = 0;
        let mut latest_executed_block = None;

        let mut last_logged_gas = 0;
        let mut last_logged_blocks = 0;
        let mut last_logged_time = Instant::now();
        let mut invalid_blocks = Vec::new();

        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                Some((block_number, gas_used)) = stats_rx.recv() => {
                    total_executed_blocks += 1;
                    total_executed_gas += gas_used;
                    latest_executed_block =
                        Some(latest_executed_block.unwrap_or(block_number).max(block_number));
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
                            ?latest_executed_block,
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
                ?latest_executed_block,
                throughput=?format_gas_throughput(total_executed_gas, instant.elapsed()),
                "Re-executed successfully"
            );
        } else {
            info!(
                start_block = min_block,
                end_block = max_block,
                %total_executed_blocks,
                ?latest_executed_block,
                invalid_block_count = invalid_blocks.len(),
                ?invalid_blocks,
                throughput=?format_gas_throughput(total_executed_gas, instant.elapsed()),
                "Re-executed with invalid blocks"
            );
        }

        Ok(())
    }
}

/// Verifies reverts against database changesets.
///
/// For each block, reverts must match changeset entries exactly. No extra slots/accounts
/// in reverts for non-destroyed accounts. Destroyed accounts may have extra changeset slots
/// (from DB storage wipe) absent from reverts.
fn verify_reverts_against_changesets<P>(
    provider: &P,
    reverts: &[BlockReverts],
    last_block: u64,
) -> eyre::Result<()>
where
    P: ChangeSetReader + StorageChangeSetReader,
{
    // Verify reverts against changesets per block
    for (i, block_reverts) in reverts.iter().rev().enumerate() {
        let block_number = last_block - i as u64;

        let mut cs_accounts: HashMap<Address, Option<Account>> = provider
            .account_block_changeset(block_number)?
            .into_iter()
            .map(|cs| (cs.address, cs.info))
            .collect();

        let mut cs_storage: HashMap<Address, HashMap<B256, U256>> = HashMap::new();
        for (bna, entry) in provider.storage_changeset(block_number)? {
            cs_storage.entry(bna.address()).or_default().insert(entry.key, entry.value);
        }

        for (addr, original) in &block_reverts.accounts {
            let cs_info = cs_accounts.remove(addr).ok_or_else(|| {
                eyre::eyre!("Block {block_number}: account {addr} in reverts but not in changeset")
            })?;
            let revert_acct = original.as_ref().map(account_from_revert);
            eyre::ensure!(
                revert_acct == cs_info,
                "Block {block_number}: account {addr} info mismatch: revert={revert_acct:?} cs={cs_info:?}",
            );
        }

        for (addr, revert) in &block_reverts.storage {
            // Verify storage slots — remove matched changeset entries as we go
            let mut cs_slots = cs_storage.get_mut(addr);
            for (slot_key, prev) in &revert.slots {
                let b256_key = B256::from(*slot_key);
                let cs_value = cs_slots.as_mut().and_then(|s| s.remove(&b256_key));
                match cs_value {
                    Some(cs_value) => eyre::ensure!(
                        *prev == cs_value,
                        "Block {block_number}: {addr} slot {b256_key} mismatch: \
                         revert={prev} cs={cs_value}",
                    ),
                    None => eyre::ensure!(
                        revert.wiped,
                        "Block {block_number}: {addr} slot {b256_key} in reverts but not in changeset",
                    ),
                }
            }

            // Any remaining cs_storage slots for this address must be from a storage wipe
            if let Some(remaining) = cs_slots.filter(|s| !s.is_empty()) {
                eyre::ensure!(
                    revert.wiped,
                    "Block {block_number}: {addr} has {} unmatched storage slots in changeset",
                    remaining.len(),
                );
            }
        }

        // Any remaining cs_accounts entries had no corresponding revert
        if let Some(addr) = cs_accounts.keys().next() {
            eyre::bail!("Block {block_number}: account {addr} in changeset but not in reverts");
        }
    }

    Ok(())
}

fn account_from_revert(info: &RevertAccount) -> Account {
    Account {
        nonce: info.nonce,
        balance: info.balance,
        bytecode_hash: (!info.code_hash.is_zero()).then_some(info.code_hash),
    }
}
