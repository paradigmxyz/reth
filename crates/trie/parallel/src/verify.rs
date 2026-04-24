use alloy_consensus::EMPTY_ROOT_HASH;
use alloy_primitives::B256;
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_provider::{DatabaseProviderROFactory, ProviderError};
use reth_storage_api::{DBProvider, StorageSettingsCache};
use reth_storage_errors::db::DatabaseError;
#[cfg(feature = "metrics")]
use reth_trie::{metrics::TrieRootMetrics, TrieType};
use reth_trie::{
    trie_cursor::{noop::NoopTrieCursorFactory, TrieCursor, TrieCursorFactory},
    verify::{
        rebuild_account_nodes_from_hashed_state, sorted_branch_nodes, Output, TrieCursorVerifier,
    },
    BranchNodeCompact, Nibbles, StorageRoot,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, LegacyKeyAdapter, PackedKeyAdapter,
    TrieTableAdapter,
};
use std::{
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

    // Rebuild the storage trie from hashed state alone so the collected updates represent the
    // full canonical storage trie, not an incremental diff against the current trie tables.
    let (root, _, updates) = StorageRoot::new_hashed(
        NoopTrieCursorFactory::default(),
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
    let mut pending_roots = BTreeMap::new();
    let mut pending_outputs = BTreeMap::new();

    let expected_account_nodes = rebuild_account_nodes_from_hashed_state(
        hashed_cursor_factory,
        |hashed_address| -> Result<B256, ParallelTrieVerifyError> {
            let storage_root = wait_for_storage_root(
                hashed_address,
                result_rx,
                &mut pending_roots,
                &mut pending_outputs,
                on_output,
            )?;
            on_output(Output::Progress(Nibbles::unpack(hashed_address)))?;
            Ok(storage_root)
        },
    )?;

    let mut account_verifier =
        TrieCursorVerifier::new(None, trie_cursor_factory.account_trie_cursor()?)?;
    let mut outputs = Vec::new();
    for (path, node) in expected_account_nodes {
        outputs.clear();
        account_verifier.next(&mut outputs, path, node)?;
        for output in outputs.drain(..) {
            on_output(output)?;
        }
    }
    outputs.clear();
    account_verifier.finalize(&mut outputs)?;
    for output in outputs {
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
                let output_account =
                    output.storage_account().expect("storage worker emitted non-storage output");
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
    let mut verifier = TrieCursorVerifier::new(Some(account), cursor)?;
    let mut outputs = Vec::new();
    for (path, node) in expected_nodes {
        outputs.clear();
        verifier.next(&mut outputs, path, node)?;
        for output in outputs.drain(..) {
            result_tx.send(Ok(StorageMessage::Output(output))).map_err(|_| {
                ParallelTrieVerifyError::Other("storage results channel closed".into())
            })?;
        }
    }

    outputs.clear();
    verifier.finalize(&mut outputs)?;
    for output in outputs {
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
    let mut verifier = TrieCursorVerifier::new(Some(account), cursor)?;
    let mut outputs = Vec::new();
    verifier.finalize(&mut outputs)?;
    for output in outputs {
        result_tx
            .send(Ok(StorageMessage::Output(output)))
            .map_err(|_| ParallelTrieVerifyError::Other("storage results channel closed".into()))?;
    }

    Ok(())
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
    use reth_provider::{test_utils::create_test_provider_factory, HashingWriter, TrieWriter};
    use reth_trie::{verify::Verifier, StateRoot};
    use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseStateRoot, DatabaseTrieCursorFactory};

    type DbStateRoot<'a, TX, A> =
        StateRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;

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

    #[test]
    fn parallel_verifier_uses_full_account_trie_on_populated_tables() {
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
                [
                    StorageEntry { key: B256::from([0x11; 32]), value: U256::from(1) },
                    StorageEntry { key: B256::from([0x22; 32]), value: U256::from(2) },
                    StorageEntry { key: B256::from([0x33; 32]), value: U256::from(3) },
                ],
            )])
            .unwrap();

        let updates = reth_trie_db::with_adapter!(provider_rw, |A| {
            let (_root, updates) =
                DbStateRoot::<_, A>::from_tx(provider_rw.tx_ref()).root_with_updates().unwrap();
            updates
        });
        provider_rw.write_trie_updates(updates).unwrap();
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
        assert!(expected.is_empty());

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
