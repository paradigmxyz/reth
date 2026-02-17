use clap::Parser;
use metrics::{self, Counter};
use reth_chainspec::EthChainSpec;
use reth_cli_util::parse_socket_address;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    transaction::{DbTx, DbTxMut},
};
use reth_db_common::DbTool;
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    version::version_metadata,
};
use reth_node_metrics::{
    chain::ChainSpecInfo,
    hooks::Hooks,
    server::{MetricServer, MetricServerConfig},
    version::VersionInfo,
};
use reth_provider::{providers::ProviderNodeTypes, ChainSpecProvider, StageCheckpointReader};
use reth_stages::StageId;
use reth_storage_api::StorageSettingsCache;
use reth_tasks::TaskExecutor;
use reth_trie::{
    verify::{Output, Verifier},
    Nibbles,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, StorageTrieEntryLike, TrieKeyAdapter,
    TrieTableAdapter,
};
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};
use tracing::{info, warn};

const PROGRESS_PERIOD: Duration = Duration::from_secs(5);

/// The arguments for the `reth db repair-trie` command
#[derive(Parser, Debug)]
pub struct Command {
    /// Only show inconsistencies without making any repairs
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long = "metrics", value_name = "ADDR:PORT", value_parser = parse_socket_address)]
    pub(crate) metrics: Option<SocketAddr>,
}

impl Command {
    /// Execute `db repair-trie` command
    pub fn execute<N: ProviderNodeTypes>(
        self,
        tool: &DbTool<N>,
        task_executor: TaskExecutor,
        data_dir: &ChainPath<DataDirPath>,
    ) -> eyre::Result<()> {
        // Set up metrics server if requested
        let _metrics_handle = if let Some(listen_addr) = self.metrics {
            let chain_name = tool.provider_factory.chain_spec().chain().to_string();
            let executor = task_executor.clone();
            let pprof_dump_dir = data_dir.pprof_dumps();

            let handle = task_executor.spawn_critical_task("metrics server", async move {
                let config = MetricServerConfig::new(
                    listen_addr,
                    VersionInfo {
                        version: version_metadata().cargo_pkg_version.as_ref(),
                        build_timestamp: version_metadata().vergen_build_timestamp.as_ref(),
                        cargo_features: version_metadata().vergen_cargo_features.as_ref(),
                        git_sha: version_metadata().vergen_git_sha.as_ref(),
                        target_triple: version_metadata().vergen_cargo_target_triple.as_ref(),
                        build_profile: version_metadata().build_profile_name.as_ref(),
                    },
                    ChainSpecInfo { name: chain_name },
                    executor,
                    Hooks::builder().build(),
                    pprof_dump_dir,
                );

                // Spawn the metrics server
                if let Err(e) = MetricServer::new(config).serve().await {
                    tracing::error!("Metrics server error: {}", e);
                }
            });

            Some(handle)
        } else {
            None
        };

        if self.dry_run {
            verify_only(tool)?
        } else {
            verify_and_repair(tool)?
        }

        Ok(())
    }
}

fn verify_only<N: ProviderNodeTypes>(tool: &DbTool<N>) -> eyre::Result<()> {
    // Log the database block tip from Finish stage checkpoint
    let finish_checkpoint = tool
        .provider_factory
        .provider()?
        .get_stage_checkpoint(StageId::Finish)?
        .unwrap_or_default();
    info!("Database block tip: {}", finish_checkpoint.block_number);

    // Get a database transaction directly from the database
    let db = tool.provider_factory.db_ref();
    let mut tx = db.tx()?;
    tx.disable_long_read_transaction_safety();

    let is_v2 = tool.provider_factory.cached_storage_settings().is_v2();

    reth_trie_db::dispatch_trie_adapter!(is_v2, |A| {
        // Create the verifier
        let hashed_cursor_factory = DatabaseHashedCursorFactory::new(&tx);
        let trie_cursor_factory = DatabaseTrieCursorFactory::<_, A>::new(&tx);
        let verifier = Verifier::new(&trie_cursor_factory, hashed_cursor_factory)?;

        let metrics = RepairTrieMetrics::new();

        let mut inconsistent_nodes = 0;
        let start_time = Instant::now();
        let mut last_progress_time = Instant::now();

        // Iterate over the verifier and repair inconsistencies
        for output_result in verifier {
            let output = output_result?;

            if let Output::Progress(path) = output {
                if last_progress_time.elapsed() > PROGRESS_PERIOD {
                    output_progress(path, start_time, inconsistent_nodes);
                    last_progress_time = Instant::now();
                }
            } else {
                warn!("Inconsistency found: {output:?}");
                inconsistent_nodes += 1;

                // Record metrics based on output type
                match output {
                    Output::AccountExtra(_, _) |
                    Output::AccountWrong { .. } |
                    Output::AccountMissing(_, _) => {
                        metrics.account_inconsistencies.increment(1);
                    }
                    Output::StorageExtra(_, _, _) |
                    Output::StorageWrong { .. } |
                    Output::StorageMissing(_, _, _) => {
                        metrics.storage_inconsistencies.increment(1);
                    }
                    Output::Progress(_) => unreachable!(),
                }
            }
        }

        info!("Found {} inconsistencies (dry run - no changes made)", inconsistent_nodes);
    });

    Ok(())
}

