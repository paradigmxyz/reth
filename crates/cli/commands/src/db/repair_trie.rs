use alloy_consensus::EMPTY_ROOT_HASH;
use alloy_primitives::B256;
use alloy_rlp::{BufMut, Encodable};
use clap::Parser;
use metrics::{self, Counter};
use reth_chainspec::EthChainSpec;
use reth_cli_util::parse_socket_address;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    tables,
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
use reth_storage_api::{DBProvider, StorageSettingsCache};
use reth_tasks::TaskExecutor;
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    metrics::TrieRootMetrics,
    node_iter::{TrieElement, TrieNodeIter},
    trie_cursor::{depth_first, DepthFirstTrieIterator, TrieCursor, TrieCursorFactory},
    updates::TrieUpdates,
    verify::Output,
    walker::TrieWalker,
    BranchNodeCompact, HashBuilder, Nibbles, StorageRoot, TrieType, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, StorageTrieEntryLike, TrieTableAdapter,
};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    net::SocketAddr,
    num::NonZeroUsize,
    sync::{mpsc, Arc, Mutex},
    thread,
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
    let finish_checkpoint = tool
        .provider_factory
        .provider()?
        .get_stage_checkpoint(StageId::Finish)?
        .unwrap_or_default();
    info!("Database block tip: {}", finish_checkpoint.block_number);

    let metrics = RepairTrieMetrics::new();
    let mut inconsistent_nodes = 0;
    let start_time = Instant::now();
    let mut last_progress_time = Instant::now();

    reth_trie_db::with_adapter!(tool.provider_factory, |A| {
        parallel_verify::<A, _, _, _>(
            &|| Ok(tool.provider_factory.provider()?.disable_long_read_transaction_safety()),
            |output| {
                if let Output::Progress(path) = output {
                    if last_progress_time.elapsed() > PROGRESS_PERIOD {
                        output_progress(path, start_time, inconsistent_nodes);
                        last_progress_time = Instant::now();
                    }
                } else {
                    warn!("Inconsistency found: {output:?}");
                    inconsistent_nodes += 1;

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

                Ok(())
            },
        )
    })?;

    info!("Found {} inconsistencies (dry run - no changes made)", inconsistent_nodes);

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
    let mut provider_rw = tool.provider_factory.provider_rw()?;

    let finish_checkpoint = provider_rw.get_stage_checkpoint(StageId::Finish)?.unwrap_or_default();
    info!("Database block tip: {}", finish_checkpoint.block_number);

    verify_checkpoints(provider_rw.as_ref())?;

    let inconsistent_nodes = reth_trie_db::with_adapter!(tool.provider_factory, |A| {
        do_verify_and_repair::<_, A, _, _>(&mut provider_rw, &|| {
            Ok(tool.provider_factory.provider()?.disable_long_read_transaction_safety())
        })
    })?;

    if inconsistent_nodes == 0 {
        info!("No inconsistencies found");
    } else {
        provider_rw.commit()?;
        info!("Repaired {} inconsistencies and committed changes", inconsistent_nodes);
    }

    Ok(())
}

