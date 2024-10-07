use crate::{
    bundle_state::StorageRevertsIter,
    providers::{database::metrics, static_file::StaticFileWriter, StaticFileProvider},
    to_range,
    traits::{
        AccountExtReader, BlockSource, ChangeSetReader, ReceiptProvider, StageCheckpointWriter,
    },
    writer::UnifiedStorageWriter,
    AccountReader, BlockExecutionReader, BlockExecutionWriter, BlockHashReader, BlockNumReader,
    BlockReader, BlockWriter, BundleStateInit, DBProvider, EvmEnvProvider, FinalizedBlockReader,
    FinalizedBlockWriter, HashingWriter, HeaderProvider, HeaderSyncGap, HeaderSyncGapProvider,
    HistoricalStateProvider, HistoryWriter, LatestStateProvider, OriginalValuesKnown,
    ProviderError, PruneCheckpointReader, PruneCheckpointWriter, RequestsProvider, RevertsInit,
    StageCheckpointReader, StateChangeWriter, StateProviderBox, StateReader, StateWriter,
    StaticFileProviderFactory, StatsReader, StorageReader, StorageTrieWriter, TransactionVariant,
    TransactionsProvider, TransactionsProviderExt, TrieWriter, WithdrawalsProvider,
};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{keccak256, Address, BlockHash, BlockNumber, TxHash, TxNumber, B256, U256};
use itertools::{izip, Itertools};
use rayon::slice::ParallelSliceMut;
use reth_chainspec::{ChainInfo, ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_db::{
    cursor::DbDupCursorRW, tables, BlockNumberList, PlainAccountState, PlainStorageState,
};
use reth_db_api::{
    common::KeyValue,
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    models::{
        sharded_key, storage_sharded_key::StorageShardedKey, AccountBeforeTx, BlockNumberAddress,
        ShardedKey, StoredBlockBodyIndices, StoredBlockOmmers, StoredBlockWithdrawals,
    },
    table::Table,
    transaction::{DbTx, DbTxMut},
    DatabaseError, DbTxUnwindExt,
};
use reth_evm::ConfigureEvmEnv;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_network_p2p::headers::downloader::SyncTarget;
use reth_primitives::{
    Account, Block, BlockBody, BlockWithSenders, Bytecode, GotExpected, Header, Receipt, Requests,
    SealedBlock, SealedBlockWithSenders, SealedHeader, StaticFileSegment, StorageEntry,
    TransactionMeta, TransactionSigned, TransactionSignedEcRecovered, TransactionSignedNoHash,
    Withdrawal, Withdrawals,
};
use reth_prune_types::{PruneCheckpoint, PruneModes, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{StorageChangeSetReader, TryIntoHistoricalStateProvider};
use reth_storage_errors::provider::{ProviderResult, RootMismatch};
use reth_trie::{
    prefix_set::{PrefixSet, PrefixSetMut, TriePrefixSets},
    updates::{StorageTrieUpdates, TrieUpdates},
    HashedPostStateSorted, Nibbles, StateRoot, StoredNibbles,
};
use reth_trie_db::{DatabaseStateRoot, DatabaseStorageTrieCursor};
use revm::{
    db::states::{PlainStateReverts, PlainStorageChangeset, PlainStorageRevert, StateChangeset},
    primitives::{BlockEnv, CfgEnvWithHandlerCfg},
};
use std::{
    cmp::Ordering,
    collections::{hash_map, BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Debug,
    ops::{Bound, Deref, DerefMut, Range, RangeBounds, RangeInclusive},
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use tokio::sync::watch;
use tracing::{debug, error, warn};

/// A [`DatabaseProvider`] that holds a read-only database transaction.
pub type DatabaseProviderRO<DB, Spec> = DatabaseProvider<<DB as Database>::TX, Spec>;

/// A [`DatabaseProvider`] that holds a read-write database transaction.
///
/// Ideally this would be an alias type. However, there's some weird compiler error (<https://github.com/rust-lang/rust/issues/102211>), that forces us to wrap this in a struct instead.
/// Once that issue is solved, we can probably revert back to being an alias type.
#[derive(Debug)]
pub struct DatabaseProviderRW<DB: Database, Spec>(
    pub DatabaseProvider<<DB as Database>::TXMut, Spec>,
);

impl<DB: Database, Spec> Deref for DatabaseProviderRW<DB, Spec> {
    type Target = DatabaseProvider<<DB as Database>::TXMut, Spec>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<DB: Database, Spec> DerefMut for DatabaseProviderRW<DB, Spec> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<DB: Database, Spec> AsRef<DatabaseProvider<<DB as Database>::TXMut, Spec>>
    for DatabaseProviderRW<DB, Spec>
{
    fn as_ref(&self) -> &DatabaseProvider<<DB as Database>::TXMut, Spec> {
        &self.0
    }
}

impl<DB: Database, Spec: Send + Sync + 'static> DatabaseProviderRW<DB, Spec> {
    /// Commit database transaction and static file if it exists.
    pub fn commit(self) -> ProviderResult<bool> {
        self.0.commit()
    }

    /// Consume `DbTx` or `DbTxMut`.
    pub fn into_tx(self) -> <DB as Database>::TXMut {
        self.0.into_tx()
    }
}

impl<DB: Database, Spec> From<DatabaseProviderRW<DB, Spec>>
    for DatabaseProvider<<DB as Database>::TXMut, Spec>
{
    fn from(provider: DatabaseProviderRW<DB, Spec>) -> Self {
        provider.0
    }
}

/// A provider struct that fetches data from the database.
/// Wrapper around [`DbTx`] and [`DbTxMut`]. Example: [`HeaderProvider`] [`BlockHashReader`]
#[derive(Debug)]
pub struct DatabaseProvider<TX, Spec> {
    /// Database transaction.
    tx: TX,
    /// Chain spec
    chain_spec: Arc<Spec>,
    /// Static File provider
    static_file_provider: StaticFileProvider,
    /// Pruning configuration
    prune_modes: PruneModes,
}

impl<TX, Spec> DatabaseProvider<TX, Spec> {
    /// Returns reference to prune modes.
    pub const fn prune_modes_ref(&self) -> &PruneModes {
        &self.prune_modes
    }
}

impl<TX, Spec> StaticFileProviderFactory for DatabaseProvider<TX, Spec> {
    /// Returns a static file provider
    fn static_file_provider(&self) -> StaticFileProvider {
        self.static_file_provider.clone()
    }
}

impl<TX: Send + Sync, Spec: EthChainSpec + 'static> ChainSpecProvider
    for DatabaseProvider<TX, Spec>
{
    type ChainSpec = Spec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.chain_spec.clone()
    }
}

impl<TX: DbTxMut, Spec> DatabaseProvider<TX, Spec> {
    /// Creates a provider with an inner read-write transaction.
    pub const fn new_rw(
        tx: TX,
        chain_spec: Arc<Spec>,
        static_file_provider: StaticFileProvider,
        prune_modes: PruneModes,
    ) -> Self {
        Self { tx, chain_spec, static_file_provider, prune_modes }
    }
}

impl<TX, Spec> AsRef<Self> for DatabaseProvider<TX, Spec> {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<TX: DbTx + 'static, Spec: Send + Sync> TryIntoHistoricalStateProvider
    for DatabaseProvider<TX, Spec>
{
    fn try_into_history_at_block(
        self,
        mut block_number: BlockNumber,
    ) -> ProviderResult<StateProviderBox> {
        if block_number == self.best_block_number().unwrap_or_default() &&
            block_number == self.last_block_number().unwrap_or_default()
        {
            return Ok(Box::new(LatestStateProvider::new(self.tx, self.static_file_provider)))
        }

        // +1 as the changeset that we want is the one that was applied after this block.
        block_number += 1;

        let account_history_prune_checkpoint =
            self.get_prune_checkpoint(PruneSegment::AccountHistory)?;
        let storage_history_prune_checkpoint =
            self.get_prune_checkpoint(PruneSegment::StorageHistory)?;

        let mut state_provider =
            HistoricalStateProvider::new(self.tx, block_number, self.static_file_provider);

        // If we pruned account or storage history, we can't return state on every historical block.
        // Instead, we should cap it at the latest prune checkpoint for corresponding prune segment.
        if let Some(prune_checkpoint_block_number) =
            account_history_prune_checkpoint.and_then(|checkpoint| checkpoint.block_number)
        {
            state_provider = state_provider.with_lowest_available_account_history_block_number(
                prune_checkpoint_block_number + 1,
            );
        }
        if let Some(prune_checkpoint_block_number) =
            storage_history_prune_checkpoint.and_then(|checkpoint| checkpoint.block_number)
        {
            state_provider = state_provider.with_lowest_available_storage_history_block_number(
                prune_checkpoint_block_number + 1,
            );
        }

        Ok(Box::new(state_provider))
    }
}

impl<Tx: DbTx + DbTxMut + 'static, Spec: Send + Sync + EthereumHardforks + 'static>
    DatabaseProvider<Tx, Spec>
{
    // TODO: uncomment below, once `reth debug_cmd` has been feature gated with dev.
    // #[cfg(any(test, feature = "test-utils"))]
    /// Inserts an historical block. **Used for setting up test environments**
    pub fn insert_historical_block(
        &self,
        block: SealedBlockWithSenders,
    ) -> ProviderResult<StoredBlockBodyIndices> {
        let ttd = if block.number == 0 {
            block.difficulty
        } else {
            let parent_block_number = block.number - 1;
            let parent_ttd = self.header_td_by_number(parent_block_number)?.unwrap_or_default();
            parent_ttd + block.difficulty
        };

        let mut writer = self.static_file_provider.latest_writer(StaticFileSegment::Headers)?;

        // Backfill: some tests start at a forward block number, but static files require no gaps.
        let segment_header = writer.user_header();
        if segment_header.block_end().is_none() && segment_header.expected_block_start() == 0 {
            for block_number in 0..block.number {
                let mut prev = block.header.clone().unseal();
                prev.number = block_number;
                writer.append_header(&prev, U256::ZERO, &B256::ZERO)?;
            }
        }

        writer.append_header(block.header.as_ref(), ttd, &block.hash())?;

        self.insert_block(block)
    }
}

/// For a given key, unwind all history shards that are below the given block number.
///
/// S - Sharded key subtype.
/// T - Table to walk over.
/// C - Cursor implementation.
///
/// This function walks the entries from the given start key and deletes all shards that belong to
/// the key and are below the given block number.
///
/// The boundary shard (the shard is split by the block number) is removed from the database. Any
/// indices that are above the block number are filtered out. The boundary shard is returned for
/// reinsertion (if it's not empty).
fn unwind_history_shards<S, T, C>(
    cursor: &mut C,
    start_key: T::Key,
    block_number: BlockNumber,
    mut shard_belongs_to_key: impl FnMut(&T::Key) -> bool,
) -> ProviderResult<Vec<u64>>
where
    T: Table<Value = BlockNumberList>,
    T::Key: AsRef<ShardedKey<S>>,
    C: DbCursorRO<T> + DbCursorRW<T>,
{
    let mut item = cursor.seek_exact(start_key)?;
    while let Some((sharded_key, list)) = item {
        // If the shard does not belong to the key, break.
        if !shard_belongs_to_key(&sharded_key) {
            break
        }
        cursor.delete_current()?;

        // Check the first item.
        // If it is greater or eq to the block number, delete it.
        let first = list.iter().next().expect("List can't be empty");
        if first >= block_number {
            item = cursor.prev()?;
            continue
        } else if block_number <= sharded_key.as_ref().highest_block_number {
            // Filter out all elements greater than block number.
            return Ok(list.iter().take_while(|i| *i < block_number).collect::<Vec<_>>())
        }
        return Ok(list.iter().collect::<Vec<_>>())
    }

    Ok(Vec::new())
}

impl<TX: DbTx, Spec: Send + Sync> DatabaseProvider<TX, Spec> {
    /// Creates a provider with an inner read-only transaction.
    pub const fn new(
        tx: TX,
        chain_spec: Arc<Spec>,
        static_file_provider: StaticFileProvider,
        prune_modes: PruneModes,
    ) -> Self {
        Self { tx, chain_spec, static_file_provider, prune_modes }
    }

    /// Consume `DbTx` or `DbTxMut`.
    pub fn into_tx(self) -> TX {
        self.tx
    }

    /// Pass `DbTx` or `DbTxMut` mutable reference.
    pub fn tx_mut(&mut self) -> &mut TX {
        &mut self.tx
    }

    /// Pass `DbTx` or `DbTxMut` immutable reference.
    pub const fn tx_ref(&self) -> &TX {
        &self.tx
    }

    /// Returns a reference to the chain specification.
    pub fn chain_spec(&self) -> &Spec {
        &self.chain_spec
    }

    /// Disables long-lived read transaction safety guarantees for leaks prevention and
    /// observability improvements.
    ///
    /// CAUTION: In most of the cases, you want the safety guarantees for long read transactions
    /// enabled. Use this only if you're sure that no write transaction is open in parallel, meaning
    /// that Reth as a node is offline and not progressing.
    pub fn disable_long_read_transaction_safety(mut self) -> Self {
        self.tx.disable_long_read_transaction_safety();
        self
    }

    /// Return full table as Vec
    pub fn table<T: Table>(&self) -> Result<Vec<KeyValue<T>>, DatabaseError>
    where
        T::Key: Default + Ord,
    {
        self.tx
            .cursor_read::<T>()?
            .walk(Some(T::Key::default()))?
            .collect::<Result<Vec<_>, DatabaseError>>()
    }

    /// Return a list of entries from the table, based on the given range.
    #[inline]
    pub fn get<T: Table>(
        &self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<Vec<KeyValue<T>>, DatabaseError> {
        self.tx.cursor_read::<T>()?.walk_range(range)?.collect::<Result<Vec<_>, _>>()
    }

    /// Iterates over read only values in the given table and collects them into a vector.
    ///
    /// Early-returns if the range is empty, without opening a cursor transaction.
    fn cursor_read_collect<T: Table<Key = u64>>(
        &self,
        range: impl RangeBounds<T::Key>,
    ) -> ProviderResult<Vec<T::Value>> {
        let capacity = match range_size_hint(&range) {
            Some(0) | None => return Ok(Vec::new()),
            Some(capacity) => capacity,
        };
        let mut cursor = self.tx.cursor_read::<T>()?;
        self.cursor_collect_with_capacity(&mut cursor, range, capacity)
    }

    /// Iterates over read only values in the given table and collects them into a vector.
    fn cursor_collect<T: Table<Key = u64>>(
        &self,
        cursor: &mut impl DbCursorRO<T>,
        range: impl RangeBounds<T::Key>,
    ) -> ProviderResult<Vec<T::Value>> {
        let capacity = range_size_hint(&range).unwrap_or(0);
        self.cursor_collect_with_capacity(cursor, range, capacity)
    }

    fn cursor_collect_with_capacity<T: Table<Key = u64>>(
        &self,
        cursor: &mut impl DbCursorRO<T>,
        range: impl RangeBounds<T::Key>,
        capacity: usize,
    ) -> ProviderResult<Vec<T::Value>> {
        let mut items = Vec::with_capacity(capacity);
        for entry in cursor.walk_range(range)? {
            items.push(entry?.1);
        }
        Ok(items)
    }

    fn transactions_by_tx_range_with_cursor<C>(
        &self,
        range: impl RangeBounds<TxNumber>,
        cursor: &mut C,
    ) -> ProviderResult<Vec<TransactionSignedNoHash>>
    where
        C: DbCursorRO<tables::Transactions>,
    {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Transactions,
            to_range(range),
            |static_file, range, _| static_file.transactions_by_tx_range(range),
            |range, _| self.cursor_collect(cursor, range),
            |_| true,
        )
    }

    fn block_with_senders<H, HF, B, BF>(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
        header_by_number: HF,
        construct_block: BF,
    ) -> ProviderResult<Option<B>>
    where
        Spec: EthereumHardforks,
        H: AsRef<Header>,
        HF: FnOnce(BlockNumber) -> ProviderResult<Option<H>>,
        BF: FnOnce(
            H,
            Vec<TransactionSigned>,
            Vec<Address>,
            Vec<Header>,
            Option<Withdrawals>,
            Option<Requests>,
        ) -> ProviderResult<Option<B>>,
    {
        let Some(block_number) = self.convert_hash_or_number(id)? else { return Ok(None) };
        let Some(header) = header_by_number(block_number)? else { return Ok(None) };

        let ommers = self.ommers(block_number.into())?.unwrap_or_default();
        let withdrawals =
            self.withdrawals_by_block(block_number.into(), header.as_ref().timestamp)?;
        let requests = self.requests_by_block(block_number.into(), header.as_ref().timestamp)?;

        // Get the block body
        //
        // If the body indices are not found, this means that the transactions either do not exist
        // in the database yet, or they do exit but are not indexed. If they exist but are not
        // indexed, we don't have enough information to return the block anyways, so we return
        // `None`.
        let Some(body) = self.block_body_indices(block_number)? else { return Ok(None) };

        let tx_range = body.tx_num_range();

        let (transactions, senders) = if tx_range.is_empty() {
            (vec![], vec![])
        } else {
            (self.transactions_by_tx_range(tx_range.clone())?, self.senders_by_tx_range(tx_range)?)
        };

        let body = transactions
            .into_iter()
            .map(|tx| match transaction_kind {
                TransactionVariant::NoHash => TransactionSigned {
                    // Caller explicitly asked for no hash, so we don't calculate it
                    hash: B256::ZERO,
                    signature: tx.signature,
                    transaction: tx.transaction,
                },
                TransactionVariant::WithHash => tx.with_hash(),
            })
            .collect();

        construct_block(header, body, senders, ommers, withdrawals, requests)
    }

    /// Returns a range of blocks from the database.
    ///
    /// Uses the provided `headers_range` to get the headers for the range, and `assemble_block` to
    /// construct blocks from the following inputs:
    ///     – Header
    ///     - Range of transaction numbers
    ///     – Ommers
    ///     – Withdrawals
    ///     – Requests
    ///     – Senders
    fn block_range<F, H, HF, R>(
        &self,
        range: RangeInclusive<BlockNumber>,
        headers_range: HF,
        mut assemble_block: F,
    ) -> ProviderResult<Vec<R>>
    where
        Spec: EthereumHardforks,
        H: AsRef<Header>,
        HF: FnOnce(RangeInclusive<BlockNumber>) -> ProviderResult<Vec<H>>,
        F: FnMut(
            H,
            Range<TxNumber>,
            Vec<Header>,
            Option<Withdrawals>,
            Option<Requests>,
        ) -> ProviderResult<R>,
    {
        if range.is_empty() {
            return Ok(Vec::new())
        }

        let len = range.end().saturating_sub(*range.start()) as usize;
        let mut blocks = Vec::with_capacity(len);

        let headers = headers_range(range)?;
        let mut ommers_cursor = self.tx.cursor_read::<tables::BlockOmmers>()?;
        let mut withdrawals_cursor = self.tx.cursor_read::<tables::BlockWithdrawals>()?;
        let mut requests_cursor = self.tx.cursor_read::<tables::BlockRequests>()?;
        let mut block_body_cursor = self.tx.cursor_read::<tables::BlockBodyIndices>()?;

        for header in headers {
            let header_ref = header.as_ref();
            // If the body indices are not found, this means that the transactions either do
            // not exist in the database yet, or they do exit but are
            // not indexed. If they exist but are not indexed, we don't
            // have enough information to return the block anyways, so
            // we skip the block.
            if let Some((_, block_body_indices)) =
                block_body_cursor.seek_exact(header_ref.number)?
            {
                let tx_range = block_body_indices.tx_num_range();

                // If we are past shanghai, then all blocks should have a withdrawal list,
                // even if empty
                let withdrawals =
                    if self.chain_spec.is_shanghai_active_at_timestamp(header_ref.timestamp) {
                        Some(
                            withdrawals_cursor
                                .seek_exact(header_ref.number)?
                                .map(|(_, w)| w.withdrawals)
                                .unwrap_or_default(),
                        )
                    } else {
                        None
                    };
                let requests =
                    if self.chain_spec.is_prague_active_at_timestamp(header_ref.timestamp) {
                        Some(requests_cursor.seek_exact(header_ref.number)?.unwrap_or_default().1)
                    } else {
                        None
                    };
                let ommers =
                    if self.chain_spec.final_paris_total_difficulty(header_ref.number).is_some() {
                        Vec::new()
                    } else {
                        ommers_cursor
                            .seek_exact(header_ref.number)?
                            .map(|(_, o)| o.ommers)
                            .unwrap_or_default()
                    };

                if let Ok(b) = assemble_block(header, tx_range, ommers, withdrawals, requests) {
                    blocks.push(b);
                }
            }
        }

        Ok(blocks)
    }

    /// Returns a range of blocks from the database, along with the senders of each
    /// transaction in the blocks.
    ///
    /// Uses the provided `headers_range` to get the headers for the range, and `assemble_block` to
    /// construct blocks from the following inputs:
    ///     – Header
    ///     - Transactions
    ///     – Ommers
    ///     – Withdrawals
    ///     – Requests
    ///     – Senders
    fn block_with_senders_range<H, HF, B, BF>(
        &self,
        range: RangeInclusive<BlockNumber>,
        headers_range: HF,
        assemble_block: BF,
    ) -> ProviderResult<Vec<B>>
    where
        Spec: EthereumHardforks,
        H: AsRef<Header>,
        HF: Fn(RangeInclusive<BlockNumber>) -> ProviderResult<Vec<H>>,
        BF: Fn(
            H,
            Vec<TransactionSigned>,
            Vec<Header>,
            Option<Withdrawals>,
            Option<Requests>,
            Vec<Address>,
        ) -> ProviderResult<B>,
    {
        let mut tx_cursor = self.tx.cursor_read::<tables::Transactions>()?;
        let mut senders_cursor = self.tx.cursor_read::<tables::TransactionSenders>()?;

        self.block_range(range, headers_range, |header, tx_range, ommers, withdrawals, requests| {
            let (body, senders) = if tx_range.is_empty() {
                (Vec::new(), Vec::new())
            } else {
                let body = self
                    .transactions_by_tx_range_with_cursor(tx_range.clone(), &mut tx_cursor)?
                    .into_iter()
                    .map(Into::into)
                    .collect::<Vec<TransactionSigned>>();
                // fetch senders from the senders table
                let known_senders =
                    senders_cursor
                        .walk_range(tx_range.clone())?
                        .collect::<Result<HashMap<_, _>, _>>()?;

                let mut senders = Vec::with_capacity(body.len());
                for (tx_num, tx) in tx_range.zip(body.iter()) {
                    match known_senders.get(&tx_num) {
                        None => {
                            // recover the sender from the transaction if not found
                            let sender = tx
                                .recover_signer_unchecked()
                                .ok_or(ProviderError::SenderRecoveryError)?;
                            senders.push(sender);
                        }
                        Some(sender) => senders.push(*sender),
                    }
                }

                (body, senders)
            };

            assemble_block(header, body, ommers, withdrawals, requests, senders)
        })
    }

    /// Get requested blocks transaction with senders
    pub(crate) fn get_block_transaction_range(
        &self,
        range: impl RangeBounds<BlockNumber> + Clone,
    ) -> ProviderResult<Vec<(BlockNumber, Vec<TransactionSignedEcRecovered>)>> {
        // Raad range of block bodies to get all transactions id's of this range.
        let block_bodies = self.get::<tables::BlockBodyIndices>(range)?;

        if block_bodies.is_empty() {
            return Ok(Vec::new())
        }

        // Compute the first and last tx ID in the range
        let first_transaction = block_bodies.first().expect("If we have headers").1.first_tx_num();
        let last_transaction = block_bodies.last().expect("Not empty").1.last_tx_num();

        // If this is the case then all of the blocks in the range are empty
        if last_transaction < first_transaction {
            return Ok(block_bodies.into_iter().map(|(n, _)| (n, Vec::new())).collect())
        }

        // Get transactions and senders
        let transactions = self
            .get::<tables::Transactions>(first_transaction..=last_transaction)?
            .into_iter()
            .map(|(id, tx)| (id, tx.into()))
            .collect::<Vec<(u64, TransactionSigned)>>();

        let mut senders =
            self.get::<tables::TransactionSenders>(first_transaction..=last_transaction)?;

        recover_block_senders(&mut senders, &transactions, first_transaction, last_transaction)?;

        // Merge transaction into blocks
        let mut block_tx = Vec::with_capacity(block_bodies.len());
        let mut senders = senders.into_iter();
        let mut transactions = transactions.into_iter();
        for (block_number, block_body) in block_bodies {
            let mut one_block_tx = Vec::with_capacity(block_body.tx_count as usize);
            for _ in block_body.tx_num_range() {
                let tx = transactions.next();
                let sender = senders.next();

                let recovered = match (tx, sender) {
                    (Some((tx_id, tx)), Some((sender_tx_id, sender))) => {
                        if tx_id == sender_tx_id {
                            Ok(TransactionSignedEcRecovered::from_signed_transaction(tx, sender))
                        } else {
                            Err(ProviderError::MismatchOfTransactionAndSenderId { tx_id })
                        }
                    }
                    (Some((tx_id, _)), _) | (_, Some((tx_id, _))) => {
                        Err(ProviderError::MismatchOfTransactionAndSenderId { tx_id })
                    }
                    (None, None) => Err(ProviderError::BlockBodyTransactionCount),
                }?;
                one_block_tx.push(recovered)
            }
            block_tx.push((block_number, one_block_tx));
        }

        Ok(block_tx)
    }

    /// Get the given range of blocks.
    pub fn get_block_range(
        &self,
        range: impl RangeBounds<BlockNumber> + Clone,
    ) -> ProviderResult<Vec<SealedBlockWithSenders>>
    where
        Spec: EthereumHardforks,
    {
        // For blocks we need:
        //
        // - Headers
        // - Bodies (transactions)
        // - Uncles/ommers
        // - Withdrawals
        // - Requests
        // - Signers

        let block_headers = self.get::<tables::Headers>(range.clone())?;
        if block_headers.is_empty() {
            return Ok(Vec::new())
        }

        let block_header_hashes = self.get::<tables::CanonicalHeaders>(range.clone())?;
        let block_ommers = self.get::<tables::BlockOmmers>(range.clone())?;
        let block_withdrawals = self.get::<tables::BlockWithdrawals>(range.clone())?;
        let block_requests = self.get::<tables::BlockRequests>(range.clone())?;

        let block_tx = self.get_block_transaction_range(range)?;
        let mut blocks = Vec::with_capacity(block_headers.len());

        // merge all into block
        let block_header_iter = block_headers.into_iter();
        let block_header_hashes_iter = block_header_hashes.into_iter();
        let block_tx_iter = block_tx.into_iter();

        // Ommers can be empty for some blocks
        let mut block_ommers_iter = block_ommers.into_iter();
        let mut block_withdrawals_iter = block_withdrawals.into_iter();
        let mut block_requests_iter = block_requests.into_iter();
        let mut block_ommers = block_ommers_iter.next();
        let mut block_withdrawals = block_withdrawals_iter.next();
        let mut block_requests = block_requests_iter.next();

        for ((main_block_number, header), (_, header_hash), (_, tx)) in
            izip!(block_header_iter, block_header_hashes_iter, block_tx_iter)
        {
            let header = SealedHeader::new(header, header_hash);

            let (transactions, senders) = tx.into_iter().map(|tx| tx.to_components()).unzip();

            // Ommers can be missing
            let mut ommers = Vec::new();
            if let Some((block_number, _)) = block_ommers.as_ref() {
                if *block_number == main_block_number {
                    ommers = block_ommers.take().unwrap().1.ommers;
                    block_ommers = block_ommers_iter.next();
                }
            };

            // withdrawal can be missing
            let shanghai_is_active =
                self.chain_spec.is_shanghai_active_at_timestamp(header.timestamp);
            let mut withdrawals = Some(Withdrawals::default());
            if shanghai_is_active {
                if let Some((block_number, _)) = block_withdrawals.as_ref() {
                    if *block_number == main_block_number {
                        withdrawals = Some(block_withdrawals.take().unwrap().1.withdrawals);
                        block_withdrawals = block_withdrawals_iter.next();
                    }
                }
            } else {
                withdrawals = None
            }

            // requests can be missing
            let prague_is_active = self.chain_spec.is_prague_active_at_timestamp(header.timestamp);
            let mut requests = Some(Requests::default());
            if prague_is_active {
                if let Some((block_number, _)) = block_requests.as_ref() {
                    if *block_number == main_block_number {
                        requests = Some(block_requests.take().unwrap().1);
                        block_requests = block_requests_iter.next();
                    }
                }
            } else {
                requests = None;
            }

            blocks.push(SealedBlockWithSenders {
                block: SealedBlock {
                    header,
                    body: BlockBody { transactions, ommers, withdrawals, requests },
                },
                senders,
            })
        }

        Ok(blocks)
    }

    /// Return the last N blocks of state, recreating the [`ExecutionOutcome`].
    ///
    /// 1. Iterate over the [`BlockBodyIndices`][tables::BlockBodyIndices] table to get all the
    ///    transaction ids.
    /// 2. Iterate over the [`StorageChangeSets`][tables::StorageChangeSets] table and the
    ///    [`AccountChangeSets`][tables::AccountChangeSets] tables in reverse order to reconstruct
    ///    the changesets.
    ///    - In order to have both the old and new values in the changesets, we also access the
    ///      plain state tables.
    /// 3. While iterating over the changeset tables, if we encounter a new account or storage slot,
    ///    we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the plain state
    ///     3. Save the old value to the local state
    /// 4. While iterating over the changeset tables, if we encounter an account/storage slot we
    ///    have seen before we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the local state
    ///     3. Set the local state to the value in the changeset
    ///
    /// If the range is empty, or there are no blocks for the given range, then this returns `None`.
    pub fn get_state(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Option<ExecutionOutcome>> {
        if range.is_empty() {
            return Ok(None)
        }
        let start_block_number = *range.start();

        // We are not removing block meta as it is used to get block changesets.
        let block_bodies = self.get::<tables::BlockBodyIndices>(range.clone())?;

        // get transaction receipts
        let Some(from_transaction_num) = block_bodies.first().map(|bodies| bodies.1.first_tx_num())
        else {
            return Ok(None)
        };
        let Some(to_transaction_num) = block_bodies.last().map(|bodies| bodies.1.last_tx_num())
        else {
            return Ok(None)
        };

        let storage_range = BlockNumberAddress::range(range.clone());

        let storage_changeset = self.get::<tables::StorageChangeSets>(storage_range)?;
        let account_changeset = self.get::<tables::AccountChangeSets>(range)?;

        // This is not working for blocks that are not at tip. as plain state is not the last
        // state of end range. We should rename the functions or add support to access
        // History state. Accessing history state can be tricky but we are not gaining
        // anything.
        let mut plain_accounts_cursor = self.tx.cursor_read::<tables::PlainAccountState>()?;
        let mut plain_storage_cursor = self.tx.cursor_dup_read::<tables::PlainStorageState>()?;

        let (state, reverts) = self.populate_bundle_state(
            account_changeset,
            storage_changeset,
            &mut plain_accounts_cursor,
            &mut plain_storage_cursor,
        )?;

        // iterate over block body and create ExecutionResult
        let mut receipt_iter =
            self.get::<tables::Receipts>(from_transaction_num..=to_transaction_num)?.into_iter();

        let mut receipts = Vec::with_capacity(block_bodies.len());
        // loop break if we are at the end of the blocks.
        for (_, block_body) in block_bodies {
            let mut block_receipts = Vec::with_capacity(block_body.tx_count as usize);
            for _ in block_body.tx_num_range() {
                if let Some((_, receipt)) = receipt_iter.next() {
                    block_receipts.push(Some(receipt));
                }
            }
            receipts.push(block_receipts);
        }

        Ok(Some(ExecutionOutcome::new_init(
            state,
            reverts,
            Vec::new(),
            receipts.into(),
            start_block_number,
            Vec::new(),
        )))
    }

    /// Populate a [`BundleStateInit`] and [`RevertsInit`] using cursors over the
    /// [`PlainAccountState`] and [`PlainStorageState`] tables, based on the given storage and
    /// account changesets.
    fn populate_bundle_state<A, S>(
        &self,
        account_changeset: Vec<(u64, AccountBeforeTx)>,
        storage_changeset: Vec<(BlockNumberAddress, StorageEntry)>,
        plain_accounts_cursor: &mut A,
        plain_storage_cursor: &mut S,
    ) -> ProviderResult<(BundleStateInit, RevertsInit)>
    where
        A: DbCursorRO<PlainAccountState>,
        S: DbDupCursorRO<PlainStorageState>,
    {
        // iterate previous value and get plain state value to create changeset
        // Double option around Account represent if Account state is know (first option) and
        // account is removed (Second Option)
        let mut state: BundleStateInit = HashMap::default();

        // This is not working for blocks that are not at tip. as plain state is not the last
        // state of end range. We should rename the functions or add support to access
        // History state. Accessing history state can be tricky but we are not gaining
        // anything.

        let mut reverts: RevertsInit = HashMap::default();

        // add account changeset changes
        for (block_number, account_before) in account_changeset.into_iter().rev() {
            let AccountBeforeTx { info: old_info, address } = account_before;
            match state.entry(address) {
                hash_map::Entry::Vacant(entry) => {
                    let new_info = plain_accounts_cursor.seek_exact(address)?.map(|kv| kv.1);
                    entry.insert((old_info, new_info, HashMap::default()));
                }
                hash_map::Entry::Occupied(mut entry) => {
                    // overwrite old account state.
                    entry.get_mut().0 = old_info;
                }
            }
            // insert old info into reverts.
            reverts.entry(block_number).or_default().entry(address).or_default().0 = Some(old_info);
        }

        // add storage changeset changes
        for (block_and_address, old_storage) in storage_changeset.into_iter().rev() {
            let BlockNumberAddress((block_number, address)) = block_and_address;
            // get account state or insert from plain state.
            let account_state = match state.entry(address) {
                hash_map::Entry::Vacant(entry) => {
                    let present_info = plain_accounts_cursor.seek_exact(address)?.map(|kv| kv.1);
                    entry.insert((present_info, present_info, HashMap::default()))
                }
                hash_map::Entry::Occupied(entry) => entry.into_mut(),
            };

            // match storage.
            match account_state.2.entry(old_storage.key) {
                hash_map::Entry::Vacant(entry) => {
                    let new_storage = plain_storage_cursor
                        .seek_by_key_subkey(address, old_storage.key)?
                        .filter(|storage| storage.key == old_storage.key)
                        .unwrap_or_default();
                    entry.insert((old_storage.value, new_storage.value));
                }
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().0 = old_storage.value;
                }
            };

            reverts
                .entry(block_number)
                .or_default()
                .entry(address)
                .or_default()
                .1
                .push(old_storage);
        }

        Ok((state, reverts))
    }
}

impl<TX: DbTxMut + DbTx, Spec: Send + Sync> DatabaseProvider<TX, Spec> {
    /// Commit database transaction.
    pub fn commit(self) -> ProviderResult<bool> {
        Ok(self.tx.commit()?)
    }

    /// Remove list of entries from the table. Returns the number of entries removed.
    #[inline]
    pub fn remove<T: Table>(
        &self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<usize, DatabaseError> {
        let mut entries = 0;
        let mut cursor_write = self.tx.cursor_write::<T>()?;
        let mut walker = cursor_write.walk_range(range)?;
        while walker.next().transpose()?.is_some() {
            walker.delete_current()?;
            entries += 1;
        }
        Ok(entries)
    }

    /// Return a list of entries from the table, and remove them, based on the given range.
    #[inline]
    pub fn take<T: Table>(
        &self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<Vec<KeyValue<T>>, DatabaseError> {
        let mut cursor_write = self.tx.cursor_write::<T>()?;
        let mut walker = cursor_write.walk_range(range)?;
        let mut items = Vec::new();
        while let Some(i) = walker.next().transpose()? {
            walker.delete_current()?;
            items.push(i)
        }
        Ok(items)
    }

    /// Remove requested block transactions, without returning them.
    ///
    /// This will remove block data for the given range from the following tables:
    /// * [`BlockBodyIndices`](tables::BlockBodyIndices)
    /// * [`Transactions`](tables::Transactions)
    /// * [`TransactionSenders`](tables::TransactionSenders)
    /// * [`TransactionHashNumbers`](tables::TransactionHashNumbers)
    /// * [`TransactionBlocks`](tables::TransactionBlocks)
    pub fn remove_block_transaction_range(
        &self,
        range: impl RangeBounds<BlockNumber> + Clone,
    ) -> ProviderResult<()> {
        // Raad range of block bodies to get all transactions id's of this range.
        let block_bodies = self.take::<tables::BlockBodyIndices>(range)?;

        if block_bodies.is_empty() {
            return Ok(())
        }

        // Compute the first and last tx ID in the range
        let first_transaction = block_bodies.first().expect("If we have headers").1.first_tx_num();
        let last_transaction = block_bodies.last().expect("Not empty").1.last_tx_num();

        // If this is the case then all of the blocks in the range are empty
        if last_transaction < first_transaction {
            return Ok(())
        }

        // Get transactions so we can then remove
        let transactions = self
            .take::<tables::Transactions>(first_transaction..=last_transaction)?
            .into_iter()
            .map(|(id, tx)| (id, tx.into()))
            .collect::<Vec<(u64, TransactionSigned)>>();

        // remove senders
        self.remove::<tables::TransactionSenders>(first_transaction..=last_transaction)?;

        // Remove TransactionHashNumbers
        let mut tx_hash_cursor = self.tx.cursor_write::<tables::TransactionHashNumbers>()?;
        for (_, tx) in &transactions {
            if tx_hash_cursor.seek_exact(tx.hash())?.is_some() {
                tx_hash_cursor.delete_current()?;
            }
        }

        // Remove TransactionBlocks index if there are transaction present
        if !transactions.is_empty() {
            let tx_id_range = transactions.first().unwrap().0..=transactions.last().unwrap().0;
            self.remove::<tables::TransactionBlocks>(tx_id_range)?;
        }

        Ok(())
    }

    /// Get requested blocks transaction with senders, also removing them from the database
    ///
    /// This will remove block data for the given range from the following tables:
    /// * [`BlockBodyIndices`](tables::BlockBodyIndices)
    /// * [`Transactions`](tables::Transactions)
    /// * [`TransactionSenders`](tables::TransactionSenders)
    /// * [`TransactionHashNumbers`](tables::TransactionHashNumbers)
    /// * [`TransactionBlocks`](tables::TransactionBlocks)
    pub fn take_block_transaction_range(
        &self,
        range: impl RangeBounds<BlockNumber> + Clone,
    ) -> ProviderResult<Vec<(BlockNumber, Vec<TransactionSignedEcRecovered>)>> {
        // Raad range of block bodies to get all transactions id's of this range.
        let block_bodies = self.get::<tables::BlockBodyIndices>(range)?;

        if block_bodies.is_empty() {
            return Ok(Vec::new())
        }

        // Compute the first and last tx ID in the range
        let first_transaction = block_bodies.first().expect("If we have headers").1.first_tx_num();
        let last_transaction = block_bodies.last().expect("Not empty").1.last_tx_num();

        // If this is the case then all of the blocks in the range are empty
        if last_transaction < first_transaction {
            return Ok(block_bodies.into_iter().map(|(n, _)| (n, Vec::new())).collect())
        }

        // Get transactions and senders
        let transactions = self
            .take::<tables::Transactions>(first_transaction..=last_transaction)?
            .into_iter()
            .map(|(id, tx)| (id, tx.into()))
            .collect::<Vec<(u64, TransactionSigned)>>();

        let mut senders =
            self.take::<tables::TransactionSenders>(first_transaction..=last_transaction)?;

        recover_block_senders(&mut senders, &transactions, first_transaction, last_transaction)?;

        // Remove TransactionHashNumbers
        let mut tx_hash_cursor = self.tx.cursor_write::<tables::TransactionHashNumbers>()?;
        for (_, tx) in &transactions {
            if tx_hash_cursor.seek_exact(tx.hash())?.is_some() {
                tx_hash_cursor.delete_current()?;
            }
        }

        // Remove TransactionBlocks index if there are transaction present
        if !transactions.is_empty() {
            let tx_id_range = transactions.first().unwrap().0..=transactions.last().unwrap().0;
            self.remove::<tables::TransactionBlocks>(tx_id_range)?;
        }

        // Merge transaction into blocks
        let mut block_tx = Vec::with_capacity(block_bodies.len());
        let mut senders = senders.into_iter();
        let mut transactions = transactions.into_iter();
        for (block_number, block_body) in block_bodies {
            let mut one_block_tx = Vec::with_capacity(block_body.tx_count as usize);
            for _ in block_body.tx_num_range() {
                let tx = transactions.next();
                let sender = senders.next();

                let recovered = match (tx, sender) {
                    (Some((tx_id, tx)), Some((sender_tx_id, sender))) => {
                        if tx_id == sender_tx_id {
                            Ok(TransactionSignedEcRecovered::from_signed_transaction(tx, sender))
                        } else {
                            Err(ProviderError::MismatchOfTransactionAndSenderId { tx_id })
                        }
                    }
                    (Some((tx_id, _)), _) | (_, Some((tx_id, _))) => {
                        Err(ProviderError::MismatchOfTransactionAndSenderId { tx_id })
                    }
                    (None, None) => Err(ProviderError::BlockBodyTransactionCount),
                }?;
                one_block_tx.push(recovered)
            }
            block_tx.push((block_number, one_block_tx));
        }

        Ok(block_tx)
    }

    /// Remove the given range of blocks, without returning any of the blocks.
    ///
    /// This will remove block data for the given range from the following tables:
    /// * [`HeaderNumbers`](tables::HeaderNumbers)
    /// * [`CanonicalHeaders`](tables::CanonicalHeaders)
    /// * [`BlockOmmers`](tables::BlockOmmers)
    /// * [`BlockWithdrawals`](tables::BlockWithdrawals)
    /// * [`BlockRequests`](tables::BlockRequests)
    /// * [`HeaderTerminalDifficulties`](tables::HeaderTerminalDifficulties)
    ///
    /// This will also remove transaction data according to
    /// [`remove_block_transaction_range`](Self::remove_block_transaction_range).
    pub fn remove_block_range(
        &self,
        range: impl RangeBounds<BlockNumber> + Clone,
    ) -> ProviderResult<()> {
        let block_headers = self.remove::<tables::Headers>(range.clone())?;
        if block_headers == 0 {
            return Ok(())
        }

        self.tx.unwind_table_by_walker::<tables::CanonicalHeaders, tables::HeaderNumbers>(
            range.clone(),
        )?;
        self.remove::<tables::CanonicalHeaders>(range.clone())?;
        self.remove::<tables::BlockOmmers>(range.clone())?;
        self.remove::<tables::BlockWithdrawals>(range.clone())?;
        self.remove::<tables::BlockRequests>(range.clone())?;
        self.remove_block_transaction_range(range.clone())?;
        self.remove::<tables::HeaderTerminalDifficulties>(range)?;

        Ok(())
    }

    /// Remove the given range of blocks, and return them.
    ///
    /// This will remove block data for the given range from the following tables:
    /// * [`HeaderNumbers`](tables::HeaderNumbers)
    /// * [`CanonicalHeaders`](tables::CanonicalHeaders)
    /// * [`BlockOmmers`](tables::BlockOmmers)
    /// * [`BlockWithdrawals`](tables::BlockWithdrawals)
    /// * [`BlockRequests`](tables::BlockRequests)
    /// * [`HeaderTerminalDifficulties`](tables::HeaderTerminalDifficulties)
    ///
    /// This will also remove transaction data according to
    /// [`take_block_transaction_range`](Self::take_block_transaction_range).
    pub fn take_block_range(
        &self,
        range: impl RangeBounds<BlockNumber> + Clone,
    ) -> ProviderResult<Vec<SealedBlockWithSenders>>
    where
        Spec: EthereumHardforks,
    {
        // For blocks we need:
        //
        // - Headers
        // - Bodies (transactions)
        // - Uncles/ommers
        // - Withdrawals
        // - Requests
        // - Signers

        let block_headers = self.take::<tables::Headers>(range.clone())?;
        if block_headers.is_empty() {
            return Ok(Vec::new())
        }

        self.tx.unwind_table_by_walker::<tables::CanonicalHeaders, tables::HeaderNumbers>(
            range.clone(),
        )?;
        let block_header_hashes = self.take::<tables::CanonicalHeaders>(range.clone())?;
        let block_ommers = self.take::<tables::BlockOmmers>(range.clone())?;
        let block_withdrawals = self.take::<tables::BlockWithdrawals>(range.clone())?;
        let block_requests = self.take::<tables::BlockRequests>(range.clone())?;
        let block_tx = self.take_block_transaction_range(range.clone())?;

        let mut blocks = Vec::with_capacity(block_headers.len());

        // rm HeaderTerminalDifficulties
        self.remove::<tables::HeaderTerminalDifficulties>(range)?;

        // merge all into block
        let block_header_iter = block_headers.into_iter();
        let block_header_hashes_iter = block_header_hashes.into_iter();
        let block_tx_iter = block_tx.into_iter();

        // Ommers can be empty for some blocks
        let mut block_ommers_iter = block_ommers.into_iter();
        let mut block_withdrawals_iter = block_withdrawals.into_iter();
        let mut block_requests_iter = block_requests.into_iter();
        let mut block_ommers = block_ommers_iter.next();
        let mut block_withdrawals = block_withdrawals_iter.next();
        let mut block_requests = block_requests_iter.next();

        for ((main_block_number, header), (_, header_hash), (_, tx)) in
            izip!(block_header_iter, block_header_hashes_iter, block_tx_iter)
        {
            let header = SealedHeader::new(header, header_hash);

            let (transactions, senders) = tx.into_iter().map(|tx| tx.to_components()).unzip();

            // Ommers can be missing
            let mut ommers = Vec::new();
            if let Some((block_number, _)) = block_ommers.as_ref() {
                if *block_number == main_block_number {
                    ommers = block_ommers.take().unwrap().1.ommers;
                    block_ommers = block_ommers_iter.next();
                }
            };

            // withdrawal can be missing
            let shanghai_is_active =
                self.chain_spec.is_shanghai_active_at_timestamp(header.timestamp);
            let mut withdrawals = Some(Withdrawals::default());
            if shanghai_is_active {
                if let Some((block_number, _)) = block_withdrawals.as_ref() {
                    if *block_number == main_block_number {
                        withdrawals = Some(block_withdrawals.take().unwrap().1.withdrawals);
                        block_withdrawals = block_withdrawals_iter.next();
                    }
                }
            } else {
                withdrawals = None
            }

            // requests can be missing
            let prague_is_active = self.chain_spec.is_prague_active_at_timestamp(header.timestamp);
            let mut requests = Some(Requests::default());
            if prague_is_active {
                if let Some((block_number, _)) = block_requests.as_ref() {
                    if *block_number == main_block_number {
                        requests = Some(block_requests.take().unwrap().1);
                        block_requests = block_requests_iter.next();
                    }
                }
            } else {
                requests = None;
            }

            blocks.push(SealedBlockWithSenders {
                block: SealedBlock {
                    header,
                    body: BlockBody { transactions, ommers, withdrawals, requests },
                },
                senders,
            })
        }

        Ok(blocks)
    }

    /// Load shard and remove it. If list is empty, last shard was full or
    /// there are no shards at all.
    fn take_shard<T>(&self, key: T::Key) -> ProviderResult<Vec<u64>>
    where
        T: Table<Value = BlockNumberList>,
    {
        let mut cursor = self.tx.cursor_read::<T>()?;
        let shard = cursor.seek_exact(key)?;
        if let Some((shard_key, list)) = shard {
            // delete old shard so new one can be inserted.
            self.tx.delete::<T>(shard_key, None)?;
            let list = list.iter().collect::<Vec<_>>();
            return Ok(list)
        }
        Ok(Vec::new())
    }

    /// Insert history index to the database.
    ///
    /// For each updated partial key, this function removes the last shard from
    /// the database (if any), appends the new indices to it, chunks the resulting integer list and
    /// inserts the new shards back into the database.
    ///
    /// This function is used by history indexing stages.
    fn append_history_index<P, T>(
        &self,
        index_updates: impl IntoIterator<Item = (P, impl IntoIterator<Item = u64>)>,
        mut sharded_key_factory: impl FnMut(P, BlockNumber) -> T::Key,
    ) -> ProviderResult<()>
    where
        P: Copy,
        T: Table<Value = BlockNumberList>,
    {
        for (partial_key, indices) in index_updates {
            let mut last_shard =
                self.take_shard::<T>(sharded_key_factory(partial_key, u64::MAX))?;
            last_shard.extend(indices);
            // Chunk indices and insert them in shards of N size.
            let indices = last_shard;
            let mut chunks = indices.chunks(sharded_key::NUM_OF_INDICES_IN_SHARD).peekable();
            while let Some(list) = chunks.next() {
                let highest_block_number = if chunks.peek().is_some() {
                    *list.last().expect("`chunks` does not return empty list")
                } else {
                    // Insert last list with `u64::MAX`.
                    u64::MAX
                };
                self.tx.put::<T>(
                    sharded_key_factory(partial_key, highest_block_number),
                    BlockNumberList::new_pre_sorted(list.iter().copied()),
                )?;
            }
        }
        Ok(())
    }
}

impl<TX: DbTx, Spec: Send + Sync> AccountReader for DatabaseProvider<TX, Spec> {
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        Ok(self.tx.get::<tables::PlainAccountState>(address)?)
    }
}

impl<TX: DbTx, Spec: Send + Sync> AccountExtReader for DatabaseProvider<TX, Spec> {
    fn changed_accounts_with_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<BTreeSet<Address>> {
        self.tx
            .cursor_read::<tables::AccountChangeSets>()?
            .walk_range(range)?
            .map(|entry| {
                entry.map(|(_, account_before)| account_before.address).map_err(Into::into)
            })
            .collect()
    }

    fn basic_accounts(
        &self,
        iter: impl IntoIterator<Item = Address>,
    ) -> ProviderResult<Vec<(Address, Option<Account>)>> {
        let mut plain_accounts = self.tx.cursor_read::<tables::PlainAccountState>()?;
        Ok(iter
            .into_iter()
            .map(|address| plain_accounts.seek_exact(address).map(|a| (address, a.map(|(_, v)| v))))
            .collect::<Result<Vec<_>, _>>()?)
    }

    fn changed_accounts_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, Vec<u64>>> {
        let mut changeset_cursor = self.tx.cursor_read::<tables::AccountChangeSets>()?;

        let account_transitions = changeset_cursor.walk_range(range)?.try_fold(
            BTreeMap::new(),
            |mut accounts: BTreeMap<Address, Vec<u64>>, entry| -> ProviderResult<_> {
                let (index, account) = entry?;
                accounts.entry(account.address).or_default().push(index);
                Ok(accounts)
            },
        )?;

        Ok(account_transitions)
    }
}

impl<TX: DbTx, Spec: Send + Sync> StorageChangeSetReader for DatabaseProvider<TX, Spec> {
    fn storage_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>> {
        let range = block_number..=block_number;
        let storage_range = BlockNumberAddress::range(range);
        self.tx
            .cursor_dup_read::<tables::StorageChangeSets>()?
            .walk_range(storage_range)?
            .map(|result| -> ProviderResult<_> { Ok(result?) })
            .collect()
    }
}

impl<TX: DbTx, Spec: Send + Sync> ChangeSetReader for DatabaseProvider<TX, Spec> {
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        let range = block_number..=block_number;
        self.tx
            .cursor_read::<tables::AccountChangeSets>()?
            .walk_range(range)?
            .map(|result| -> ProviderResult<_> {
                let (_, account_before) = result?;
                Ok(account_before)
            })
            .collect()
    }
}

impl<TX: DbTx, Spec: Send + Sync> HeaderSyncGapProvider for DatabaseProvider<TX, Spec> {
    fn sync_gap(
        &self,
        tip: watch::Receiver<B256>,
        highest_uninterrupted_block: BlockNumber,
    ) -> ProviderResult<HeaderSyncGap> {
        let static_file_provider = self.static_file_provider();

        // Make sure Headers static file is at the same height. If it's further, this
        // input execution was interrupted previously and we need to unwind the static file.
        let next_static_file_block_num = static_file_provider
            .get_highest_static_file_block(StaticFileSegment::Headers)
            .map(|id| id + 1)
            .unwrap_or_default();
        let next_block = highest_uninterrupted_block + 1;

        match next_static_file_block_num.cmp(&next_block) {
            // The node shutdown between an executed static file commit and before the database
            // commit, so we need to unwind the static files.
            Ordering::Greater => {
                let mut static_file_producer =
                    static_file_provider.latest_writer(StaticFileSegment::Headers)?;
                static_file_producer.prune_headers(next_static_file_block_num - next_block)?;
                // Since this is a database <-> static file inconsistency, we commit the change
                // straight away.
                static_file_producer.commit()?
            }
            Ordering::Less => {
                // There's either missing or corrupted files.
                return Err(ProviderError::HeaderNotFound(next_static_file_block_num.into()))
            }
            Ordering::Equal => {}
        }

        let local_head = static_file_provider
            .sealed_header(highest_uninterrupted_block)?
            .ok_or_else(|| ProviderError::HeaderNotFound(highest_uninterrupted_block.into()))?;

        let target = SyncTarget::Tip(*tip.borrow());

        Ok(HeaderSyncGap { local_head, target })
    }
}

impl<TX: DbTx, Spec: Send + Sync + EthereumHardforks> HeaderProvider
    for DatabaseProvider<TX, Spec>
{
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        if let Some(num) = self.block_number(*block_hash)? {
            Ok(self.header_by_number(num)?)
        } else {
            Ok(None)
        }
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Header>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            num,
            |static_file| static_file.header_by_number(num),
            || Ok(self.tx.get::<tables::Headers>(num)?),
        )
    }

    fn header_td(&self, block_hash: &BlockHash) -> ProviderResult<Option<U256>> {
        if let Some(num) = self.block_number(*block_hash)? {
            self.header_td_by_number(num)
        } else {
            Ok(None)
        }
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        if let Some(td) = self.chain_spec.final_paris_total_difficulty(number) {
            // if this block is higher than the final paris(merge) block, return the final paris
            // difficulty
            return Ok(Some(td))
        }

        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.header_td_by_number(number),
            || Ok(self.tx.get::<tables::HeaderTerminalDifficulties>(number)?.map(|td| td.0)),
        )
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            to_range(range),
            |static_file, range, _| static_file.headers_range(range),
            |range, _| self.cursor_read_collect::<tables::Headers>(range).map_err(Into::into),
            |_| true,
        )
    }

    fn sealed_header(&self, number: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.sealed_header(number),
            || {
                if let Some(header) = self.header_by_number(number)? {
                    let hash = self
                        .block_hash(number)?
                        .ok_or_else(|| ProviderError::HeaderNotFound(number.into()))?;
                    Ok(Some(SealedHeader::new(header, hash)))
                } else {
                    Ok(None)
                }
            },
        )
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            to_range(range),
            |static_file, range, predicate| static_file.sealed_headers_while(range, predicate),
            |range, mut predicate| {
                let mut headers = vec![];
                for entry in self.tx.cursor_read::<tables::Headers>()?.walk_range(range)? {
                    let (number, header) = entry?;
                    let hash = self
                        .block_hash(number)?
                        .ok_or_else(|| ProviderError::HeaderNotFound(number.into()))?;
                    let sealed = SealedHeader::new(header, hash);
                    if !predicate(&sealed) {
                        break
                    }
                    headers.push(sealed);
                }
                Ok(headers)
            },
            predicate,
        )
    }
}