/// Checks that the merkle stage has completed running up to the account and storage hashing stages.
fn verify_checkpoints(provider: impl StageCheckpointReader) -> eyre::Result<()> {
    let account_hashing_checkpoint =
        provider.get_stage_checkpoint(StageId::AccountHashing)?.unwrap_or_default();
    let storage_hashing_checkpoint =
        provider.get_stage_checkpoint(StageId::StorageHashing)?.unwrap_or_default();
    let merkle_checkpoint =
        provider.get_stage_checkpoint(StageId::MerkleExecute)?.unwrap_or_default();

    if account_hashing_checkpoint.block_number != merkle_checkpoint.block_number {
        return Err(eyre::eyre!(
            "MerkleExecute stage checkpoint ({}) != AccountHashing stage checkpoint ({}), you must first complete the pipeline sync by running `reth node`",
            merkle_checkpoint.block_number,
            account_hashing_checkpoint.block_number,
        ))
    }

    if storage_hashing_checkpoint.block_number != merkle_checkpoint.block_number {
        return Err(eyre::eyre!(
            "MerkleExecute stage checkpoint ({}) != StorageHashing stage checkpoint ({}), you must first complete the pipeline sync by running `reth node`",
            merkle_checkpoint.block_number,
            storage_hashing_checkpoint.block_number,
        ))
    }

    let merkle_checkpoint_progress =
        provider.get_stage_checkpoint_progress(StageId::MerkleExecute)?;
    if merkle_checkpoint_progress.is_some_and(|progress| !progress.is_empty()) {
        return Err(eyre::eyre!(
            "MerkleExecute sync stage in-progress, you must first complete the pipeline sync by running `reth node`",
        ))
    }

    Ok(())
}