fn do_verify_and_repair<N: ProviderNodeTypes, A: TrieTableAdapter + Send, P, F>(
    provider_rw: &mut reth_provider::DatabaseProviderRW<N::DB, N>,
    make_provider: &F,
) -> eyre::Result<usize>
where
    P: DBProvider + Send,
    P::Tx: DbTx,
    F: Fn() -> eyre::Result<P> + Sync,
    <N::DB as Database>::TXMut: DbTxMut + DbTx,
{
    let tx = provider_rw.tx_mut();
    tx.disable_long_read_transaction_safety();
    let mut account_trie_cursor = tx.cursor_write::<A::AccountTrieTable>()?;
    let mut storage_trie_cursor = tx.cursor_dup_write::<A::StorageTrieTable>()?;

    let metrics = RepairTrieMetrics::new();
    let mut inconsistent_nodes = 0;
    let start_time = Instant::now();
    let mut last_progress_time = Instant::now();

    parallel_verify::<A, _, _, _>(make_provider, |output| {
        if !matches!(output, Output::Progress(_)) {
            warn!("Inconsistency found, will repair: {output:?}");
            inconsistent_nodes += 1;

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
                let key: A::AccountKey = path.into();
                if account_trie_cursor.seek_exact(key)?.is_some() {
                    account_trie_cursor.delete_current()?;
                }
            }
            Output::StorageExtra(account, path, _node) => {
                let subkey: A::StorageSubKey = path.into();
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
                let key: A::AccountKey = path.into();
                account_trie_cursor.upsert(key, &node)?;
            }
            Output::StorageWrong { account, path, expected: node, .. } |
            Output::StorageMissing(account, path, node) => {
                let subkey: A::StorageSubKey = path.into();
                let entry = A::StorageValue::new(subkey.clone(), node);
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

        Ok(())
    })?;

    Ok(inconsistent_nodes as usize)
}

type StorageResult = eyre::Result<StorageMessage>;

// Each account's storage result stream must come from exactly one producer, and that producer must
// send every inconsistency before it sends the final root for the account. `wait_for_storage_root`
// relies on that ordering when it buffers future-account messages.
#[derive(Debug)]
enum StorageMessage {
    Root { account: B256, root: B256 },
    Output(Output),
}

#[derive(Debug)]
struct RepairTrieCursorVerifier<I> {
    account: Option<B256>,
    trie_iter: I,
    curr: Option<(Nibbles, BranchNodeCompact)>,
}

impl<C: TrieCursor> RepairTrieCursorVerifier<DepthFirstTrieIterator<C>> {
    fn new(account: Option<B256>, trie_cursor: C) -> eyre::Result<Self> {
        let mut trie_iter = DepthFirstTrieIterator::new(trie_cursor);
        let curr = trie_iter.next().transpose()?;
        Ok(Self { account, trie_iter, curr })
    }

    const fn output_extra(&self, path: Nibbles, node: BranchNodeCompact) -> Output {
        if let Some(account) = self.account {
            Output::StorageExtra(account, path, node)
        } else {
            Output::AccountExtra(path, node)
        }
    }

    const fn output_wrong(
        &self,
        path: Nibbles,
        expected: BranchNodeCompact,
        found: BranchNodeCompact,
    ) -> Output {
        if let Some(account) = self.account {
            Output::StorageWrong { account, path, expected, found }
        } else {
            Output::AccountWrong { path, expected, found }
        }
    }

    const fn output_missing(&self, path: Nibbles, node: BranchNodeCompact) -> Output {
        if let Some(account) = self.account {
            Output::StorageMissing(account, path, node)
        } else {
            Output::AccountMissing(path, node)
        }
    }

    fn next(&mut self, path: Nibbles, node: BranchNodeCompact) -> eyre::Result<Vec<Output>> {
        let mut outputs = Vec::new();

        loop {
            if self.curr.is_none() {
                outputs.push(self.output_missing(path, node));
                return Ok(outputs)
            }

            let (curr_path, curr_node) = self.curr.as_ref().expect("not None");
            match depth_first::cmp(&path, curr_path) {
                Ordering::Less => {
                    outputs.push(self.output_missing(path, node));
                    return Ok(outputs)
                }
                Ordering::Equal => {
                    if *curr_node != node {
                        outputs.push(self.output_wrong(path, node, curr_node.clone()));
                    }
                    self.curr = self.trie_iter.next().transpose()?;
                    return Ok(outputs)
                }
                Ordering::Greater => {
                    outputs.push(self.output_extra(*curr_path, curr_node.clone()));
                    self.curr = self.trie_iter.next().transpose()?;
                }
            }
        }
    }

    fn finalize(&mut self) -> eyre::Result<Vec<Output>> {
        let mut outputs = Vec::new();
        while let Some((curr_path, curr_node)) = self.curr.take() {
            outputs.push(self.output_extra(curr_path, curr_node));
            self.curr = self.trie_iter.next().transpose()?;
        }
        Ok(outputs)
    }
}

fn parallel_verify<A, P, F, O>(make_provider: &F, mut on_output: O) -> eyre::Result<()>
where
    A: TrieTableAdapter + Send,
    P: DBProvider + Send,
    P::Tx: DbTx,
    F: Fn() -> eyre::Result<P> + Sync,
    O: FnMut(Output) -> eyre::Result<()>,
{
    let storage_workers = storage_worker_count();
    info!(storage_workers, "Verifying storage tries in parallel");

    let (job_tx, job_rx) = mpsc::sync_channel(storage_workers.saturating_mul(2).max(1));
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (result_tx, result_rx) = mpsc::channel();

    thread::scope(|scope| -> eyre::Result<()> {
        {
            let result_tx = result_tx.clone();
            let job_tx = job_tx.clone();
            scope.spawn(move || {
                if let Err(err) =
                    dispatch_storage_jobs::<A, P, F>(make_provider, &job_tx, &result_tx)
                {
                    let _ = result_tx.send(Err(err));
                }
            });
        }

        for _ in 0..storage_workers {
            let job_rx = Arc::clone(&job_rx);
            let result_tx = result_tx.clone();

            scope.spawn(move || {
                let provider = match make_provider() {
                    Ok(provider) => provider.disable_long_read_transaction_safety(),
                    Err(err) => {
                        let _ = result_tx.send(Err(err));
                        return;
                    }
                };

                loop {
                    let account = match job_rx.lock().expect("poisoned storage job receiver").recv()
                    {
                        Ok(account) => account,
                        Err(_) => break,
                    };

                    if let Err(err) = verify_storage_account::<A, _>(&provider, account, &result_tx)
                    {
                        let _ = result_tx.send(Err(err));
                        break;
                    }
                }
            });
        }

        drop(job_tx);
        drop(result_tx);

        verify_accounts::<A, _, _, _>(make_provider, &result_rx, &mut on_output)
    })
}

fn dispatch_storage_jobs<A, P, F>(
    make_provider: &F,
    job_tx: &mpsc::SyncSender<B256>,
    result_tx: &mpsc::Sender<StorageResult>,
) -> eyre::Result<()>
where
    A: TrieTableAdapter,
    P: DBProvider,
    P::Tx: DbTx,
    F: Fn() -> eyre::Result<P>,
{
    let provider = make_provider()?.disable_long_read_transaction_safety();
    let tx = provider.tx_ref();
    let trie_cursor_factory = DatabaseTrieCursorFactory::<_, A>::new(tx);
    let mut account_cursor = tx.cursor_read::<tables::HashedAccounts>()?;
    let mut storage_cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;

    let mut next_storage_account = storage_cursor.first()?.map(|entry| entry.0);
    let mut next_account = account_cursor.first()?;

    while let Some((account, _)) = next_account {
        while next_storage_account.is_some_and(|storage_account| storage_account < account) {
            next_storage_account = storage_cursor.next_no_dup()?.map(|entry| entry.0);
        }

        if next_storage_account == Some(account) {
            job_tx.send(account).map_err(|_| eyre::eyre!("storage worker queue closed"))?;
            next_storage_account = storage_cursor.next_no_dup()?.map(|entry| entry.0);
        } else {
            emit_empty_storage_inconsistencies(
                account,
                trie_cursor_factory.storage_trie_cursor(account)?,
                result_tx,
            )?;
            result_tx
                .send(Ok(StorageMessage::Root { account, root: EMPTY_ROOT_HASH }))
                .map_err(|_| eyre::eyre!("storage results channel closed"))?;
        }

        next_account = account_cursor.next()?;
    }

    Ok(())
}

fn verify_storage_account<A, P>(
    provider: &P,
    account: B256,
    result_tx: &mpsc::Sender<StorageResult>,
) -> eyre::Result<()>
where
    A: TrieTableAdapter,
    P: DBProvider,
    P::Tx: DbTx,
{
    let tx = provider.tx_ref();
    let trie_cursor_factory = DatabaseTrieCursorFactory::<_, A>::new(tx);
    let hashed_cursor_factory = DatabaseHashedCursorFactory::new(tx);

    let (root, _, updates) = StorageRoot::new_hashed(
        trie_cursor_factory.clone(),
        hashed_cursor_factory,
        account,
        Default::default(),
        TrieRootMetrics::new(TrieType::Storage),
    )
    .root_with_updates()?;

    emit_storage_updates(
        account,
        trie_cursor_factory.storage_trie_cursor(account)?,
        sorted_branch_nodes(updates.storage_nodes.into_iter()),
        result_tx,
    )?;

    result_tx
        .send(Ok(StorageMessage::Root { account, root }))
        .map_err(|_| eyre::eyre!("storage results channel closed"))?;

    Ok(())
}

fn verify_accounts<A, P, F, O>(
    make_provider: &F,
    result_rx: &mpsc::Receiver<StorageResult>,
    on_output: &mut O,
) -> eyre::Result<()>
where
    A: TrieTableAdapter,
    P: DBProvider,
    P::Tx: DbTx,
    F: Fn() -> eyre::Result<P>,
    O: FnMut(Output) -> eyre::Result<()>,
{
    let provider = make_provider()?.disable_long_read_transaction_safety();
    let tx = provider.tx_ref();
    let trie_cursor_factory = DatabaseTrieCursorFactory::<_, A>::new(tx);
    let hashed_cursor_factory = DatabaseHashedCursorFactory::new(tx);

    let walker: TrieWalker<_> =
        TrieWalker::state_trie(trie_cursor_factory.account_trie_cursor()?, Default::default())
            .with_deletions_retained(true);
    let mut account_node_iter =
        TrieNodeIter::state_trie(walker, hashed_cursor_factory.hashed_account_cursor()?);
    let mut hash_builder = HashBuilder::default().with_updates(true);
    let mut account_rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
    let mut pending_roots = BTreeMap::new();
    let mut pending_outputs = BTreeMap::new();

    while let Some(node) = account_node_iter.try_next()? {
        match node {
            TrieElement::Branch(node) => {
                hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
            }
            TrieElement::Leaf(hashed_address, account) => {
                let storage_root = wait_for_storage_root(
                    hashed_address,
                    result_rx,
                    &mut pending_roots,
                    &mut pending_outputs,
                    on_output,
                )?;

                account_rlp.clear();
                account.into_trie_account(storage_root).encode(&mut account_rlp as &mut dyn BufMut);
                hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);
                on_output(Output::Progress(Nibbles::unpack(hashed_address)))?;
            }
        }
    }

    let removed_keys = account_node_iter.walker.take_removed_keys();
    let mut trie_updates = TrieUpdates::default();
    trie_updates.finalize(hash_builder, removed_keys, Default::default());

    let mut account_verifier =
        RepairTrieCursorVerifier::new(None, trie_cursor_factory.account_trie_cursor()?)?;
    for (path, node) in sorted_branch_nodes(trie_updates.account_nodes.into_iter()) {
        for output in account_verifier.next(path, node)? {
            on_output(output)?;
        }
    }
    for output in account_verifier.finalize()? {
        on_output(output)?;
    }

    Ok(())
}

