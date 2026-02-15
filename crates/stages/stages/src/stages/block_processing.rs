use super::bodies::ensure_consistency;
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, TxHash, TxNumber};
use futures_util::TryStreamExt;
use num_traits::Zero;
use reth_config::config::StageConfig;
#[cfg(all(unix, feature = "rocksdb"))]
use reth_db_api::Tables;
use reth_db_api::{
    table::{Decode, Decompress, Value},
    tables,
    transaction::DbTxMut,
};
use reth_etl::Collector;
use reth_network_p2p::bodies::{downloader::BodyDownloader, response::BlockResponse};
use reth_primitives_traits::{BlockBody, NodePrimitives, SignedTransaction};
use reth_provider::{
    BlockReader, BlockWriter, DBProvider, EitherWriter, ProviderError, PruneCheckpointReader,
    PruneCheckpointWriter, RocksDBProviderFactory, StageCheckpointWriter,
    StaticFileProviderFactory, StatsReader, StorageSettingsCache, TransactionsProvider,
};
use reth_prune_types::{PruneMode, PruneModes, PruneSegment};
use reth_stages_api::{
    EntitiesCheckpoint, ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId,
    UnwindInput, UnwindOutput,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_errors::provider::ProviderResult;
use std::{
    fmt::Debug,
    task::{ready, Context, Poll},
};
use thiserror::Error;

/// Maximum number of transactions per rayon worker chunk for sender recovery.
const WORKER_CHUNK_SIZE: usize = 100;

/// Unified block processing stage that combines bodies download, sender recovery,
/// and transaction lookup into a single pipeline stage.
///
/// Key optimization: transactions are extracted from the in-memory download buffer
/// and dispatched directly to rayon workers for sender recovery and tx hash computation.
/// This eliminates the need for a static file commit between body writing and recovery,
/// since we never read transactions back from disk.
///
/// Sender recovery and transaction lookup run in parallel via `rayon::scope`.
#[derive(Debug)]
pub struct BlockProcessingStage<D: BodyDownloader> {
    /// The body downloader.
    downloader: D,
    /// Block response buffer.
    buffer: Option<Vec<BlockResponse<D::Block>>>,
    /// Configuration for the ETL collector used by tx lookup.
    etl_config: reth_config::config::EtlConfig,
    /// The maximum number of lookup entries to hold in memory before flushing.
    tx_lookup_chunk_size: u64,
    /// Commit threshold for sender recovery (controls how many tx before pipeline yields).
    sender_recovery_commit_threshold: u64,
    /// Prune mode for transaction lookup (used in future prune integration).
    #[allow(dead_code)]
    tx_lookup_prune_mode: Option<PruneMode>,
    /// Prune mode for sender recovery (used in future prune integration).
    #[allow(dead_code)]
    sender_recovery_prune_mode: Option<PruneMode>,
}

impl<D: BodyDownloader> BlockProcessingStage<D> {
    /// Create a new [`BlockProcessingStage`].
    pub fn new(downloader: D, stages_config: &StageConfig, prune_modes: &PruneModes) -> Self {
        Self {
            downloader,
            buffer: None,
            etl_config: stages_config.etl.clone(),
            tx_lookup_chunk_size: stages_config.transaction_lookup.chunk_size,
            sender_recovery_commit_threshold: stages_config.sender_recovery.commit_threshold,
            tx_lookup_prune_mode: prune_modes.transaction_lookup,
            sender_recovery_prune_mode: prune_modes.sender_recovery,
        }
    }
}

impl<Provider, D> Stage<Provider> for BlockProcessingStage<D>
where
    Provider: DBProvider<Tx: DbTxMut>
        + StaticFileProviderFactory<Primitives: NodePrimitives<SignedTx: Value + SignedTransaction>>
        + StatsReader
        + BlockReader
        + BlockWriter<Block = D::Block>
        + TransactionsProvider
        + PruneCheckpointReader
        + PruneCheckpointWriter
        + StorageSettingsCache
        + RocksDBProviderFactory
        + StageCheckpointWriter,
    D: BodyDownloader,
{
    fn id(&self) -> StageId {
        StageId::Bodies
    }

    fn poll_execute_ready(
        &mut self,
        cx: &mut Context<'_>,
        input: ExecInput,
    ) -> Poll<Result<(), StageError>> {
        if input.target_reached() || self.buffer.is_some() {
            return Poll::Ready(Ok(()));
        }

        self.downloader.set_download_range(input.next_block_range())?;
        let maybe_next_result = ready!(self.downloader.try_poll_next_unpin(cx));

        let response = match maybe_next_result {
            Some(Ok(downloaded)) => {
                self.buffer = Some(downloaded);
                Ok(())
            }
            Some(Err(err)) => Err(err.into()),
            None => Err(StageError::ChannelClosed),
        };
        Poll::Ready(response)
    }

    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()));
        }

        let (from_block, to_block) = input.next_block_range().into_inner();

        // Phase 1: Write block bodies (sequential, ordering required)
        ensure_consistency(provider, None)?;

        let buffer = self.buffer.take().ok_or(StageError::MissingDownloadBuffer)?;
        let highest_block = buffer.last().map(|r| r.block_number()).unwrap_or(from_block);

        // Collect (tx_number, transaction) pairs from the in-memory buffer BEFORE writing.
        // This is the key optimization: we use the transactions we already have in memory
        // instead of reading them back from static files after a commit.
        //
        // Derive first_tx_num from the previous block's body indices rather than
        // scanning the whole TransactionBlocks table. This is correct even when
        // resuming from a mid-range checkpoint.
        let first_tx_num = if from_block == 0 {
            0
        } else {
            provider
                .block_body_indices(from_block - 1)?
                .map(|indices| indices.next_tx_num())
                .unwrap_or_default()
        };

        let mut in_memory_txs: Vec<(
            TxNumber,
            <<D::Block as reth_primitives_traits::Block>::Body as BlockBody>::Transaction,
        )> = Vec::new();
        let mut tx_num = first_tx_num;
        for response in &buffer {
            if let Some(body) = response.body() {
                for tx in body.transactions() {
                    in_memory_txs.push((tx_num, tx.clone()));
                    tx_num += 1;
                }
            }
        }

        // Write bodies to static files and database
        provider.append_block_bodies(
            buffer.iter().map(|response| (response.block_number(), response.body())).collect(),
        )?;

        // NO static file commit needed — we have transactions in memory

        let done = highest_block == to_block;

        // Phase 2: Parallel sender recovery + tx lookup from in-memory transactions
        let sr_result = self.execute_sender_recovery_and_tx_lookup(
            provider,
            from_block,
            highest_block,
            in_memory_txs,
        )?;

        // Save sub-stage checkpoints so downstream stages see consistent state
        provider.save_stage_checkpoint(StageId::SenderRecovery, sr_result.sr_checkpoint)?;
        provider.save_stage_checkpoint(StageId::TransactionLookup, sr_result.tl_checkpoint)?;

        let bodies_checkpoint = StageCheckpoint::new(highest_block)
            .with_entities_stage_checkpoint(bodies_stage_checkpoint(provider)?);

        Ok(ExecOutput { checkpoint: bodies_checkpoint, done })
    }

    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        self.buffer.take();
        let unwind_to = input.unwind_to;

        // Unwind tx lookup
        self.unwind_tx_lookup(provider, input)?;
        provider
            .save_stage_checkpoint(StageId::TransactionLookup, StageCheckpoint::new(unwind_to))?;

        // Unwind sender recovery
        self.unwind_sender_recovery(provider, input)?;
        provider.save_stage_checkpoint(StageId::SenderRecovery, StageCheckpoint::new(unwind_to))?;

        // Unwind bodies
        ensure_consistency(provider, Some(unwind_to))?;
        provider.remove_bodies_above(unwind_to)?;

        Ok(UnwindOutput {
            checkpoint: StageCheckpoint::new(unwind_to)
                .with_entities_stage_checkpoint(bodies_stage_checkpoint(provider)?),
        })
    }
}