impl<TX: DbTx, Spec: Send + Sync> BlockHashReader for DatabaseProvider<TX, Spec> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.block_hash(number),
            || Ok(self.tx.get::<tables::CanonicalHeaders>(number)?),
        )
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            start..end,
            |static_file, range, _| static_file.canonical_hashes_range(range.start, range.end),
            |range, _| {
                self.cursor_read_collect::<tables::CanonicalHeaders>(range).map_err(Into::into)
            },
            |_| true,
        )
    }
}

impl<TX: DbTx, Spec: Send + Sync> BlockNumReader for DatabaseProvider<TX, Spec> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        let best_number = self.best_block_number()?;
        let best_hash = self.block_hash(best_number)?.unwrap_or_default();
        Ok(ChainInfo { best_hash, best_number })
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(self
            .get_stage_checkpoint(StageId::Finish)?
            .map(|checkpoint| checkpoint.block_number)
            .unwrap_or_default())
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(self
            .tx
            .cursor_read::<tables::CanonicalHeaders>()?
            .last()?
            .map(|(num, _)| num)
            .max(
                self.static_file_provider.get_highest_static_file_block(StaticFileSegment::Headers),
            )
            .unwrap_or_default())
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        Ok(self.tx.get::<tables::HeaderNumbers>(hash)?)
    }
}