fn wait_for_storage_root<O>(
    account: B256,
    result_rx: &mpsc::Receiver<StorageResult>,
    pending_roots: &mut BTreeMap<B256, B256>,
    pending_outputs: &mut BTreeMap<B256, Vec<Output>>,
    on_output: &mut O,
) -> eyre::Result<B256>
where
    O: FnMut(Output) -> eyre::Result<()>,
{
    if let Some(root) = pending_roots.remove(&account) {
        flush_pending_outputs(account, pending_outputs, on_output)?;
        return Ok(root)
    }

    loop {
        // `Root` is the end-of-account marker: a producer must emit all storage inconsistencies
        // for that account before its root, so flushing buffered outputs here is sufficient.
        match result_rx.recv().map_err(|_| eyre::eyre!("storage results channel closed"))?? {
            StorageMessage::Root { account: root_account, root } => {
                if root_account == account {
                    flush_pending_outputs(account, pending_outputs, on_output)?;
                    return Ok(root)
                }

                pending_roots.insert(root_account, root);
            }
            StorageMessage::Output(output) => {
                let output_account = output_storage_account(&output)
                    .expect("storage worker emitted non-storage output");
                if output_account == account {
                    on_output(output)?;
                } else {
                    pending_outputs.entry(output_account).or_default().push(output);
                }
            }
        }
    }
}