/// Result of parallel sender recovery + tx lookup.
struct SubStageResult {
    sr_checkpoint: StageCheckpoint,
    tl_checkpoint: StageCheckpoint,
}

impl<D: BodyDownloader> BlockProcessingStage<D> {
    /// Runs sender recovery and tx hash computation in parallel, then writes results.
    ///
    /// Sender recovery: dispatches transaction chunks to rayon for ECDSA recovery.
    /// Tx lookup: computes transaction hashes (already cached on `SignedTransaction`).
    ///
    /// Both run concurrently via `rayon::join`, operating on the in-memory transaction
    /// data extracted from the download buffer — no static file read-back required.
    fn execute_sender_recovery_and_tx_lookup<Provider, T>(
        &self,
        provider: &Provider,
        from_block: u64,
        to_block: u64,
        in_memory_txs: Vec<(TxNumber, T)>,
    ) -> Result<SubStageResult, StageError>
    where
        T: SignedTransaction,
        Provider: DBProvider<Tx: DbTxMut>
            + StaticFileProviderFactory<
                Primitives: NodePrimitives<SignedTx: Value + SignedTransaction>,
            > + StatsReader
            + BlockReader
            + PruneCheckpointReader
            + PruneCheckpointWriter
            + StorageSettingsCache
            + RocksDBProviderFactory
            + StageCheckpointWriter,
    {
        if in_memory_txs.is_empty() {
            // Advance the static file header for senders to cover empty blocks
            EitherWriter::new_senders(
                provider,
                provider
                    .static_file_provider()
                    .get_highest_static_file_block(StaticFileSegment::TransactionSenders)
                    .unwrap_or_default(),
            )?
            .ensure_at_block(to_block)?;

            let sr_checkpoint = StageCheckpoint::new(to_block)
                .with_entities_stage_checkpoint(sender_stage_checkpoint(provider)?);
            let tl_checkpoint = StageCheckpoint::new(to_block)
                .with_entities_stage_checkpoint(tx_lookup_stage_checkpoint(provider)?);

            return Ok(SubStageResult { sr_checkpoint, tl_checkpoint });
        }

        // Run sender recovery and tx hash computation in parallel via rayon::join.
        // Both operate on shared reference to in_memory_txs (no cloning needed).
        let (sr_result, tx_hashes) = rayon::join(
            || -> Result<Vec<(TxNumber, Address)>, SenderRecoveryError> {
                // Sender recovery: ECDSA recover from each transaction
                use rayon::prelude::*;
                in_memory_txs
                    .par_chunks(WORKER_CHUNK_SIZE)
                    .try_fold(
                        Vec::new,
                        |mut acc, chunk| -> Result<Vec<(TxNumber, Address)>, SenderRecoveryError> {
                            let mut rlp_buf = Vec::with_capacity(128);
                            for (tx_id, tx) in chunk {
                                let sender = tx
                                    .recover_unchecked_with_buf(&mut rlp_buf)
                                    .map_err(|_| SenderRecoveryError { tx: *tx_id })?;
                                acc.push((*tx_id, sender));
                                rlp_buf.clear();
                            }
                            Ok(acc)
                        },
                    )
                    .try_reduce(Vec::new, |mut a, b| {
                        a.extend(b);
                        Ok(a)
                    })
            },
            || -> Vec<(TxHash, TxNumber)> {
                // Tx hash extraction: tx_hash() is already cached on SignedTransaction
                in_memory_txs.iter().map(|(tx_id, tx)| (*tx.tx_hash(), *tx_id)).collect()
            },
        );

        // Handle sender recovery result
        let recovered_senders = sr_result.map_err(|err| StageError::Fatal(Box::new(err).into()))?;

        // Validate we recovered all senders
        let expected = in_memory_txs.len() as u64;
        if recovered_senders.len() as u64 != expected {
            return Err(StageError::Fatal(
                format!(
                    "sender recovery mismatch: got {} expected {}",
                    recovered_senders.len(),
                    expected
                )
                .into(),
            ));
        }

        // Write recovered senders
        let mut sr_writer = EitherWriter::new_senders(provider, from_block)?;
        let block_body_indices = provider.block_body_indices_range(from_block..=to_block)?;
        let mut blocks_with_indices = (from_block..=to_block).zip(block_body_indices).peekable();

        for (tx_id, sender) in &recovered_senders {
            // Advance block cursor to find the block this tx belongs to
            while let Some((block_num, indices)) = blocks_with_indices.peek() {
                if indices.contains_tx(*tx_id) {
                    sr_writer.ensure_at_block(*block_num)?;
                    break;
                }
                blocks_with_indices.next();
            }
            sr_writer.append_sender(*tx_id, sender)?;
        }
        sr_writer.ensure_at_block(to_block)?;

        // Write transaction hash → number mappings
        let mut hash_collector: Collector<TxHash, TxNumber> =
            Collector::new(self.etl_config.file_size, self.etl_config.dir.clone());

        for (hash, tx_num) in tx_hashes {
            hash_collector.insert(hash, tx_num)?;
        }

        let _total_hashes = hash_collector.len();
        let append_only = provider.count_entries::<tables::TransactionHashNumbers>()?.is_zero();

        provider.with_rocksdb_batch_auto_commit(|rocksdb_batch| {
            let mut writer = EitherWriter::new_transaction_hash_numbers(provider, rocksdb_batch)?;

            let iter = hash_collector.iter().map_err(|e| ProviderError::other(Box::new(e)))?;

            for hash_to_number in iter {
                let (hash_bytes, number_bytes) =
                    hash_to_number.map_err(|e| ProviderError::other(Box::new(e)))?;
                let hash = TxHash::decode(&hash_bytes)?;
                let tx_num = TxNumber::decompress(&number_bytes)?;
                writer.put_transaction_hash_number(hash, tx_num, append_only)?;
            }

            Ok(((), writer.into_raw_rocksdb_batch()))
        })?;

        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().storage_v2 {
            provider.commit_pending_rocksdb_batches()?;
            provider.rocksdb_provider().flush(&[Tables::TransactionHashNumbers.name()])?;
        }

        let sr_checkpoint = StageCheckpoint::new(to_block)
            .with_entities_stage_checkpoint(sender_stage_checkpoint(provider)?);
        let tl_checkpoint = StageCheckpoint::new(to_block)
            .with_entities_stage_checkpoint(tx_lookup_stage_checkpoint(provider)?);

        Ok(SubStageResult { sr_checkpoint, tl_checkpoint })
    }