impl<TX: DbTx, Spec: Send + Sync + EthereumHardforks> BlockReader for DatabaseProvider<TX, Spec> {
    fn find_block_by_hash(&self, hash: B256, source: BlockSource) -> ProviderResult<Option<Block>> {
        if source.is_canonical() {
            self.block(hash.into())
        } else {
            Ok(None)
        }
    }

    /// Returns the block with matching number from database.
    ///
    /// If the header for this block is not found, this returns `None`.
    /// If the header is found, but the transactions either do not exist, or are not indexed, this
    /// will return None.
    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        if let Some(number) = self.convert_hash_or_number(id)? {
            if let Some(header) = self.header_by_number(number)? {
                let withdrawals = self.withdrawals_by_block(number.into(), header.timestamp)?;
                let ommers = self.ommers(number.into())?.unwrap_or_default();
                let requests = self.requests_by_block(number.into(), header.timestamp)?;
                // If the body indices are not found, this means that the transactions either do not
                // exist in the database yet, or they do exit but are not indexed.
                // If they exist but are not indexed, we don't have enough
                // information to return the block anyways, so we return `None`.
                let transactions = match self.transactions_by_block(number.into())? {
                    Some(transactions) => transactions,
                    None => return Ok(None),
                };

                return Ok(Some(Block {
                    header,
                    body: BlockBody { transactions, ommers, withdrawals, requests },
                }))
            }
        }