fn flush_pending_outputs<O>(
    account: B256,
    pending_outputs: &mut BTreeMap<B256, Vec<Output>>,
    on_output: &mut O,
) -> eyre::Result<()>
where
    O: FnMut(Output) -> eyre::Result<()>,
{
    if let Some(outputs) = pending_outputs.remove(&account) {
        for output in outputs {
            on_output(output)?;
        }
    }

    Ok(())
}

fn emit_storage_updates<C>(
    account: B256,
    cursor: C,
    expected_nodes: Vec<(Nibbles, BranchNodeCompact)>,
    result_tx: &mpsc::Sender<StorageResult>,
) -> eyre::Result<()>
where
    C: TrieCursor,
{
    let mut verifier = RepairTrieCursorVerifier::new(Some(account), cursor)?;
    for (path, node) in expected_nodes {
        for output in verifier.next(path, node)? {
            result_tx
                .send(Ok(StorageMessage::Output(output)))
                .map_err(|_| eyre::eyre!("storage results channel closed"))?;
        }
    }

    for output in verifier.finalize()? {
        result_tx
            .send(Ok(StorageMessage::Output(output)))
            .map_err(|_| eyre::eyre!("storage results channel closed"))?;
    }

    Ok(())
}

fn emit_empty_storage_inconsistencies<C>(
    account: B256,
    cursor: C,
    result_tx: &mpsc::Sender<StorageResult>,
) -> eyre::Result<()>
where
    C: TrieCursor,
{
    let mut verifier = RepairTrieCursorVerifier::new(Some(account), cursor)?;
    for output in verifier.finalize()? {
        result_tx
            .send(Ok(StorageMessage::Output(output)))
            .map_err(|_| eyre::eyre!("storage results channel closed"))?;
    }

    Ok(())
}

