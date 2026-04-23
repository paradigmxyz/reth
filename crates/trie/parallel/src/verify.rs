use alloy_consensus::EMPTY_ROOT_HASH;
use alloy_primitives::B256;
use alloy_rlp::{BufMut, Encodable};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_provider::{DatabaseProviderROFactory, ProviderError};
use reth_storage_api::{DBProvider, StorageSettingsCache};
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    node_iter::{TrieElement, TrieNodeIter},
    trie_cursor::{depth_first, DepthFirstTrieIterator, TrieCursor, TrieCursorFactory},
    updates::TrieUpdates,
    verify::Output,
    walker::TrieWalker,
    BranchNodeCompact, HashBuilder, Nibbles, StorageRoot, TRIE_ACCOUNT_RLP_MAX_SIZE,
};
#[cfg(feature = "metrics")]
use reth_trie::{metrics::TrieRootMetrics, TrieType};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, LegacyKeyAdapter, PackedKeyAdapter,
    TrieTableAdapter,
};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    num::NonZeroUsize,
    sync::{mpsc, Arc, Mutex},
    thread,
};
use thiserror::Error;
use tracing::info;

/// Parallel trie verifier for `reth db repair-trie`-style consistency checks.
#[derive(Debug)]
pub struct ParallelTrieVerifier<Factory> {
    factory: Factory,
    storage_workers: usize,
}

impl<Factory> ParallelTrieVerifier<Factory> {
    /// Create a new parallel trie verifier.
    pub fn new(factory: Factory) -> Self {
        Self { factory, storage_workers: storage_worker_count() }
    }

    /// Override the number of storage worker threads.
    pub const fn with_storage_workers(mut self, storage_workers: usize) -> Self {
        self.storage_workers = storage_workers;
        self
    }
}

impl<Factory> ParallelTrieVerifier<Factory>
where
    Factory: DatabaseProviderROFactory + StorageSettingsCache + Clone + Send + Sync,
    Factory::Provider: DBProvider + Send,
{
    /// Verify the trie tables and stream outputs into the provided callback.
    pub fn verify<O>(self, mut on_output: O) -> Result<(), ParallelTrieVerifyError>
    where
        O: FnMut(Output) -> Result<(), DatabaseError>,
    {
        if self.factory.cached_storage_settings().is_v2() {
            self.verify_with_adapter::<PackedKeyAdapter, _>(&mut on_output)
        } else {
            self.verify_with_adapter::<LegacyKeyAdapter, _>(&mut on_output)
        }
    }

    fn verify_with_adapter<A, O>(self, on_output: &mut O) -> Result<(), ParallelTrieVerifyError>
    where
        A: TrieTableAdapter + Send,
        O: FnMut(Output) -> Result<(), DatabaseError>,
    {
        let storage_workers = self.storage_workers.max(1);
        info!(storage_workers, "Verifying storage tries in parallel");

        let (job_tx, job_rx) = mpsc::sync_channel(storage_workers.saturating_mul(2).max(1));
        let job_rx = Arc::new(Mutex::new(job_rx));
        let (result_tx, result_rx) = mpsc::channel();

        thread::scope(|scope| -> Result<(), ParallelTrieVerifyError> {
            {
                let result_tx = result_tx.clone();
                let job_tx = job_tx.clone();
                let factory = self.factory.clone();

                scope.spawn(move || {
                    if let Err(err) = dispatch_storage_jobs::<A, _>(&factory, &job_tx, &result_tx) {
                        let _ = result_tx.send(Err(err));
                    }
                });
            }

            for _ in 0..storage_workers {
                let job_rx = Arc::clone(&job_rx);
                let result_tx = result_tx.clone();
                let factory = self.factory.clone();

                scope.spawn(move || {
                    let provider = match open_provider(&factory) {
                        Ok(provider) => provider,
                        Err(err) => {
                            let _ = result_tx.send(Err(err));
                            return;
                        }
                    };

                    loop {
                        let account =
                            match job_rx.lock().expect("poisoned storage job receiver").recv() {
                                Ok(account) => account,
                                Err(_) => break,
                            };

                        if let Err(err) =
                            verify_storage_account::<A, _>(&provider, account, &result_tx)
                        {
                            let _ = result_tx.send(Err(err));
                            break;
                        }
                    }
                });
            }

            drop(job_tx);
            drop(result_tx);

            verify_accounts::<A, _, _>(&self.factory, &result_rx, on_output)
        })
    }
}