    fn unwind_tx_lookup<Provider>(
        &self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<(), StageError>
    where
        Provider: DBProvider<Tx: DbTxMut>
            + BlockReader
            + TransactionsProvider
            + StaticFileProviderFactory<
                Primitives: NodePrimitives<SignedTx: Value + SignedTransaction>,
            > + StorageSettingsCache
            + RocksDBProviderFactory,
    {
        let (range, unwind_to, _) =
            input.unwind_block_range_with_threshold(self.tx_lookup_chunk_size);

        provider.with_rocksdb_batch(|rocksdb_batch| {
            let mut writer = EitherWriter::new_transaction_hash_numbers(provider, rocksdb_batch)?;

            let rev_walker = provider
                .block_body_indices_range(range.clone())?
                .into_iter()
                .rev()
                .zip(range.rev());

            for (body, number) in rev_walker {
                if number <= unwind_to {
                    break;
                }

                for tx in provider.transactions_by_tx_range(body.tx_num_range())? {
                    writer.delete_transaction_hash_number(tx.trie_hash())?;
                }
            }

            Ok(((), writer.into_raw_rocksdb_batch()))
        })?;

        Ok(())
    }

    fn unwind_sender_recovery<Provider>(
        &self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<(), StageError>
    where
        Provider: DBProvider<Tx: DbTxMut>
            + BlockReader
            + StaticFileProviderFactory<
                Primitives: NodePrimitives<SignedTx: Value + SignedTransaction>,
            > + StorageSettingsCache,
    {
        let (_, unwind_to, _) =
            input.unwind_block_range_with_threshold(self.sender_recovery_commit_threshold);

        let unwind_tx_from = provider
            .block_body_indices(unwind_to)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(unwind_to))?
            .next_tx_num();

        EitherWriter::new_senders(provider, unwind_to)?.prune_senders(unwind_tx_from, unwind_to)?;

        Ok(())
    }
}

fn bodies_stage_checkpoint<Provider>(provider: &Provider) -> ProviderResult<EntitiesCheckpoint>
where
    Provider: StatsReader + StaticFileProviderFactory,
{
    Ok(EntitiesCheckpoint {
        processed: provider.count_entries::<tables::BlockBodyIndices>()? as u64,
        total: provider.static_file_provider().count_entries::<tables::Headers>()? as u64,
    })
}

fn sender_stage_checkpoint<Provider>(provider: &Provider) -> Result<EntitiesCheckpoint, StageError>
where
    Provider: StatsReader + StaticFileProviderFactory + PruneCheckpointReader,
{
    let pruned_entries = provider
        .get_prune_checkpoint(PruneSegment::SenderRecovery)?
        .and_then(|checkpoint| checkpoint.tx_number)
        .unwrap_or_default();
    Ok(EntitiesCheckpoint {
        processed: provider.count_entries::<tables::TransactionSenders>()? as u64 + pruned_entries,
        total: provider.static_file_provider().count_entries::<tables::Transactions>()? as u64,
    })
}

fn tx_lookup_stage_checkpoint<Provider>(
    provider: &Provider,
) -> Result<EntitiesCheckpoint, StageError>
where
    Provider: StatsReader + StaticFileProviderFactory + PruneCheckpointReader,
{
    let pruned_entries = provider
        .get_prune_checkpoint(PruneSegment::TransactionLookup)?
        .and_then(|checkpoint| checkpoint.tx_number)
        .unwrap_or_default();
    Ok(EntitiesCheckpoint {
        processed: provider.count_entries::<tables::TransactionHashNumbers>()? as u64 +
            pruned_entries,
        total: provider.static_file_provider().count_entries::<tables::Transactions>()? as u64,
    })
}

#[derive(Error, Debug)]
#[error("failed to recover sender for tx {tx}")]
struct SenderRecoveryError {
    tx: TxNumber,
}