        Ok(None)
    }

    fn pending_block(&self) -> ProviderResult<Option<SealedBlock>> {
        Ok(None)
    }

    fn pending_block_with_senders(&self) -> ProviderResult<Option<SealedBlockWithSenders>> {
        Ok(None)
    }

    fn pending_block_and_receipts(&self) -> ProviderResult<Option<(SealedBlock, Vec<Receipt>)>> {
        Ok(None)
    }

    /// Returns the ommers for the block with matching id from the database.
    ///
    /// If the block is not found, this returns `None`.
    /// If the block exists, but doesn't contain ommers, this returns `None`.
    fn ommers(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>> {
        if let Some(number) = self.convert_hash_or_number(id)? {
            // If the Paris (Merge) hardfork block is known and block is after it, return empty
            // ommers.
            if self.chain_spec.final_paris_total_difficulty(number).is_some() {
                return Ok(Some(Vec::new()))
            }

            let ommers = self.tx.get::<tables::BlockOmmers>(number)?.map(|o| o.ommers);
            return Ok(ommers)
        }

        Ok(None)
    }

    fn block_body_indices(&self, num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        Ok(self.tx.get::<tables::BlockBodyIndices>(num)?)
    }

    /// Returns the block with senders with matching number or hash from database.
    ///
    /// **NOTE: The transactions have invalid hashes, since they would need to be calculated on the
    /// spot, and we want fast querying.**
    ///
    /// If the header for this block is not found, this returns `None`.
    /// If the header is found, but the transactions either do not exist, or are not indexed, this
    /// will return None.
    fn block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders>> {
        self.block_with_senders(
            id,
            transaction_kind,
            |block_number| self.header_by_number(block_number),
            |header, transactions, senders, ommers, withdrawals, requests| {
                Block { header, body: BlockBody { transactions, ommers, withdrawals, requests } }
                    // Note: we're using unchecked here because we know the block contains valid txs
                    // wrt to its height and can ignore the s value check so pre
                    // EIP-2 txs are allowed
                    .try_with_senders_unchecked(senders)
                    .map(Some)
                    .map_err(|_| ProviderError::SenderRecoveryError)
            },
        )
    }

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<SealedBlockWithSenders>> {
        self.block_with_senders(
            id,
            transaction_kind,
            |block_number| self.sealed_header(block_number),
            |header, transactions, senders, ommers, withdrawals, requests| {
                SealedBlock {
                    header,
                    body: BlockBody { transactions, ommers, withdrawals, requests },
                }
                // Note: we're using unchecked here because we know the block contains valid txs
                // wrt to its height and can ignore the s value check so pre
                // EIP-2 txs are allowed
                .try_with_senders_unchecked(senders)
                .map(Some)
                .map_err(|_| ProviderError::SenderRecoveryError)
            },
        )
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        let mut tx_cursor = self.tx.cursor_read::<tables::Transactions>()?;
        self.block_range(
            range,
            |range| self.headers_range(range),
            |header, tx_range, ommers, withdrawals, requests| {
                let transactions = if tx_range.is_empty() {
                    Vec::new()
                } else {
                    self.transactions_by_tx_range_with_cursor(tx_range, &mut tx_cursor)?
                        .into_iter()
                        .map(Into::into)
                        .collect()
                };
                Ok(Block {
                    header,
                    body: BlockBody { transactions, ommers, withdrawals, requests },
                })
            },
        )
    }

    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockWithSenders>> {
        self.block_with_senders_range(
            range,
            |range| self.headers_range(range),
            |header, transactions, ommers, withdrawals, requests, senders| {
                Block { header, body: BlockBody { transactions, ommers, withdrawals, requests } }
                    .try_with_senders_unchecked(senders)
                    .map_err(|_| ProviderError::SenderRecoveryError)
            },
        )
    }

    fn sealed_block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<SealedBlockWithSenders>> {
        self.block_with_senders_range(
            range,
            |range| self.sealed_headers_range(range),
            |header, transactions, ommers, withdrawals, requests, senders| {
                SealedBlockWithSenders::new(
                    SealedBlock {
                        header,
                        body: BlockBody { transactions, ommers, withdrawals, requests },
                    },
                    senders,
                )
                .ok_or(ProviderError::SenderRecoveryError)
            },
        )
    }
}