/// Error returned by [`ParallelTrieVerifier`].
#[derive(Error, Debug)]
pub enum ParallelTrieVerifyError {
    /// Provider error while opening or reading providers.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Direct database error.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Storage root computation error.
    #[error(transparent)]
    StorageRoot(#[from] reth_execution_errors::StorageRootError),
    /// Generic error.
    #[error("{0}")]
    Other(String),
}

type StorageResult = Result<StorageMessage, ParallelTrieVerifyError>;

// Each account's storage result stream must come from exactly one producer, and that producer must
// send every inconsistency before it sends the final root. `wait_for_storage_root` relies on that
// ordering when it buffers future-account messages.
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
    fn new(account: Option<B256>, trie_cursor: C) -> Result<Self, DatabaseError> {
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

    fn next(
        &mut self,
        path: Nibbles,
        node: BranchNodeCompact,
    ) -> Result<Vec<Output>, DatabaseError> {
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

    fn finalize(&mut self) -> Result<Vec<Output>, DatabaseError> {
        let mut outputs = Vec::new();
        while let Some((curr_path, curr_node)) = self.curr.take() {
            outputs.push(self.output_extra(curr_path, curr_node));
            self.curr = self.trie_iter.next().transpose()?;
        }
        Ok(outputs)
    }
}

fn open_provider<Factory>(factory: &Factory) -> Result<Factory::Provider, ParallelTrieVerifyError>
where
    Factory: DatabaseProviderROFactory,
    Factory::Provider: DBProvider,
{
    Ok(factory.database_provider_ro()?.disable_long_read_transaction_safety())
}

fn dispatch_storage_jobs<A, Factory>(
    factory: &Factory,
    job_tx: &mpsc::SyncSender<B256>,
    result_tx: &mpsc::Sender<StorageResult>,
) -> Result<(), ParallelTrieVerifyError>
where
    Factory: DatabaseProviderROFactory,
    Factory::Provider: DBProvider,
    A: TrieTableAdapter,
{
    let provider = open_provider(factory)?;
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
            job_tx.send(account).map_err(|_| {
                ParallelTrieVerifyError::Other("storage worker queue closed".into())
            })?;
            next_storage_account = storage_cursor.next_no_dup()?.map(|entry| entry.0);
        } else {
            emit_empty_storage_inconsistencies(
                account,
                trie_cursor_factory.storage_trie_cursor(account)?,
                result_tx,
            )?;
            result_tx.send(Ok(StorageMessage::Root { account, root: EMPTY_ROOT_HASH })).map_err(
                |_| ParallelTrieVerifyError::Other("storage results channel closed".into()),
            )?;
        }

        next_account = account_cursor.next()?;
    }

    Ok(())
}