fn verify_and_repair<N: ProviderNodeTypes>(tool: &DbTool<N>) -> eyre::Result<()> {
    // Get a read-write database provider
    let mut provider_rw = tool.provider_factory.provider_rw()?;

    // Log the database block tip from Finish stage checkpoint
    let finish_checkpoint = provider_rw.get_stage_checkpoint(StageId::Finish)?.unwrap_or_default();
    info!("Database block tip: {}", finish_checkpoint.block_number);

    // Check that a pipeline sync isn't in progress.
    verify_checkpoints(provider_rw.as_ref())?;

    let is_v2 = tool.provider_factory.cached_storage_settings().is_v2();

    reth_trie_db::dispatch_trie_adapter!(is_v2, |A| {
        // Create cursors for making modifications with
        let tx = provider_rw.tx_mut();
        tx.disable_long_read_transaction_safety();
        let mut account_trie_cursor =
            tx.cursor_write::<<A as TrieTableAdapter>::AccountTrieTable>()?;
        let mut storage_trie_cursor =
            tx.cursor_dup_write::<<A as TrieTableAdapter>::StorageTrieTable>()?;

        // Create the cursor factories. These cannot accept the `&mut` tx above because they
        // require it to be AsRef.
        let tx = provider_rw.tx_ref();
        let hashed_cursor_factory = DatabaseHashedCursorFactory::new(tx);
        let trie_cursor_factory = DatabaseTrieCursorFactory::<_, A>::new(tx);

        // Create the verifier
        let verifier = Verifier::new(&trie_cursor_factory, hashed_cursor_factory)?;

        let metrics = RepairTrieMetrics::new();

        let mut inconsistent_nodes = 0;
        let start_time = Instant::now();
        let mut last_progress_time = Instant::now();

        // Iterate over the verifier and repair inconsistencies
        for output_result in verifier {
            let output = output_result?;

            if !matches!(output, Output::Progress(_)) {
                warn!("Inconsistency found, will repair: {output:?}");
                inconsistent_nodes += 1;

                // Record metrics based on output type
                match &output {
                    Output::AccountExtra(_, _) |
                    Output::AccountWrong { .. } |
                    Output::AccountMissing(_, _) => {
                        metrics.account_inconsistencies.increment(1);
                    }
                    Output::StorageExtra(_, _, _) |
                    Output::StorageWrong { .. } |
                    Output::StorageMissing(_, _, _) => {
                        metrics.storage_inconsistencies.increment(1);
                    }
                    Output::Progress(_) => {}
                }
            }

            match output {
                Output::AccountExtra(path, _node) => {
                    // Extra account node in trie, remove it
                    let key: <A as TrieKeyAdapter>::AccountKey = path.into();
                    if account_trie_cursor.seek_exact(key)?.is_some() {
                        account_trie_cursor.delete_current()?;
                    }
                }
                Output::StorageExtra(account, path, _node) => {
                    // Extra storage node in trie, remove it
                    let subkey: <A as TrieKeyAdapter>::StorageSubKey = path.into();
                    if storage_trie_cursor
                        .seek_by_key_subkey(account, subkey.clone())?
                        .filter(|e| *e.nibbles() == subkey)
                        .is_some()
                    {
                        storage_trie_cursor.delete_current()?;
                    }
                }
                Output::AccountWrong { path, expected: node, .. } |
                Output::AccountMissing(path, node) => {
                    // Wrong/missing account node value, upsert it
                    let key: <A as TrieKeyAdapter>::AccountKey = path.into();
                    account_trie_cursor.upsert(key, &node)?;
                }
                Output::StorageWrong { account, path, expected: node, .. } |
                Output::StorageMissing(account, path, node) => {
                    // Wrong/missing storage node value, upsert it
                    // (We can't just use `upsert` method with a dup cursor, it's not properly
                    // supported)
                    let subkey: <A as TrieKeyAdapter>::StorageSubKey = path.into();
                    let entry = <A as TrieKeyAdapter>::StorageValue::new(subkey.clone(), node);
                    if storage_trie_cursor
                        .seek_by_key_subkey(account, subkey.clone())?
                        .filter(|v| *v.nibbles() == subkey)
                        .is_some()
                    {
                        storage_trie_cursor.delete_current()?;
                    }
                    storage_trie_cursor.upsert(account, &entry)?;
                }
                Output::Progress(path) => {
                    if last_progress_time.elapsed() > PROGRESS_PERIOD {
                        output_progress(path, start_time, inconsistent_nodes);
                        last_progress_time = Instant::now();
                    }
                }
            }
        }

        if inconsistent_nodes == 0 {
            info!("No inconsistencies found");
        } else {
            provider_rw.commit()?;
            info!("Repaired {} inconsistencies and committed changes", inconsistent_nodes);
        }
    });

    Ok(())
}

/// Output progress information based on the last seen account path.
fn output_progress(last_account: Nibbles, start_time: Instant, inconsistent_nodes: u64) {
    // Calculate percentage based on position in the trie path space
    // For progress estimation, we'll use the first few nibbles as an approximation

    // Convert the first 16 nibbles (8 bytes) to a u64 for progress calculation
    let mut current_value: u64 = 0;
    let nibbles_to_use = last_account.len().min(16);

    for i in 0..nibbles_to_use {
        current_value = (current_value << 4) | (last_account.get(i).unwrap_or(0) as u64);
    }
    // Shift left to fill remaining bits if we have fewer than 16 nibbles
    if nibbles_to_use < 16 {
        current_value <<= (16 - nibbles_to_use) * 4;
    }

    let progress_percent = current_value as f64 / u64::MAX as f64 * 100.0;
    let progress_percent_str = format!("{progress_percent:.2}");

    // Calculate ETA based on current speed
    let elapsed = start_time.elapsed();
    let elapsed_secs = elapsed.as_secs_f64();

    let estimated_total_time =
        if progress_percent > 0.0 { elapsed_secs / (progress_percent / 100.0) } else { 0.0 };
    let remaining_time = estimated_total_time - elapsed_secs;
    let eta_duration = Duration::from_secs(remaining_time as u64);

    info!(
        progress_percent = progress_percent_str,
        eta = %humantime::format_duration(eta_duration),
        inconsistent_nodes,
        "Repairing trie tables",
    );
}

/// Metrics for tracking trie repair inconsistencies
#[derive(Debug)]
struct RepairTrieMetrics {
    account_inconsistencies: Counter,
    storage_inconsistencies: Counter,
}

impl RepairTrieMetrics {
    fn new() -> Self {
        Self {
            account_inconsistencies: metrics::counter!(
                "db.repair_trie.inconsistencies_found",
                "type" => "account"
            ),
            storage_inconsistencies: metrics::counter!(
                "db.repair_trie.inconsistencies_found",
                "type" => "storage"
            ),
        }
    }
}