impl<TX: DbTx, Spec: Send + Sync + EthereumHardforks> TransactionsProviderExt
    for DatabaseProvider<TX, Spec>
{
    /// Recovers transaction hashes by walking through `Transactions` table and
    /// calculating them in a parallel manner. Returned unsorted.
    fn transaction_hashes_by_range(
        &self,
        tx_range: Range<TxNumber>,
    ) -> ProviderResult<Vec<(TxHash, TxNumber)>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Transactions,
            tx_range,
            |static_file, range, _| static_file.transaction_hashes_by_range(range),
            |tx_range, _| {
                let mut tx_cursor = self.tx.cursor_read::<tables::Transactions>()?;
                let tx_range_size = tx_range.clone().count();
                let tx_walker = tx_cursor.walk_range(tx_range)?;

                let chunk_size = (tx_range_size / rayon::current_num_threads()).max(1);
                let mut channels = Vec::with_capacity(chunk_size);
                let mut transaction_count = 0;

                #[inline]
                fn calculate_hash(
                    entry: Result<(TxNumber, TransactionSignedNoHash), DatabaseError>,
                    rlp_buf: &mut Vec<u8>,
                ) -> Result<(B256, TxNumber), Box<ProviderError>> {
                    let (tx_id, tx) = entry.map_err(|e| Box::new(e.into()))?;
                    tx.transaction.encode_with_signature(&tx.signature, rlp_buf, false);
                    Ok((keccak256(rlp_buf), tx_id))
                }

                for chunk in &tx_walker.chunks(chunk_size) {
                    let (tx, rx) = mpsc::channel();
                    channels.push(rx);

                    // Note: Unfortunate side-effect of how chunk is designed in itertools (it is
                    // not Send)
                    let chunk: Vec<_> = chunk.collect();
                    transaction_count += chunk.len();

                    // Spawn the task onto the global rayon pool
                    // This task will send the results through the channel after it has calculated
                    // the hash.
                    rayon::spawn(move || {
                        let mut rlp_buf = Vec::with_capacity(128);
                        for entry in chunk {
                            rlp_buf.clear();
                            let _ = tx.send(calculate_hash(entry, &mut rlp_buf));
                        }
                    });
                }
                let mut tx_list = Vec::with_capacity(transaction_count);

                // Iterate over channels and append the tx hashes unsorted
                for channel in channels {
                    while let Ok(tx) = channel.recv() {
                        let (tx_hash, tx_id) = tx.map_err(|boxed| *boxed)?;
                        tx_list.push((tx_hash, tx_id));
                    }
                }

                Ok(tx_list)
            },
            |_| true,
        )
    }
}

// Calculates the hash of the given transaction
impl<TX: DbTx, Spec: Send + Sync + EthereumHardforks> TransactionsProvider
    for DatabaseProvider<TX, Spec>
{
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        Ok(self.tx.get::<tables::TransactionHashNumbers>(tx_hash)?)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Transactions,
            id,
            |static_file| static_file.transaction_by_id(id),
            || Ok(self.tx.get::<tables::Transactions>(id)?.map(Into::into)),
        )
    }

    fn transaction_by_id_no_hash(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Transactions,
            id,
            |static_file| static_file.transaction_by_id_no_hash(id),
            || Ok(self.tx.get::<tables::Transactions>(id)?),
        )
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<TransactionSigned>> {
        if let Some(id) = self.transaction_id(hash)? {
            Ok(self.transaction_by_id_no_hash(id)?.map(|tx| TransactionSigned {
                hash,
                signature: tx.signature,
                transaction: tx.transaction,
            }))
        } else {
            Ok(None)
        }
        .map(|tx| tx.map(Into::into))
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(TransactionSigned, TransactionMeta)>> {
        let mut transaction_cursor = self.tx.cursor_read::<tables::TransactionBlocks>()?;
        if let Some(transaction_id) = self.transaction_id(tx_hash)? {
            if let Some(tx) = self.transaction_by_id_no_hash(transaction_id)? {
                let transaction = TransactionSigned {
                    hash: tx_hash,
                    signature: tx.signature,
                    transaction: tx.transaction,
                };
                if let Some(block_number) =
                    transaction_cursor.seek(transaction_id).map(|b| b.map(|(_, bn)| bn))?
                {
                    if let Some(sealed_header) = self.sealed_header(block_number)? {
                        let (header, block_hash) = sealed_header.split();
                        if let Some(block_body) = self.block_body_indices(block_number)? {
                            // the index of the tx in the block is the offset:
                            // len([start..tx_id])
                            // NOTE: `transaction_id` is always `>=` the block's first
                            // index
                            let index = transaction_id - block_body.first_tx_num();

                            let meta = TransactionMeta {
                                tx_hash,
                                index,
                                block_hash,
                                block_number,
                                base_fee: header.base_fee_per_gas,
                                excess_blob_gas: header.excess_blob_gas,
                                timestamp: header.timestamp,
                            };

                            return Ok(Some((transaction, meta)))
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    fn transaction_block(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        let mut cursor = self.tx.cursor_read::<tables::TransactionBlocks>()?;
        Ok(cursor.seek(id)?.map(|(_, bn)| bn))
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        let mut tx_cursor = self.tx.cursor_read::<tables::Transactions>()?;

        if let Some(block_number) = self.convert_hash_or_number(id)? {
            if let Some(body) = self.block_body_indices(block_number)? {
                let tx_range = body.tx_num_range();
                return if tx_range.is_empty() {
                    Ok(Some(Vec::new()))
                } else {
                    Ok(Some(
                        self.transactions_by_tx_range_with_cursor(tx_range, &mut tx_cursor)?
                            .into_iter()
                            .map(Into::into)
                            .collect(),
                    ))
                }
            }
        }
        Ok(None)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<TransactionSigned>>> {
        let mut tx_cursor = self.tx.cursor_read::<tables::Transactions>()?;
        let mut results = Vec::new();
        let mut body_cursor = self.tx.cursor_read::<tables::BlockBodyIndices>()?;
        for entry in body_cursor.walk_range(range)? {
            let (_, body) = entry?;
            let tx_num_range = body.tx_num_range();
            if tx_num_range.is_empty() {
                results.push(Vec::new());
            } else {
                results.push(
                    self.transactions_by_tx_range_with_cursor(tx_num_range, &mut tx_cursor)?
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                );
            }
        }
        Ok(results)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<TransactionSignedNoHash>> {
        self.transactions_by_tx_range_with_cursor(
            range,
            &mut self.tx.cursor_read::<tables::Transactions>()?,
        )
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        self.cursor_read_collect::<tables::TransactionSenders>(range).map_err(Into::into)
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        Ok(self.tx.get::<tables::TransactionSenders>(id)?)
    }
}

impl<TX: DbTx, Spec: Send + Sync + EthereumHardforks> ReceiptProvider
    for DatabaseProvider<TX, Spec>
{
    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Receipt>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Receipts,
            id,
            |static_file| static_file.receipt(id),
            || Ok(self.tx.get::<tables::Receipts>(id)?),
        )
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Receipt>> {
        if let Some(id) = self.transaction_id(hash)? {
            self.receipt(id)
        } else {
            Ok(None)
        }
    }

    fn receipts_by_block(&self, block: BlockHashOrNumber) -> ProviderResult<Option<Vec<Receipt>>> {
        if let Some(number) = self.convert_hash_or_number(block)? {
            if let Some(body) = self.block_body_indices(number)? {
                let tx_range = body.tx_num_range();
                return if tx_range.is_empty() {
                    Ok(Some(Vec::new()))
                } else {
                    self.receipts_by_tx_range(tx_range).map(Some)
                }
            }
        }
        Ok(None)
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Receipt>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Receipts,
            to_range(range),
            |static_file, range, _| static_file.receipts_by_tx_range(range),
            |range, _| self.cursor_read_collect::<tables::Receipts>(range).map_err(Into::into),
            |_| true,
        )
    }
}

impl<TX: DbTx, Spec: Send + Sync + EthereumHardforks> WithdrawalsProvider
    for DatabaseProvider<TX, Spec>
{
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<Withdrawals>> {
        if self.chain_spec.is_shanghai_active_at_timestamp(timestamp) {
            if let Some(number) = self.convert_hash_or_number(id)? {
                // If we are past shanghai, then all blocks should have a withdrawal list, even if
                // empty
                let withdrawals = self
                    .tx
                    .get::<tables::BlockWithdrawals>(number)
                    .map(|w| w.map(|w| w.withdrawals))?
                    .unwrap_or_default();
                return Ok(Some(withdrawals))
            }
        }
        Ok(None)
    }

    fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> {
        let latest_block_withdrawal = self.tx.cursor_read::<tables::BlockWithdrawals>()?.last()?;
        Ok(latest_block_withdrawal
            .and_then(|(_, mut block_withdrawal)| block_withdrawal.withdrawals.pop()))
    }
}

impl<TX: DbTx, Spec: Send + Sync + EthereumHardforks> RequestsProvider
    for DatabaseProvider<TX, Spec>
{
    fn requests_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<Requests>> {
        if self.chain_spec.is_prague_active_at_timestamp(timestamp) {
            if let Some(number) = self.convert_hash_or_number(id)? {
                // If we are past Prague, then all blocks should have a requests list, even if
                // empty
                let requests = self.tx.get::<tables::BlockRequests>(number)?.unwrap_or_default();
                return Ok(Some(requests))
            }
        }
        Ok(None)
    }
}

impl<TX: DbTx, Spec: Send + Sync + EthereumHardforks> EvmEnvProvider
    for DatabaseProvider<TX, Spec>
{
    fn fill_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        let hash = self.convert_number(at)?.ok_or(ProviderError::HeaderNotFound(at))?;
        let header = self.header(&hash)?.ok_or(ProviderError::HeaderNotFound(at))?;
        self.fill_env_with_header(cfg, block_env, &header, evm_config)
    }

    fn fill_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        let total_difficulty = self
            .header_td_by_number(header.number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(header.number.into()))?;
        evm_config.fill_cfg_and_block_env(cfg, block_env, header, total_difficulty);
        Ok(())
    }

    fn fill_cfg_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        let hash = self.convert_number(at)?.ok_or(ProviderError::HeaderNotFound(at))?;
        let header = self.header(&hash)?.ok_or(ProviderError::HeaderNotFound(at))?;
        self.fill_cfg_env_with_header(cfg, &header, evm_config)
    }

    fn fill_cfg_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        let total_difficulty = self
            .header_td_by_number(header.number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(header.number.into()))?;
        evm_config.fill_cfg_env(cfg, header, total_difficulty);
        Ok(())
    }
}

impl<TX: DbTx, Spec: Send + Sync> StageCheckpointReader for DatabaseProvider<TX, Spec> {
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        Ok(self.tx.get::<tables::StageCheckpoints>(id.to_string())?)
    }

    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
        self.tx
            .cursor_read::<tables::StageCheckpoints>()?
            .walk(None)?
            .collect::<Result<Vec<(String, StageCheckpoint)>, _>>()
            .map_err(ProviderError::Database)
    }

    /// Get stage checkpoint progress.
    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        Ok(self.tx.get::<tables::StageCheckpointProgresses>(id.to_string())?)
    }
}

impl<TX: DbTxMut, Spec: Send + Sync> StageCheckpointWriter for DatabaseProvider<TX, Spec> {
    /// Save stage checkpoint.
    fn save_stage_checkpoint(
        &self,
        id: StageId,
        checkpoint: StageCheckpoint,
    ) -> ProviderResult<()> {
        Ok(self.tx.put::<tables::StageCheckpoints>(id.to_string(), checkpoint)?)
    }

    /// Save stage checkpoint progress.
    fn save_stage_checkpoint_progress(
        &self,
        id: StageId,
        checkpoint: Vec<u8>,
    ) -> ProviderResult<()> {
        Ok(self.tx.put::<tables::StageCheckpointProgresses>(id.to_string(), checkpoint)?)
    }

    fn update_pipeline_stages(
        &self,
        block_number: BlockNumber,
        drop_stage_checkpoint: bool,
    ) -> ProviderResult<()> {
        // iterate over all existing stages in the table and update its progress.
        let mut cursor = self.tx.cursor_write::<tables::StageCheckpoints>()?;
        for stage_id in StageId::ALL {
            let (_, checkpoint) = cursor.seek_exact(stage_id.to_string())?.unwrap_or_default();
            cursor.upsert(
                stage_id.to_string(),
                StageCheckpoint {
                    block_number,
                    ..if drop_stage_checkpoint { Default::default() } else { checkpoint }
                },
            )?;
        }

        Ok(())
    }
}

impl<TX: DbTx, Spec: Send + Sync> StorageReader for DatabaseProvider<TX, Spec> {
    fn plain_state_storages(
        &self,
        addresses_with_keys: impl IntoIterator<Item = (Address, impl IntoIterator<Item = B256>)>,
    ) -> ProviderResult<Vec<(Address, Vec<StorageEntry>)>> {
        let mut plain_storage = self.tx.cursor_dup_read::<tables::PlainStorageState>()?;

        addresses_with_keys
            .into_iter()
            .map(|(address, storage)| {
                storage
                    .into_iter()
                    .map(|key| -> ProviderResult<_> {
                        Ok(plain_storage
                            .seek_by_key_subkey(address, key)?
                            .filter(|v| v.key == key)
                            .unwrap_or_else(|| StorageEntry { key, value: Default::default() }))
                    })
                    .collect::<ProviderResult<Vec<_>>>()
                    .map(|storage| (address, storage))
            })
            .collect::<ProviderResult<Vec<(_, _)>>>()
    }

    fn changed_storages_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, BTreeSet<B256>>> {
        self.tx
            .cursor_read::<tables::StorageChangeSets>()?
            .walk_range(BlockNumberAddress::range(range))?
            // fold all storages and save its old state so we can remove it from HashedStorage
            // it is needed as it is dup table.
            .try_fold(BTreeMap::new(), |mut accounts: BTreeMap<Address, BTreeSet<B256>>, entry| {
                let (BlockNumberAddress((_, address)), storage_entry) = entry?;
                accounts.entry(address).or_default().insert(storage_entry.key);
                Ok(accounts)
            })
    }

    fn changed_storages_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<(Address, B256), Vec<u64>>> {
        let mut changeset_cursor = self.tx.cursor_read::<tables::StorageChangeSets>()?;

        let storage_changeset_lists =
            changeset_cursor.walk_range(BlockNumberAddress::range(range))?.try_fold(
                BTreeMap::new(),
                |mut storages: BTreeMap<(Address, B256), Vec<u64>>, entry| -> ProviderResult<_> {
                    let (index, storage) = entry?;
                    storages
                        .entry((index.address(), storage.key))
                        .or_default()
                        .push(index.block_number());
                    Ok(storages)
                },
            )?;

        Ok(storage_changeset_lists)
    }
}

impl<TX: DbTxMut + DbTx, Spec: Send + Sync> StateChangeWriter for DatabaseProvider<TX, Spec> {
    fn write_state_reverts(
        &self,
        reverts: PlainStateReverts,
        first_block: BlockNumber,
    ) -> ProviderResult<()> {
        // Write storage changes
        tracing::trace!("Writing storage changes");
        let mut storages_cursor = self.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
        let mut storage_changeset_cursor =
            self.tx_ref().cursor_dup_write::<tables::StorageChangeSets>()?;
        for (block_index, mut storage_changes) in reverts.storage.into_iter().enumerate() {
            let block_number = first_block + block_index as BlockNumber;

            tracing::trace!(block_number, "Writing block change");
            // sort changes by address.
            storage_changes.par_sort_unstable_by_key(|a| a.address);
            for PlainStorageRevert { address, wiped, storage_revert } in storage_changes {
                let storage_id = BlockNumberAddress((block_number, address));

                let mut storage = storage_revert
                    .into_iter()
                    .map(|(k, v)| (B256::new(k.to_be_bytes()), v))
                    .collect::<Vec<_>>();
                // sort storage slots by key.
                storage.par_sort_unstable_by_key(|a| a.0);

                // If we are writing the primary storage wipe transition, the pre-existing plain
                // storage state has to be taken from the database and written to storage history.
                // See [StorageWipe::Primary] for more details.
                let mut wiped_storage = Vec::new();
                if wiped {
                    tracing::trace!(?address, "Wiping storage");
                    if let Some((_, entry)) = storages_cursor.seek_exact(address)? {
                        wiped_storage.push((entry.key, entry.value));
                        while let Some(entry) = storages_cursor.next_dup_val()? {
                            wiped_storage.push((entry.key, entry.value))
                        }
                    }
                }

                tracing::trace!(?address, ?storage, "Writing storage reverts");
                for (key, value) in StorageRevertsIter::new(storage, wiped_storage) {
                    storage_changeset_cursor.append_dup(storage_id, StorageEntry { key, value })?;
                }
            }
        }

        // Write account changes
        tracing::trace!("Writing account changes");
        let mut account_changeset_cursor =
            self.tx_ref().cursor_dup_write::<tables::AccountChangeSets>()?;

        for (block_index, mut account_block_reverts) in reverts.accounts.into_iter().enumerate() {
            let block_number = first_block + block_index as BlockNumber;
            // Sort accounts by address.
            account_block_reverts.par_sort_by_key(|a| a.0);

            for (address, info) in account_block_reverts {
                account_changeset_cursor.append_dup(
                    block_number,
                    AccountBeforeTx { address, info: info.map(Into::into) },
                )?;
            }
        }

        Ok(())
    }