fn verify_storage_account<A, P>(
    provider: &P,
    account: B256,
    result_tx: &mpsc::Sender<StorageResult>,
) -> Result<(), ParallelTrieVerifyError>
where
    A: TrieTableAdapter,
    P: DBProvider,
{
    let tx = provider.tx_ref();
    let trie_cursor_factory = DatabaseTrieCursorFactory::<_, A>::new(tx);
    let hashed_cursor_factory = DatabaseHashedCursorFactory::new(tx);

    let (root, _, updates) = StorageRoot::new_hashed(
        trie_cursor_factory.clone(),
        hashed_cursor_factory,
        account,
        Default::default(),
        #[cfg(feature = "metrics")]
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
        .map_err(|_| ParallelTrieVerifyError::Other("storage results channel closed".into()))?;

    Ok(())
}

fn verify_accounts<A, Factory, O>(
    factory: &Factory,
    result_rx: &mpsc::Receiver<StorageResult>,
    on_output: &mut O,
) -> Result<(), ParallelTrieVerifyError>
where
    Factory: DatabaseProviderROFactory,
    Factory::Provider: DBProvider,
    A: TrieTableAdapter,
    O: FnMut(Output) -> Result<(), DatabaseError>,
{
    let provider = open_provider(factory)?;
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
) -> Result<B256, ParallelTrieVerifyError>
where
    O: FnMut(Output) -> Result<(), DatabaseError>,
{
    if let Some(root) = pending_roots.remove(&account) {
        flush_pending_outputs(account, pending_outputs, on_output)?;
        return Ok(root)
    }

    loop {
        // `Root` is the end-of-account marker: a producer must emit all storage inconsistencies
        // for that account before its root, so flushing buffered outputs here is sufficient.
        match result_rx.recv().map_err(|_| {
            ParallelTrieVerifyError::Other("storage results channel closed".into())
        })?? {
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
) -> Result<(), DatabaseError>
where
    O: FnMut(Output) -> Result<(), DatabaseError>,
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
) -> Result<(), ParallelTrieVerifyError>
where
    C: TrieCursor,
{
    let mut verifier = RepairTrieCursorVerifier::new(Some(account), cursor)?;
    for (path, node) in expected_nodes {
        for output in verifier.next(path, node)? {
            result_tx.send(Ok(StorageMessage::Output(output))).map_err(|_| {
                ParallelTrieVerifyError::Other("storage results channel closed".into())
            })?;
        }
    }

    for output in verifier.finalize()? {
        result_tx
            .send(Ok(StorageMessage::Output(output)))
            .map_err(|_| ParallelTrieVerifyError::Other("storage results channel closed".into()))?;
    }

    Ok(())
}

fn emit_empty_storage_inconsistencies<C>(
    account: B256,
    cursor: C,
    result_tx: &mpsc::Sender<StorageResult>,
) -> Result<(), ParallelTrieVerifyError>
where
    C: TrieCursor,
{
    let mut verifier = RepairTrieCursorVerifier::new(Some(account), cursor)?;
    for output in verifier.finalize()? {
        result_tx
            .send(Ok(StorageMessage::Output(output)))
            .map_err(|_| ParallelTrieVerifyError::Other("storage results channel closed".into()))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::{test_utils::create_test_provider_factory, HashingWriter};
    use reth_trie::verify::Verifier;
    use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};

    fn non_progress(outputs: Vec<Output>) -> Vec<String> {
        let mut outputs = outputs
            .into_iter()
            .filter(|output| !matches!(output, Output::Progress(_)))
            .map(|output| format!("{output:?}"))
            .collect::<Vec<_>>();
        outputs.sort_unstable();
        outputs
    }

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

    #[test]
    fn wait_for_storage_root_flushes_buffered_outputs_when_root_is_already_pending() {
        let account = B256::from([0x11; 32]);
        let root = B256::from([0xaa; 32]);
        let output = Output::StorageExtra(
            account,
            Nibbles::from_nibbles([0x1]),
            BranchNodeCompact::default(),
        );

        let (_tx, rx) = mpsc::channel();
        let mut pending_roots = BTreeMap::from([(account, root)]);
        let mut pending_outputs = BTreeMap::from([(account, vec![output.clone()])]);
        let mut seen_outputs = Vec::new();

        let returned_root = wait_for_storage_root(
            account,
            &rx,
            &mut pending_roots,
            &mut pending_outputs,
            &mut |output| {
                seen_outputs.push(output);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(returned_root, root);
        assert_eq!(seen_outputs, vec![output]);
        assert!(pending_roots.is_empty());
        assert!(pending_outputs.is_empty());
    }

    #[test]
    fn parallel_verifier_matches_sequential_outputs() {
        let factory = create_test_provider_factory();
        let address_with_storage = "0x1000000000000000000000000000000000000001".parse().unwrap();
        let address_without_storage = "0x2000000000000000000000000000000000000002".parse().unwrap();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw
            .insert_account_for_hashing([
                (
                    address_with_storage,
                    Some(Account { nonce: 1, balance: U256::from(10), bytecode_hash: None }),
                ),
                (
                    address_without_storage,
                    Some(Account { nonce: 2, balance: U256::from(20), bytecode_hash: None }),
                ),
            ])
            .unwrap();
        provider_rw
            .insert_storage_for_hashing([(
                address_with_storage,
                [StorageEntry { key: B256::from([0x33; 32]), value: U256::from(1) }],
            )])
            .unwrap();
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap().disable_long_read_transaction_safety();
        let expected = if provider.cached_storage_settings().is_v2() {
            let trie_cursor_factory =
                DatabaseTrieCursorFactory::<_, PackedKeyAdapter>::new(provider.tx_ref());
            non_progress(
                Verifier::new(
                    &trie_cursor_factory,
                    DatabaseHashedCursorFactory::new(provider.tx_ref()),
                )
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            )
        } else {
            let trie_cursor_factory =
                DatabaseTrieCursorFactory::<_, LegacyKeyAdapter>::new(provider.tx_ref());
            non_progress(
                Verifier::new(
                    &trie_cursor_factory,
                    DatabaseHashedCursorFactory::new(provider.tx_ref()),
                )
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            )
        };

        let mut actual = Vec::new();
        ParallelTrieVerifier::new(factory.clone())
            .verify(|output| {
                actual.push(output);
                Ok(())
            })
            .unwrap();

        assert_eq!(expected, non_progress(actual));
    }
}