fn sorted_branch_nodes<I>(nodes: I) -> Vec<(Nibbles, BranchNodeCompact)>
where
    I: IntoIterator<Item = (Nibbles, BranchNodeCompact)>,
{
    let mut nodes = nodes.into_iter().collect::<Vec<_>>();
    nodes.sort_unstable_by(|a, b| depth_first::cmp(&a.0, &b.0));
    nodes
}

fn output_storage_account(output: &Output) -> Option<B256> {
    match output {
        Output::StorageExtra(account, ..) |
        Output::StorageMissing(account, ..) |
        Output::StorageWrong { account, .. } => Some(*account),
        Output::AccountExtra(..) |
        Output::AccountMissing(..) |
        Output::AccountWrong { .. } |
        Output::Progress(..) => None,
    }
}

fn storage_worker_count() -> usize {
    let available = thread::available_parallelism().map(NonZeroUsize::get).unwrap_or(1);
    available.saturating_sub(2).max(1)
}

/// Output progress information based on the last seen account path.
fn output_progress(last_account: Nibbles, start_time: Instant, inconsistent_nodes: u64) {
    let mut current_value: u64 = 0;
    let nibbles_to_use = last_account.len().min(16);

    for i in 0..nibbles_to_use {
        current_value = (current_value << 4) | (last_account.get(i).unwrap_or(0) as u64);
    }
    if nibbles_to_use < 16 {
        current_value <<= (16 - nibbles_to_use) * 4;
    }

    let progress_percent = current_value as f64 / u64::MAX as f64 * 100.0;
    let progress_percent_str = format!("{progress_percent:.2}");

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_for_storage_root_buffers_future_accounts() {
        let target_account = B256::from([0x11; 32]);
        let future_account = B256::from([0x22; 32]);
        let target_root = B256::from([0xaa; 32]);
        let future_root = B256::from([0xbb; 32]);
        let target_output = Output::StorageExtra(
            target_account,
            Nibbles::from_nibbles([0x1]),
            BranchNodeCompact::default(),
        );
        let future_output = Output::StorageExtra(
            future_account,
            Nibbles::from_nibbles([0x2]),
            BranchNodeCompact::default(),
        );

        let (tx, rx) = mpsc::channel();
        tx.send(Ok(StorageMessage::Output(future_output.clone()))).unwrap();
        tx.send(Ok(StorageMessage::Root { account: future_account, root: future_root })).unwrap();
        tx.send(Ok(StorageMessage::Output(target_output.clone()))).unwrap();
        tx.send(Ok(StorageMessage::Root { account: target_account, root: target_root })).unwrap();

        let mut pending_roots = BTreeMap::new();
        let mut pending_outputs = BTreeMap::new();
        let mut seen_outputs = Vec::new();

        let root = wait_for_storage_root(
            target_account,
            &rx,
            &mut pending_roots,
            &mut pending_outputs,
            &mut |output| {
                seen_outputs.push(output);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(root, target_root);
        assert_eq!(seen_outputs, vec![target_output]);

        seen_outputs.clear();

        let root = wait_for_storage_root(
            future_account,
            &rx,
            &mut pending_roots,
            &mut pending_outputs,
            &mut |output| {
                seen_outputs.push(output);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(root, future_root);
        assert_eq!(seen_outputs, vec![future_output]);
        assert!(pending_roots.is_empty());
        assert!(pending_outputs.is_empty());
    }
}