    fn write_state_changes(&self, mut changes: StateChangeset) -> ProviderResult<()> {
        // sort all entries so they can be written to database in more performant way.
        // and take smaller memory footprint.
        changes.accounts.par_sort_by_key(|a| a.0);
        changes.storage.par_sort_by_key(|a| a.address);
        changes.contracts.par_sort_by_key(|a| a.0);

        // Write new account state
        tracing::trace!(len = changes.accounts.len(), "Writing new account state");
        let mut accounts_cursor = self.tx_ref().cursor_write::<tables::PlainAccountState>()?;
        // write account to database.
        for (address, account) in changes.accounts {
            if let Some(account) = account {
                tracing::trace!(?address, "Updating plain state account");
                accounts_cursor.upsert(address, account.into())?;
            } else if accounts_cursor.seek_exact(address)?.is_some() {
                tracing::trace!(?address, "Deleting plain state account");
                accounts_cursor.delete_current()?;
            }
        }

        // Write bytecode
        tracing::trace!(len = changes.contracts.len(), "Writing bytecodes");
        let mut bytecodes_cursor = self.tx_ref().cursor_write::<tables::Bytecodes>()?;
        for (hash, bytecode) in changes.contracts {
            bytecodes_cursor.upsert(hash, Bytecode(bytecode))?;
        }

        // Write new storage state and wipe storage if needed.
        tracing::trace!(len = changes.storage.len(), "Writing new storage state");
        let mut storages_cursor = self.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
        for PlainStorageChangeset { address, wipe_storage, storage } in changes.storage {
            // Wiping of storage.
            if wipe_storage && storages_cursor.seek_exact(address)?.is_some() {
                storages_cursor.delete_current_duplicates()?;
            }
            // cast storages to B256.
            let mut storage = storage
                .into_iter()
                .map(|(k, value)| StorageEntry { key: k.into(), value })
                .collect::<Vec<_>>();
            // sort storage slots by key.
            storage.par_sort_unstable_by_key(|a| a.key);

            for entry in storage {
                tracing::trace!(?address, ?entry.key, "Updating plain state storage");
                if let Some(db_entry) = storages_cursor.seek_by_key_subkey(address, entry.key)? {
                    if db_entry.key == entry.key {
                        storages_cursor.delete_current()?;
                    }
                }

                if !entry.value.is_zero() {
                    storages_cursor.upsert(address, entry)?;
                }
            }
        }

        Ok(())
    }

    fn write_hashed_state(&self, hashed_state: &HashedPostStateSorted) -> ProviderResult<()> {
        // Write hashed account updates.
        let mut hashed_accounts_cursor = self.tx_ref().cursor_write::<tables::HashedAccounts>()?;
        for (hashed_address, account) in hashed_state.accounts().accounts_sorted() {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(hashed_address, account)?;
            } else if hashed_accounts_cursor.seek_exact(hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }

        // Write hashed storage changes.
        let sorted_storages = hashed_state.account_storages().iter().sorted_by_key(|(key, _)| *key);
        let mut hashed_storage_cursor =
            self.tx_ref().cursor_dup_write::<tables::HashedStorages>()?;
        for (hashed_address, storage) in sorted_storages {
            if storage.is_wiped() && hashed_storage_cursor.seek_exact(*hashed_address)?.is_some() {
                hashed_storage_cursor.delete_current_duplicates()?;
            }

            for (hashed_slot, value) in storage.storage_slots_sorted() {
                let entry = StorageEntry { key: hashed_slot, value };
                if let Some(db_entry) =
                    hashed_storage_cursor.seek_by_key_subkey(*hashed_address, entry.key)?
                {
                    if db_entry.key == entry.key {
                        hashed_storage_cursor.delete_current()?;
                    }
                }

                if !entry.value.is_zero() {
                    hashed_storage_cursor.upsert(*hashed_address, entry)?;
                }
            }
        }

        Ok(())
    }

    /// Remove the last N blocks of state.
    ///
    /// The latest state will be unwound
    ///
    /// 1. Iterate over the [`BlockBodyIndices`][tables::BlockBodyIndices] table to get all the
    ///    transaction ids.
    /// 2. Iterate over the [`StorageChangeSets`][tables::StorageChangeSets] table and the
    ///    [`AccountChangeSets`][tables::AccountChangeSets] tables in reverse order to reconstruct
    ///    the changesets.
    ///    - In order to have both the old and new values in the changesets, we also access the
    ///      plain state tables.
    /// 3. While iterating over the changeset tables, if we encounter a new account or storage slot,
    ///    we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the plain state
    ///     3. Save the old value to the local state
    /// 4. While iterating over the changeset tables, if we encounter an account/storage slot we
    ///    have seen before we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the local state
    ///     3. Set the local state to the value in the changeset
    fn remove_state(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<()> {
        if range.is_empty() {
            return Ok(())
        }

        // We are not removing block meta as it is used to get block changesets.
        let block_bodies = self.get::<tables::BlockBodyIndices>(range.clone())?;

        // get transaction receipts
        let from_transaction_num =
            block_bodies.first().expect("already checked if there are blocks").1.first_tx_num();
        let to_transaction_num =
            block_bodies.last().expect("already checked if there are blocks").1.last_tx_num();

        let storage_range = BlockNumberAddress::range(range.clone());

        let storage_changeset = self.take::<tables::StorageChangeSets>(storage_range)?;
        let account_changeset = self.take::<tables::AccountChangeSets>(range)?;

        // This is not working for blocks that are not at tip. as plain state is not the last
        // state of end range. We should rename the functions or add support to access
        // History state. Accessing history state can be tricky but we are not gaining
        // anything.
        let mut plain_accounts_cursor = self.tx.cursor_write::<tables::PlainAccountState>()?;
        let mut plain_storage_cursor = self.tx.cursor_dup_write::<tables::PlainStorageState>()?;

        let (state, _) = self.populate_bundle_state(
            account_changeset,
            storage_changeset,
            &mut plain_accounts_cursor,
            &mut plain_storage_cursor,
        )?;

        // iterate over local plain state remove all account and all storages.
        for (address, (old_account, new_account, storage)) in &state {
            // revert account if needed.
            if old_account != new_account {
                let existing_entry = plain_accounts_cursor.seek_exact(*address)?;
                if let Some(account) = old_account {
                    plain_accounts_cursor.upsert(*address, *account)?;
                } else if existing_entry.is_some() {
                    plain_accounts_cursor.delete_current()?;
                }
            }

            // revert storages
            for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                let storage_entry = StorageEntry { key: *storage_key, value: *old_storage_value };
                // delete previous value
                // TODO: This does not use dupsort features
                if plain_storage_cursor
                    .seek_by_key_subkey(*address, *storage_key)?
                    .filter(|s| s.key == *storage_key)
                    .is_some()
                {
                    plain_storage_cursor.delete_current()?
                }

                // insert value if needed
                if !old_storage_value.is_zero() {
                    plain_storage_cursor.upsert(*address, storage_entry)?;
                }
            }
        }

        // iterate over block body and remove receipts
        self.remove::<tables::Receipts>(from_transaction_num..=to_transaction_num)?;

        Ok(())
    }

    /// Take the last N blocks of state, recreating the [`ExecutionOutcome`].
    ///
    /// The latest state will be unwound and returned back with all the blocks
    ///
    /// 1. Iterate over the [`BlockBodyIndices`][tables::BlockBodyIndices] table to get all the
    ///    transaction ids.
    /// 2. Iterate over the [`StorageChangeSets`][tables::StorageChangeSets] table and the
    ///    [`AccountChangeSets`][tables::AccountChangeSets] tables in reverse order to reconstruct
    ///    the changesets.
    ///    - In order to have both the old and new values in the changesets, we also access the
    ///      plain state tables.
    /// 3. While iterating over the changeset tables, if we encounter a new account or storage slot,
    ///    we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the plain state
    ///     3. Save the old value to the local state
    /// 4. While iterating over the changeset tables, if we encounter an account/storage slot we
    ///    have seen before we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the local state
    ///     3. Set the local state to the value in the changeset
    fn take_state(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<ExecutionOutcome> {
        if range.is_empty() {
            return Ok(ExecutionOutcome::default())
        }
        let start_block_number = *range.start();

        // We are not removing block meta as it is used to get block changesets.
        let block_bodies = self.get::<tables::BlockBodyIndices>(range.clone())?;

        // get transaction receipts
        let from_transaction_num =
            block_bodies.first().expect("already checked if there are blocks").1.first_tx_num();
        let to_transaction_num =
            block_bodies.last().expect("already checked if there are blocks").1.last_tx_num();

        let storage_range = BlockNumberAddress::range(range.clone());

        let storage_changeset = self.take::<tables::StorageChangeSets>(storage_range)?;
        let account_changeset = self.take::<tables::AccountChangeSets>(range)?;

        // This is not working for blocks that are not at tip. as plain state is not the last
        // state of end range. We should rename the functions or add support to access
        // History state. Accessing history state can be tricky but we are not gaining
        // anything.
        let mut plain_accounts_cursor = self.tx.cursor_write::<tables::PlainAccountState>()?;
        let mut plain_storage_cursor = self.tx.cursor_dup_write::<tables::PlainStorageState>()?;

        // populate bundle state and reverts from changesets / state cursors, to iterate over,
        // remove, and return later
        let (state, reverts) = self.populate_bundle_state(
            account_changeset,
            storage_changeset,
            &mut plain_accounts_cursor,
            &mut plain_storage_cursor,
        )?;

        // iterate over local plain state remove all account and all storages.
        for (address, (old_account, new_account, storage)) in &state {
            // revert account if needed.
            if old_account != new_account {
                let existing_entry = plain_accounts_cursor.seek_exact(*address)?;
                if let Some(account) = old_account {
                    plain_accounts_cursor.upsert(*address, *account)?;
                } else if existing_entry.is_some() {
                    plain_accounts_cursor.delete_current()?;
                }
            }

            // revert storages
            for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                let storage_entry = StorageEntry { key: *storage_key, value: *old_storage_value };
                // delete previous value
                // TODO: This does not use dupsort features
                if plain_storage_cursor
                    .seek_by_key_subkey(*address, *storage_key)?
                    .filter(|s| s.key == *storage_key)
                    .is_some()
                {
                    plain_storage_cursor.delete_current()?
                }

                // insert value if needed
                if !old_storage_value.is_zero() {
                    plain_storage_cursor.upsert(*address, storage_entry)?;
                }
            }
        }

        // iterate over block body and create ExecutionResult
        let mut receipt_iter =
            self.take::<tables::Receipts>(from_transaction_num..=to_transaction_num)?.into_iter();

        let mut receipts = Vec::with_capacity(block_bodies.len());
        // loop break if we are at the end of the blocks.
        for (_, block_body) in block_bodies {
            let mut block_receipts = Vec::with_capacity(block_body.tx_count as usize);
            for _ in block_body.tx_num_range() {
                if let Some((_, receipt)) = receipt_iter.next() {
                    block_receipts.push(Some(receipt));
                }
            }
            receipts.push(block_receipts);
        }

        Ok(ExecutionOutcome::new_init(
            state,
            reverts,
            Vec::new(),
            receipts.into(),
            start_block_number,
            Vec::new(),
        ))
    }
}

impl<TX: DbTxMut + DbTx, Spec: Send + Sync> TrieWriter for DatabaseProvider<TX, Spec> {
    /// Writes trie updates. Returns the number of entries modified.
    fn write_trie_updates(&self, trie_updates: &TrieUpdates) -> ProviderResult<usize> {
        if trie_updates.is_empty() {
            return Ok(0)
        }

        // Track the number of inserted entries.
        let mut num_entries = 0;

        // Merge updated and removed nodes. Updated nodes must take precedence.
        let mut account_updates = trie_updates
            .removed_nodes_ref()
            .iter()
            .filter_map(|n| {
                (!trie_updates.account_nodes_ref().contains_key(n)).then_some((n, None))
            })
            .collect::<Vec<_>>();
        account_updates.extend(
            trie_updates.account_nodes_ref().iter().map(|(nibbles, node)| (nibbles, Some(node))),
        );
        // Sort trie node updates.
        account_updates.sort_unstable_by(|a, b| a.0.cmp(b.0));

        let tx = self.tx_ref();
        let mut account_trie_cursor = tx.cursor_write::<tables::AccountsTrie>()?;
        for (key, updated_node) in account_updates {
            let nibbles = StoredNibbles(key.clone());
            match updated_node {
                Some(node) => {
                    if !nibbles.0.is_empty() {
                        num_entries += 1;
                        account_trie_cursor.upsert(nibbles, node.clone())?;
                    }
                }
                None => {
                    num_entries += 1;
                    if account_trie_cursor.seek_exact(nibbles)?.is_some() {
                        account_trie_cursor.delete_current()?;
                    }
                }
            }
        }

        num_entries += self.write_storage_trie_updates(trie_updates.storage_tries_ref())?;

        Ok(num_entries)
    }
}

impl<TX: DbTxMut + DbTx, Spec: Send + Sync> StorageTrieWriter for DatabaseProvider<TX, Spec> {
    /// Writes storage trie updates from the given storage trie map. First sorts the storage trie
    /// updates by the hashed address, writing in sorted order.
    fn write_storage_trie_updates(
        &self,
        storage_tries: &HashMap<B256, StorageTrieUpdates>,
    ) -> ProviderResult<usize> {
        let mut num_entries = 0;
        let mut storage_tries = Vec::from_iter(storage_tries);
        storage_tries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let mut cursor = self.tx_ref().cursor_dup_write::<tables::StoragesTrie>()?;
        for (hashed_address, storage_trie_updates) in storage_tries {
            let mut db_storage_trie_cursor =
                DatabaseStorageTrieCursor::new(cursor, *hashed_address);
            num_entries +=
                db_storage_trie_cursor.write_storage_trie_updates(storage_trie_updates)?;
            cursor = db_storage_trie_cursor.cursor;
        }

        Ok(num_entries)
    }

    fn write_individual_storage_trie_updates(
        &self,
        hashed_address: B256,
        updates: &StorageTrieUpdates,
    ) -> ProviderResult<usize> {
        if updates.is_empty() {
            return Ok(0)
        }

        let cursor = self.tx_ref().cursor_dup_write::<tables::StoragesTrie>()?;
        let mut trie_db_cursor = DatabaseStorageTrieCursor::new(cursor, hashed_address);
        Ok(trie_db_cursor.write_storage_trie_updates(updates)?)
    }
}

impl<TX: DbTxMut + DbTx, Spec: Send + Sync> HashingWriter for DatabaseProvider<TX, Spec> {
    fn unwind_account_hashing(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account>>> {
        // Aggregate all block changesets and make a list of accounts that have been changed.
        // Note that collecting and then reversing the order is necessary to ensure that the
        // changes are applied in the correct order.
        let hashed_accounts = self
            .tx
            .cursor_read::<tables::AccountChangeSets>()?
            .walk_range(range)?
            .map(|entry| entry.map(|(_, e)| (keccak256(e.address), e.info)))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .rev()
            .collect::<BTreeMap<_, _>>();

        // Apply values to HashedState, and remove the account if it's None.
        let mut hashed_accounts_cursor = self.tx.cursor_write::<tables::HashedAccounts>()?;
        for (hashed_address, account) in &hashed_accounts {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(*hashed_address, *account)?;
            } else if hashed_accounts_cursor.seek_exact(*hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }

        Ok(hashed_accounts)
    }

    fn insert_account_for_hashing(
        &self,
        accounts: impl IntoIterator<Item = (Address, Option<Account>)>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account>>> {
        let mut hashed_accounts_cursor = self.tx.cursor_write::<tables::HashedAccounts>()?;
        let hashed_accounts =
            accounts.into_iter().map(|(ad, ac)| (keccak256(ad), ac)).collect::<BTreeMap<_, _>>();
        for (hashed_address, account) in &hashed_accounts {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(*hashed_address, *account)?;
            } else if hashed_accounts_cursor.seek_exact(*hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }
        Ok(hashed_accounts)
    }

    fn unwind_storage_hashing(
        &self,
        range: Range<BlockNumberAddress>,
    ) -> ProviderResult<HashMap<B256, BTreeSet<B256>>> {
        // Aggregate all block changesets and make list of accounts that have been changed.
        let mut changesets = self.tx.cursor_read::<tables::StorageChangeSets>()?;
        let mut hashed_storages = changesets
            .walk_range(range)?
            .map(|entry| {
                entry.map(|(BlockNumberAddress((_, address)), storage_entry)| {
                    (keccak256(address), keccak256(storage_entry.key), storage_entry.value)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        hashed_storages.sort_by_key(|(ha, hk, _)| (*ha, *hk));

        // Apply values to HashedState, and remove the account if it's None.
        let mut hashed_storage_keys: HashMap<B256, BTreeSet<B256>> =
            HashMap::with_capacity(hashed_storages.len());
        let mut hashed_storage = self.tx.cursor_dup_write::<tables::HashedStorages>()?;
        for (hashed_address, key, value) in hashed_storages.into_iter().rev() {
            hashed_storage_keys.entry(hashed_address).or_default().insert(key);

            if hashed_storage
                .seek_by_key_subkey(hashed_address, key)?
                .filter(|entry| entry.key == key)
                .is_some()
            {
                hashed_storage.delete_current()?;
            }

            if !value.is_zero() {
                hashed_storage.upsert(hashed_address, StorageEntry { key, value })?;
            }
        }
        Ok(hashed_storage_keys)
    }

    fn insert_storage_for_hashing(
        &self,
        storages: impl IntoIterator<Item = (Address, impl IntoIterator<Item = StorageEntry>)>,
    ) -> ProviderResult<HashMap<B256, BTreeSet<B256>>> {
        // hash values
        let hashed_storages =
            storages.into_iter().fold(BTreeMap::new(), |mut map, (address, storage)| {
                let storage = storage.into_iter().fold(BTreeMap::new(), |mut map, entry| {
                    map.insert(keccak256(entry.key), entry.value);
                    map
                });
                map.insert(keccak256(address), storage);
                map
            });

        let hashed_storage_keys =
            HashMap::from_iter(hashed_storages.iter().map(|(hashed_address, entries)| {
                (*hashed_address, BTreeSet::from_iter(entries.keys().copied()))
            }));

        let mut hashed_storage_cursor = self.tx.cursor_dup_write::<tables::HashedStorages>()?;
        // Hash the address and key and apply them to HashedStorage (if Storage is None
        // just remove it);
        hashed_storages.into_iter().try_for_each(|(hashed_address, storage)| {
            storage.into_iter().try_for_each(|(key, value)| -> ProviderResult<()> {
                if hashed_storage_cursor
                    .seek_by_key_subkey(hashed_address, key)?
                    .filter(|entry| entry.key == key)
                    .is_some()
                {
                    hashed_storage_cursor.delete_current()?;
                }

                if !value.is_zero() {
                    hashed_storage_cursor.upsert(hashed_address, StorageEntry { key, value })?;
                }
                Ok(())
            })
        })?;

        Ok(hashed_storage_keys)
    }

    fn insert_hashes(
        &self,
        range: RangeInclusive<BlockNumber>,
        end_block_hash: B256,
        expected_state_root: B256,
    ) -> ProviderResult<()> {
        // Initialize prefix sets.
        let mut account_prefix_set = PrefixSetMut::default();
        let mut storage_prefix_sets: HashMap<B256, PrefixSetMut> = HashMap::default();
        let mut destroyed_accounts = HashSet::default();

        let mut durations_recorder = metrics::DurationsRecorder::default();

        // storage hashing stage
        {
            let lists = self.changed_storages_with_range(range.clone())?;
            let storages = self.plain_state_storages(lists)?;
            let storage_entries = self.insert_storage_for_hashing(storages)?;
            for (hashed_address, hashed_slots) in storage_entries {
                account_prefix_set.insert(Nibbles::unpack(hashed_address));
                for slot in hashed_slots {
                    storage_prefix_sets
                        .entry(hashed_address)
                        .or_default()
                        .insert(Nibbles::unpack(slot));
                }
            }
        }
        durations_recorder.record_relative(metrics::Action::InsertStorageHashing);

        // account hashing stage
        {
            let lists = self.changed_accounts_with_range(range.clone())?;
            let accounts = self.basic_accounts(lists)?;
            let hashed_addresses = self.insert_account_for_hashing(accounts)?;
            for (hashed_address, account) in hashed_addresses {
                account_prefix_set.insert(Nibbles::unpack(hashed_address));
                if account.is_none() {
                    destroyed_accounts.insert(hashed_address);
                }
            }
        }
        durations_recorder.record_relative(metrics::Action::InsertAccountHashing);

        // merkle tree
        {
            // This is the same as `StateRoot::incremental_root_with_updates`, only the prefix sets
            // are pre-loaded.
            let prefix_sets = TriePrefixSets {
                account_prefix_set: account_prefix_set.freeze(),
                storage_prefix_sets: storage_prefix_sets
                    .into_iter()
                    .map(|(k, v)| (k, v.freeze()))
                    .collect(),
                destroyed_accounts,
            };
            let (state_root, trie_updates) = StateRoot::from_tx(&self.tx)
                .with_prefix_sets(prefix_sets)
                .root_with_updates()
                .map_err(Into::<reth_db::DatabaseError>::into)?;
            if state_root != expected_state_root {
                return Err(ProviderError::StateRootMismatch(Box::new(RootMismatch {
                    root: GotExpected { got: state_root, expected: expected_state_root },
                    block_number: *range.end(),
                    block_hash: end_block_hash,
                })))
            }
            self.write_trie_updates(&trie_updates)?;
        }
        durations_recorder.record_relative(metrics::Action::InsertMerkleTree);

        debug!(target: "providers::db", ?range, actions = ?durations_recorder.actions, "Inserted hashes");

        Ok(())
    }
}

impl<TX: DbTxMut + DbTx, Spec: Send + Sync> HistoryWriter for DatabaseProvider<TX, Spec> {
    fn unwind_account_history_indices(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<usize> {
        let mut last_indices = self
            .tx
            .cursor_read::<tables::AccountChangeSets>()?
            .walk_range(range)?
            .map(|entry| entry.map(|(index, account)| (account.address, index)))
            .collect::<Result<Vec<_>, _>>()?;
        last_indices.sort_by_key(|(a, _)| *a);

        // Unwind the account history index.
        let mut cursor = self.tx.cursor_write::<tables::AccountsHistory>()?;
        for &(address, rem_index) in &last_indices {
            let partial_shard = unwind_history_shards::<_, tables::AccountsHistory, _>(
                &mut cursor,
                ShardedKey::last(address),
                rem_index,
                |sharded_key| sharded_key.key == address,
            )?;

            // Check the last returned partial shard.
            // If it's not empty, the shard needs to be reinserted.
            if !partial_shard.is_empty() {
                cursor.insert(
                    ShardedKey::last(address),
                    BlockNumberList::new_pre_sorted(partial_shard),
                )?;
            }
        }

        let changesets = last_indices.len();
        Ok(changesets)
    }

    fn insert_account_history_index(
        &self,
        account_transitions: impl IntoIterator<Item = (Address, impl IntoIterator<Item = u64>)>,
    ) -> ProviderResult<()> {
        self.append_history_index::<_, tables::AccountsHistory>(
            account_transitions,
            ShardedKey::new,
        )
    }

    fn unwind_storage_history_indices(
        &self,
        range: Range<BlockNumberAddress>,
    ) -> ProviderResult<usize> {
        let mut storage_changesets = self
            .tx
            .cursor_read::<tables::StorageChangeSets>()?
            .walk_range(range)?
            .map(|entry| {
                entry.map(|(BlockNumberAddress((bn, address)), storage)| (address, storage.key, bn))
            })
            .collect::<Result<Vec<_>, _>>()?;
        storage_changesets.sort_by_key(|(address, key, _)| (*address, *key));

        let mut cursor = self.tx.cursor_write::<tables::StoragesHistory>()?;
        for &(address, storage_key, rem_index) in &storage_changesets {
            let partial_shard = unwind_history_shards::<_, tables::StoragesHistory, _>(
                &mut cursor,
                StorageShardedKey::last(address, storage_key),
                rem_index,
                |storage_sharded_key| {
                    storage_sharded_key.address == address &&
                        storage_sharded_key.sharded_key.key == storage_key
                },
            )?;

            // Check the last returned partial shard.
            // If it's not empty, the shard needs to be reinserted.
            if !partial_shard.is_empty() {
                cursor.insert(
                    StorageShardedKey::last(address, storage_key),
                    BlockNumberList::new_pre_sorted(partial_shard),
                )?;
            }
        }

        let changesets = storage_changesets.len();
        Ok(changesets)
    }

    fn insert_storage_history_index(
        &self,
        storage_transitions: impl IntoIterator<Item = ((Address, B256), impl IntoIterator<Item = u64>)>,
    ) -> ProviderResult<()> {
        self.append_history_index::<_, tables::StoragesHistory>(
            storage_transitions,
            |(address, storage_key), highest_block_number| {
                StorageShardedKey::new(address, storage_key, highest_block_number)
            },
        )
    }

    fn update_history_indices(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<()> {
        // account history stage
        {
            let indices = self.changed_accounts_and_blocks_with_range(range.clone())?;
            self.insert_account_history_index(indices)?;
        }

        // storage history stage
        {
            let indices = self.changed_storages_and_blocks_with_range(range)?;
            self.insert_storage_history_index(indices)?;
        }

        Ok(())
    }
}

impl<TX: DbTx, Spec: Send + Sync + EthereumHardforks> BlockExecutionReader
    for DatabaseProvider<TX, Spec>
{
    fn get_block_and_execution_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Chain> {
        // get blocks
        let blocks = self.get_block_range(range.clone())?;

        // get execution res
        let execution_state = self.get_state(range)?.unwrap_or_default();

        Ok(Chain::new(blocks, execution_state, None))
    }
}

impl<TX: DbTx, Spec: Send + Sync> StateReader for DatabaseProvider<TX, Spec> {
    fn get_state(&self, block: BlockNumber) -> ProviderResult<Option<ExecutionOutcome>> {
        self.get_state(block..=block)
    }
}

impl<TX: DbTxMut + DbTx + 'static, Spec: Send + Sync + EthereumHardforks + 'static>
    BlockExecutionWriter for DatabaseProvider<TX, Spec>
{
    fn take_block_and_execution_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Chain> {
        let storage_range = BlockNumberAddress::range(range.clone());

        // Unwind account hashes. Add changed accounts to account prefix set.
        let hashed_addresses = self.unwind_account_hashing(range.clone())?;
        let mut account_prefix_set = PrefixSetMut::with_capacity(hashed_addresses.len());
        let mut destroyed_accounts = HashSet::default();
        for (hashed_address, account) in hashed_addresses {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
            if account.is_none() {
                destroyed_accounts.insert(hashed_address);
            }
        }

        // Unwind account history indices.
        self.unwind_account_history_indices(range.clone())?;

        // Unwind storage hashes. Add changed account and storage keys to corresponding prefix
        // sets.
        let mut storage_prefix_sets = HashMap::<B256, PrefixSet>::default();
        let storage_entries = self.unwind_storage_hashing(storage_range.clone())?;
        for (hashed_address, hashed_slots) in storage_entries {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
            let mut storage_prefix_set = PrefixSetMut::with_capacity(hashed_slots.len());
            for slot in hashed_slots {
                storage_prefix_set.insert(Nibbles::unpack(slot));
            }
            storage_prefix_sets.insert(hashed_address, storage_prefix_set.freeze());
        }

        // Unwind storage history indices.
        self.unwind_storage_history_indices(storage_range)?;

        // Calculate the reverted merkle root.
        // This is the same as `StateRoot::incremental_root_with_updates`, only the prefix sets
        // are pre-loaded.
        let prefix_sets = TriePrefixSets {
            account_prefix_set: account_prefix_set.freeze(),
            storage_prefix_sets,
            destroyed_accounts,
        };
        let (new_state_root, trie_updates) = StateRoot::from_tx(&self.tx)
            .with_prefix_sets(prefix_sets)
            .root_with_updates()
            .map_err(Into::<reth_db::DatabaseError>::into)?;

        let parent_number = range.start().saturating_sub(1);
        let parent_state_root = self
            .header_by_number(parent_number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(parent_number.into()))?
            .state_root;

        // state root should be always correct as we are reverting state.
        // but for sake of double verification we will check it again.
        if new_state_root != parent_state_root {
            let parent_hash = self
                .block_hash(parent_number)?
                .ok_or_else(|| ProviderError::HeaderNotFound(parent_number.into()))?;
            return Err(ProviderError::UnwindStateRootMismatch(Box::new(RootMismatch {
                root: GotExpected { got: new_state_root, expected: parent_state_root },
                block_number: parent_number,
                block_hash: parent_hash,
            })))
        }
        self.write_trie_updates(&trie_updates)?;

        // get blocks
        let blocks = self.take_block_range(range.clone())?;
        let unwind_to = blocks.first().map(|b| b.number.saturating_sub(1));

        // get execution res
        let execution_state = self.take_state(range.clone())?;

        // remove block bodies it is needed for both get block range and get block execution results
        // that is why it is deleted afterwards.
        self.remove::<tables::BlockBodyIndices>(range)?;

        // Update pipeline progress
        if let Some(fork_number) = unwind_to {
            self.update_pipeline_stages(fork_number, true)?;
        }

        Ok(Chain::new(blocks, execution_state, None))
    }

    fn remove_block_and_execution_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let storage_range = BlockNumberAddress::range(range.clone());

        // Unwind account hashes. Add changed accounts to account prefix set.
        let hashed_addresses = self.unwind_account_hashing(range.clone())?;
        let mut account_prefix_set = PrefixSetMut::with_capacity(hashed_addresses.len());
        let mut destroyed_accounts = HashSet::default();
        for (hashed_address, account) in hashed_addresses {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
            if account.is_none() {
                destroyed_accounts.insert(hashed_address);
            }
        }

        // Unwind account history indices.
        self.unwind_account_history_indices(range.clone())?;

        // Unwind storage hashes. Add changed account and storage keys to corresponding prefix
        // sets.
        let mut storage_prefix_sets = HashMap::<B256, PrefixSet>::default();
        let storage_entries = self.unwind_storage_hashing(storage_range.clone())?;
        for (hashed_address, hashed_slots) in storage_entries {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
            let mut storage_prefix_set = PrefixSetMut::with_capacity(hashed_slots.len());
            for slot in hashed_slots {
                storage_prefix_set.insert(Nibbles::unpack(slot));
            }
            storage_prefix_sets.insert(hashed_address, storage_prefix_set.freeze());
        }

        // Unwind storage history indices.
        self.unwind_storage_history_indices(storage_range)?;

        // Calculate the reverted merkle root.
        // This is the same as `StateRoot::incremental_root_with_updates`, only the prefix sets
        // are pre-loaded.
        let prefix_sets = TriePrefixSets {
            account_prefix_set: account_prefix_set.freeze(),
            storage_prefix_sets,
            destroyed_accounts,
        };
        let (new_state_root, trie_updates) = StateRoot::from_tx(&self.tx)
            .with_prefix_sets(prefix_sets)
            .root_with_updates()
            .map_err(Into::<reth_db::DatabaseError>::into)?;

        let parent_number = range.start().saturating_sub(1);
        let parent_state_root = self
            .header_by_number(parent_number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(parent_number.into()))?
            .state_root;

        // state root should be always correct as we are reverting state.
        // but for sake of double verification we will check it again.
        if new_state_root != parent_state_root {
            let parent_hash = self
                .block_hash(parent_number)?
                .ok_or_else(|| ProviderError::HeaderNotFound(parent_number.into()))?;
            return Err(ProviderError::UnwindStateRootMismatch(Box::new(RootMismatch {
                root: GotExpected { got: new_state_root, expected: parent_state_root },
                block_number: parent_number,
                block_hash: parent_hash,
            })))
        }
        self.write_trie_updates(&trie_updates)?;

        // get blocks
        let blocks = self.take_block_range(range.clone())?;
        let unwind_to = blocks.first().map(|b| b.number.saturating_sub(1));

        // remove execution res
        self.remove_state(range.clone())?;

        // remove block bodies it is needed for both get block range and get block execution results
        // that is why it is deleted afterwards.
        self.remove::<tables::BlockBodyIndices>(range)?;

        // Update pipeline progress
        if let Some(block_number) = unwind_to {
            self.update_pipeline_stages(block_number, true)?;
        }

        Ok(())
    }
}

impl<TX: DbTxMut + DbTx + 'static, Spec: Send + Sync + EthereumHardforks + 'static> BlockWriter
    for DatabaseProvider<TX, Spec>
{
    /// Inserts the block into the database, always modifying the following tables:
    /// * [`CanonicalHeaders`](tables::CanonicalHeaders)
    /// * [`Headers`](tables::Headers)
    /// * [`HeaderNumbers`](tables::HeaderNumbers)
    /// * [`HeaderTerminalDifficulties`](tables::HeaderTerminalDifficulties)
    /// * [`BlockBodyIndices`](tables::BlockBodyIndices)
    ///
    /// If there are transactions in the block, the following tables will be modified:
    /// * [`Transactions`](tables::Transactions)
    /// * [`TransactionBlocks`](tables::TransactionBlocks)
    ///
    /// If ommers are not empty, this will modify [`BlockOmmers`](tables::BlockOmmers).
    /// If withdrawals are not empty, this will modify
    /// [`BlockWithdrawals`](tables::BlockWithdrawals).
    /// If requests are not empty, this will modify [`BlockRequests`](tables::BlockRequests).
    ///
    /// If the provider has __not__ configured full sender pruning, this will modify
    /// [`TransactionSenders`](tables::TransactionSenders).
    ///
    /// If the provider has __not__ configured full transaction lookup pruning, this will modify
    /// [`TransactionHashNumbers`](tables::TransactionHashNumbers).
    fn insert_block(
        &self,
        block: SealedBlockWithSenders,
    ) -> ProviderResult<StoredBlockBodyIndices> {
        let block_number = block.number;

        let mut durations_recorder = metrics::DurationsRecorder::default();

        self.tx.put::<tables::CanonicalHeaders>(block_number, block.hash())?;
        durations_recorder.record_relative(metrics::Action::InsertCanonicalHeaders);

        // Put header with canonical hashes.
        self.tx.put::<tables::Headers>(block_number, block.header.as_ref().clone())?;
        durations_recorder.record_relative(metrics::Action::InsertHeaders);

        self.tx.put::<tables::HeaderNumbers>(block.hash(), block_number)?;
        durations_recorder.record_relative(metrics::Action::InsertHeaderNumbers);

        // total difficulty
        let ttd = if block_number == 0 {
            block.difficulty
        } else {
            let parent_block_number = block_number - 1;
            let parent_ttd = self.header_td_by_number(parent_block_number)?.unwrap_or_default();
            durations_recorder.record_relative(metrics::Action::GetParentTD);
            parent_ttd + block.difficulty
        };

        self.tx.put::<tables::HeaderTerminalDifficulties>(block_number, ttd.into())?;
        durations_recorder.record_relative(metrics::Action::InsertHeaderTerminalDifficulties);

        // insert body ommers data
        if !block.body.ommers.is_empty() {
            self.tx.put::<tables::BlockOmmers>(
                block_number,
                StoredBlockOmmers { ommers: block.block.body.ommers },
            )?;
            durations_recorder.record_relative(metrics::Action::InsertBlockOmmers);
        }

        let mut next_tx_num = self
            .tx
            .cursor_read::<tables::TransactionBlocks>()?
            .last()?
            .map(|(n, _)| n + 1)
            .unwrap_or_default();
        durations_recorder.record_relative(metrics::Action::GetNextTxNum);
        let first_tx_num = next_tx_num;

        let tx_count = block.block.body.transactions.len() as u64;

        // Ensures we have all the senders for the block's transactions.
        let mut tx_senders_elapsed = Duration::default();
        let mut transactions_elapsed = Duration::default();
        let mut tx_hash_numbers_elapsed = Duration::default();

        for (transaction, sender) in
            block.block.body.transactions.into_iter().zip(block.senders.iter())
        {
            let hash = transaction.hash();

            if self
                .prune_modes
                .sender_recovery
                .as_ref()
                .filter(|prune_mode| prune_mode.is_full())
                .is_none()
            {
                let start = Instant::now();
                self.tx.put::<tables::TransactionSenders>(next_tx_num, *sender)?;
                tx_senders_elapsed += start.elapsed();
            }

            let start = Instant::now();
            self.tx.put::<tables::Transactions>(next_tx_num, transaction.into())?;
            let elapsed = start.elapsed();
            if elapsed > Duration::from_secs(1) {
                warn!(
                    target: "providers::db",
                    ?block_number,
                    tx_num = %next_tx_num,
                    hash = %hash,
                    ?elapsed,
                    "Transaction insertion took too long"
                );
            }
            transactions_elapsed += elapsed;

            if self
                .prune_modes
                .transaction_lookup
                .filter(|prune_mode| prune_mode.is_full())
                .is_none()
            {
                let start = Instant::now();
                self.tx.put::<tables::TransactionHashNumbers>(hash, next_tx_num)?;
                tx_hash_numbers_elapsed += start.elapsed();
            }
            next_tx_num += 1;
        }
        durations_recorder
            .record_duration(metrics::Action::InsertTransactionSenders, tx_senders_elapsed);
        durations_recorder
            .record_duration(metrics::Action::InsertTransactions, transactions_elapsed);
        durations_recorder.record_duration(
            metrics::Action::InsertTransactionHashNumbers,
            tx_hash_numbers_elapsed,
        );

        if let Some(withdrawals) = block.block.body.withdrawals {
            if !withdrawals.is_empty() {
                self.tx.put::<tables::BlockWithdrawals>(
                    block_number,
                    StoredBlockWithdrawals { withdrawals },
                )?;
                durations_recorder.record_relative(metrics::Action::InsertBlockWithdrawals);
            }
        }

        if let Some(requests) = block.block.body.requests {
            if !requests.0.is_empty() {
                self.tx.put::<tables::BlockRequests>(block_number, requests)?;
                durations_recorder.record_relative(metrics::Action::InsertBlockRequests);
            }
        }

        let block_indices = StoredBlockBodyIndices { first_tx_num, tx_count };
        self.tx.put::<tables::BlockBodyIndices>(block_number, block_indices.clone())?;
        durations_recorder.record_relative(metrics::Action::InsertBlockBodyIndices);

        if !block_indices.is_empty() {
            self.tx.put::<tables::TransactionBlocks>(block_indices.last_tx_num(), block_number)?;
            durations_recorder.record_relative(metrics::Action::InsertTransactionBlocks);
        }

        debug!(
            target: "providers::db",
            ?block_number,
            actions = ?durations_recorder.actions,
            "Inserted block"
        );

        Ok(block_indices)
    }

    /// TODO(joshie): this fn should be moved to `UnifiedStorageWriter` eventually
    fn append_blocks_with_state(
        &self,
        blocks: Vec<SealedBlockWithSenders>,
        execution_outcome: ExecutionOutcome,
        hashed_state: HashedPostStateSorted,
        trie_updates: TrieUpdates,
    ) -> ProviderResult<()> {
        if blocks.is_empty() {
            debug!(target: "providers::db", "Attempted to append empty block range");
            return Ok(())
        }

        let first_number = blocks.first().unwrap().number;

        let last = blocks.last().unwrap();
        let last_block_number = last.number;

        let mut durations_recorder = metrics::DurationsRecorder::default();

        // Insert the blocks
        for block in blocks {
            self.insert_block(block)?;
            durations_recorder.record_relative(metrics::Action::InsertBlock);
        }

        // Write state and changesets to the database.
        // Must be written after blocks because of the receipt lookup.
        // TODO: should _these_ be moved to storagewriter? seems like storagewriter should be
        // _above_ db provider
        let mut storage_writer = UnifiedStorageWriter::from_database(self);
        storage_writer.write_to_storage(execution_outcome, OriginalValuesKnown::No)?;
        durations_recorder.record_relative(metrics::Action::InsertState);

        // insert hashes and intermediate merkle nodes
        self.write_hashed_state(&hashed_state)?;
        self.write_trie_updates(&trie_updates)?;
        durations_recorder.record_relative(metrics::Action::InsertHashes);

        self.update_history_indices(first_number..=last_block_number)?;
        durations_recorder.record_relative(metrics::Action::InsertHistoryIndices);

        // Update pipeline progress
        self.update_pipeline_stages(last_block_number, false)?;
        durations_recorder.record_relative(metrics::Action::UpdatePipelineStages);

        debug!(target: "providers::db", range = ?first_number..=last_block_number, actions = ?durations_recorder.actions, "Appended blocks");

        Ok(())
    }
}

impl<TX: DbTx, Spec: Send + Sync> PruneCheckpointReader for DatabaseProvider<TX, Spec> {
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        Ok(self.tx.get::<tables::PruneCheckpoints>(segment)?)
    }

    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        Ok(self
            .tx
            .cursor_read::<tables::PruneCheckpoints>()?
            .walk(None)?
            .collect::<Result<_, _>>()?)
    }
}

impl<TX: DbTxMut, Spec: Send + Sync> PruneCheckpointWriter for DatabaseProvider<TX, Spec> {
    fn save_prune_checkpoint(
        &self,
        segment: PruneSegment,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()> {
        Ok(self.tx.put::<tables::PruneCheckpoints>(segment, checkpoint)?)
    }
}

impl<TX: DbTx, Spec: Send + Sync> StatsReader for DatabaseProvider<TX, Spec> {
    fn count_entries<T: Table>(&self) -> ProviderResult<usize> {
        let db_entries = self.tx.entries::<T>()?;
        let static_file_entries = match self.static_file_provider.count_entries::<T>() {
            Ok(entries) => entries,
            Err(ProviderError::UnsupportedProvider) => 0,
            Err(err) => return Err(err),
        };

        Ok(db_entries + static_file_entries)
    }
}

impl<TX: DbTx, Spec: Send + Sync> FinalizedBlockReader for DatabaseProvider<TX, Spec> {
    fn last_finalized_block_number(&self) -> ProviderResult<Option<BlockNumber>> {
        let mut finalized_blocks = self
            .tx
            .cursor_read::<tables::ChainState>()?
            .walk(Some(tables::ChainStateKey::LastFinalizedBlock))?
            .take(1)
            .collect::<Result<BTreeMap<tables::ChainStateKey, BlockNumber>, _>>()?;

        let last_finalized_block_number = finalized_blocks.pop_first().map(|pair| pair.1);
        Ok(last_finalized_block_number)
    }
}

impl<TX: DbTxMut, Spec: Send + Sync> FinalizedBlockWriter for DatabaseProvider<TX, Spec> {
    fn save_finalized_block_number(&self, block_number: BlockNumber) -> ProviderResult<()> {
        Ok(self
            .tx
            .put::<tables::ChainState>(tables::ChainStateKey::LastFinalizedBlock, block_number)?)
    }
}

impl<TX: DbTx + 'static, Spec: Send + Sync + 'static> DBProvider for DatabaseProvider<TX, Spec> {
    type Tx = TX;

    fn tx_ref(&self) -> &Self::Tx {
        &self.tx
    }

    fn tx_mut(&mut self) -> &mut Self::Tx {
        &mut self.tx
    }

    fn into_tx(self) -> Self::Tx {
        self.tx
    }

    fn prune_modes_ref(&self) -> &PruneModes {
        self.prune_modes_ref()
    }
}

/// Helper method to recover senders for any blocks in the db which do not have senders. This
/// compares the length of the input senders [`Vec`], with the length of given transactions [`Vec`],
/// and will add to the input senders vec if there are more transactions.
///
/// NOTE: This will modify the input senders list, which is why a mutable reference is required.
fn recover_block_senders(
    senders: &mut Vec<(u64, Address)>,
    transactions: &[(u64, TransactionSigned)],
    first_transaction: u64,
    last_transaction: u64,
) -> ProviderResult<()> {
    // Recover senders manually if not found in db
    // NOTE: Transactions are always guaranteed to be in the database whereas
    // senders might be pruned.
    if senders.len() != transactions.len() {
        if senders.len() > transactions.len() {
            error!(target: "providers::db", senders=%senders.len(), transactions=%transactions.len(),
                first_tx=%first_transaction, last_tx=%last_transaction,
                "unexpected senders and transactions mismatch");
        }
        let missing = transactions.len().saturating_sub(senders.len());
        senders.reserve(missing);
        // Find all missing senders, their corresponding tx numbers and indexes to the original
        // `senders` vector at which the recovered senders will be inserted.
        let mut missing_senders = Vec::with_capacity(missing);
        {
            let mut senders = senders.iter().peekable();

            // `transactions` contain all entries. `senders` contain _some_ of the senders for
            // these transactions. Both are sorted and indexed by `TxNumber`.
            //
            // The general idea is to iterate on both `transactions` and `senders`, and advance
            // the `senders` iteration only if it matches the current `transactions` entry's
            // `TxNumber`. Otherwise, add the transaction to the list of missing senders.
            for (i, (tx_number, transaction)) in transactions.iter().enumerate() {
                if let Some((sender_tx_number, _)) = senders.peek() {
                    if sender_tx_number == tx_number {
                        // If current sender's `TxNumber` matches current transaction's
                        // `TxNumber`, advance the senders iterator.
                        senders.next();
                    } else {
                        // If current sender's `TxNumber` doesn't match current transaction's
                        // `TxNumber`, add it to missing senders.
                        missing_senders.push((i, tx_number, transaction));
                    }
                } else {
                    // If there's no more senders left, but we're still iterating over
                    // transactions, add them to missing senders
                    missing_senders.push((i, tx_number, transaction));
                }
            }
        }

        // Recover senders
        let recovered_senders = TransactionSigned::recover_signers(
            missing_senders.iter().map(|(_, _, tx)| *tx).collect::<Vec<_>>(),
            missing_senders.len(),
        )
        .ok_or(ProviderError::SenderRecoveryError)?;

        // Insert recovered senders along with tx numbers at the corresponding indexes to the
        // original `senders` vector
        for ((i, tx_number, _), sender) in missing_senders.into_iter().zip(recovered_senders) {
            // Insert will put recovered senders at necessary positions and shift the rest
            senders.insert(i, (*tx_number, sender));
        }

        // Debug assertions which are triggered during the test to ensure that all senders are
        // present and sorted
        debug_assert_eq!(senders.len(), transactions.len(), "missing one or more senders");
        debug_assert!(senders.iter().tuple_windows().all(|(a, b)| a.0 < b.0), "senders not sorted");
    }

    Ok(())
}

fn range_size_hint(range: &impl RangeBounds<TxNumber>) -> Option<usize> {
    let start = match range.start_bound().cloned() {
        Bound::Included(start) => start,
        Bound::Excluded(start) => start.checked_add(1)?,
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound().cloned() {
        Bound::Included(end) => end.saturating_add(1),
        Bound::Excluded(end) => end,
        Bound::Unbounded => return None,
    };
    end.checked_sub(start).map(|x| x as _)
}
