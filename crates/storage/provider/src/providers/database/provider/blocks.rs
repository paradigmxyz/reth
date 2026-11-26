//! Reads for headers, blocks, transactions and receipts.

use super::DatabaseProvider;
use crate::{
    providers::{database::chain::ChainStorage, NodeTypesForProvider},
    to_range,
    traits::{BlockSource, ReceiptProvider},
    BlockHashReader, BlockNumReader, BlockReader, DBProvider, EitherReader,
    EitherWriterDestination, HeaderProvider, ProviderError, PruneCheckpointReader,
    RocksDBProviderFactory, StageCheckpointReader, TransactionVariant, TransactionsProvider,
    TransactionsProviderExt,
};
use alloy_consensus::{
    transaction::{SignerRecoverable, TransactionMeta},
    BlockHeader,
};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{map::HashMap, Address, BlockHash, BlockNumber, TxHash, TxNumber, B256};
use reth_chainspec::ChainInfo;
use reth_db_api::{cursor::DbCursorRO, models::StoredBlockBodyIndices, tables, transaction::DbTx};
use reth_execution_types::RecoveredBlockAndExecutionOutput;
use reth_node_types::{BlockTy, BodyTy, HeaderTy, NodeTypes, ReceiptTy, TxTy};
use reth_primitives_traits::{Block as _, BlockBody as _, RecoveredBlock, SealedHeader};
use reth_prune_types::PruneSegment;
use reth_stages_types::StageId;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{BlockBodyIndicesProvider, BlockBodyReader};
use reth_storage_errors::provider::ProviderResult;
use std::{
    ops::{Range, RangeBounds, RangeInclusive},
    sync::Arc,
};

impl<TX: DbTx + 'static, N: NodeTypesForProvider> DatabaseProvider<TX, N> {
    fn recovered_block<H, HF, B, BF>(
        &self,
        id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
        header_by_number: HF,
        construct_block: BF,
    ) -> ProviderResult<Option<B>>
    where
        H: AsRef<HeaderTy<N>>,
        HF: FnOnce(BlockNumber) -> ProviderResult<Option<H>>,
        BF: FnOnce(H, BodyTy<N>, Vec<Address>) -> ProviderResult<Option<B>>,
    {
        let Some(block_number) = self.convert_hash_or_number(id)? else { return Ok(None) };
        self.ensure_body_available(block_number)?;
        let Some(header) = header_by_number(block_number)? else { return Ok(None) };

        // Get the block body
        //
        // If the body indices are not found, this means that the transactions either do not exist
        // in the database yet, or they do exit but are not indexed. If they exist but are not
        // indexed, we don't have enough information to return the block anyways, so we return
        // `None`.
        let Some(body) = self.block_body_indices(block_number)? else { return Ok(None) };

        let tx_range = body.tx_num_range();

        let transactions = if tx_range.is_empty() {
            vec![]
        } else {
            self.transactions_by_tx_range(tx_range.clone())?
        };

        let body = self
            .storage
            .reader()
            .read_block_bodies(self, vec![(header.as_ref(), transactions)])?
            .pop()
            .ok_or(ProviderError::InvalidStorageOutput)?;

        let senders = if tx_range.is_empty() {
            vec![]
        } else {
            let known_senders: HashMap<TxNumber, Address> =
                EitherReader::new_senders(self)?.senders_by_tx_range(tx_range.clone())?;

            let mut senders = Vec::with_capacity(body.transactions().len());
            for (tx_num, tx) in tx_range.zip(body.transactions()) {
                match known_senders.get(&tx_num) {
                    None => {
                        let sender = tx.recover_signer_unchecked()?;
                        senders.push(sender);
                    }
                    Some(sender) => senders.push(*sender),
                }
            }
            senders
        };

        construct_block(header, body, senders)
    }

    /// Returns a range of blocks from the database.
    ///
    /// Uses the provided `headers_range` to get the headers for the range, and `assemble_block` to
    /// construct blocks from the following inputs:
    ///     – Header
    ///     - Range of transaction numbers
    ///     – Ommers
    ///     – Withdrawals
    ///     – Senders
    fn block_range<F, H, HF, R>(
        &self,
        range: RangeInclusive<BlockNumber>,
        headers_range: HF,
        mut assemble_block: F,
    ) -> ProviderResult<Vec<R>>
    where
        H: AsRef<HeaderTy<N>>,
        HF: FnOnce(RangeInclusive<BlockNumber>) -> ProviderResult<Vec<H>>,
        F: FnMut(H, BodyTy<N>, Range<TxNumber>) -> ProviderResult<R>,
    {
        if range.is_empty() {
            return Ok(Vec::new())
        }

        self.ensure_body_available(*range.start())?;

        let len = range.end().saturating_sub(*range.start()) as usize + 1;
        let mut blocks = Vec::with_capacity(len);

        let headers = headers_range(range.clone())?;

        // If the body indices are not found, this means that the transactions either do
        // not exist in the database yet, or they do exit but are
        // not indexed. If they exist but are not indexed, we don't
        // have enough information to return the block anyways, so
        // we skip the block.
        let present_headers = self
            .block_body_indices_range(range)?
            .into_iter()
            .map(|b| b.tx_num_range())
            .zip(headers)
            .collect::<Vec<_>>();

        let mut inputs = Vec::with_capacity(present_headers.len());
        for (tx_range, header) in &present_headers {
            let transactions = if tx_range.is_empty() {
                Vec::new()
            } else {
                self.transactions_by_tx_range(tx_range.clone())?
            };

            inputs.push((header.as_ref(), transactions));
        }

        let bodies = self.storage.reader().read_block_bodies(self, inputs)?;

        for ((tx_range, header), body) in present_headers.into_iter().zip(bodies) {
            blocks.push(assemble_block(header, body, tx_range)?);
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
    ///     – Senders
    fn block_with_senders_range<H, HF, B, BF>(
        &self,
        range: RangeInclusive<BlockNumber>,
        headers_range: HF,
        assemble_block: BF,
    ) -> ProviderResult<Vec<B>>
    where
        H: AsRef<HeaderTy<N>>,
        HF: Fn(RangeInclusive<BlockNumber>) -> ProviderResult<Vec<H>>,
        BF: Fn(H, BodyTy<N>, Vec<Address>) -> ProviderResult<B>,
    {
        self.block_range(range, headers_range, |header, body, tx_range| {
            let senders = if tx_range.is_empty() {
                Vec::new()
            } else {
                let known_senders: HashMap<TxNumber, Address> =
                    EitherReader::new_senders(self)?.senders_by_tx_range(tx_range.clone())?;

                let mut senders = Vec::with_capacity(body.transactions().len());
                for (tx_num, tx) in tx_range.zip(body.transactions()) {
                    match known_senders.get(&tx_num) {
                        None => {
                            // recover the sender from the transaction if not found
                            let sender = tx.recover_signer_unchecked()?;
                            senders.push(sender);
                        }
                        Some(sender) => senders.push(*sender),
                    }
                }

                senders
            };

            assemble_block(header, body, senders)
        })
    }

    /// The snapshot Bodies checkpoint can expire a block whose state and receipts still exist.
    fn ensure_body_available(&self, requested: BlockNumber) -> ProviderResult<()> {
        let checkpoint = self.get_prune_checkpoint(PruneSegment::Bodies)?;
        // Reconciling an older download's checkpoint only lowers it, so later blocks skip that.
        if requested >= self.static_file_provider.earliest_history_height() &&
            checkpoint
                .and_then(|checkpoint| checkpoint.block_number)
                .is_none_or(|pruned| pruned < requested)
        {
            return Ok(())
        }
        let earliest_available = self.earliest_body_block(checkpoint)?;
        if requested < earliest_available {
            return Err(ProviderError::BlockExpired { requested, earliest_available })
        }
        Ok(())
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> HeaderProvider for DatabaseProvider<TX, N> {
    type Header = HeaderTy<N>;

    fn header(&self, block_hash: BlockHash) -> ProviderResult<Option<Self::Header>> {
        if let Some(num) = self.block_number(block_hash)? {
            Ok(self.header_by_number(num)?)
        } else {
            Ok(None)
        }
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Self::Header>> {
        self.static_file_provider.header_by_number(num)
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        self.static_file_provider.headers_range(range)
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        self.static_file_provider.sealed_header(number)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        self.static_file_provider.sealed_headers_while(range, predicate)
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> reth_storage_api::DurableBlockReader
    for DatabaseProvider<TX, N>
{
}

impl<TX: DbTx + 'static, N: NodeTypes> BlockHashReader for DatabaseProvider<TX, N> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.static_file_provider.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.static_file_provider.canonical_hashes_range(start, end)
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> BlockNumReader for DatabaseProvider<TX, N> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        let best_number = self.best_block_number()?;
        let best_hash = self.block_hash(best_number)?.unwrap_or_default();
        Ok(ChainInfo { best_hash, best_number })
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        // The best block number is tracked via the finished stage which gets updated in the same tx
        // when new blocks committed
        Ok(self
            .get_stage_checkpoint(StageId::Finish)?
            .map(|checkpoint| checkpoint.block_number)
            .unwrap_or_default())
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        self.static_file_provider.last_block_number()
    }

    fn earliest_block_number(&self) -> ProviderResult<BlockNumber> {
        self.earliest_body_block(self.get_prune_checkpoint(PruneSegment::Bodies)?)
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        Ok(self.tx.get::<tables::HeaderNumbers>(hash)?)
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> BlockReader for DatabaseProvider<TX, N> {
    type Block = BlockTy<N>;

    fn find_block_by_hash(
        &self,
        hash: B256,
        source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
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
    ///
    /// Returns an error if the requested block is below the earliest available history.
    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        if let Some(number) = self.convert_hash_or_number(id)? {
            self.ensure_body_available(number)?;

            let Some(header) = self.header_by_number(number)? else { return Ok(None) };

            // If the body indices are not found, this means that the transactions either do not
            // exist in the database yet, or they do exit but are not indexed.
            // If they exist but are not indexed, we don't have enough
            // information to return the block anyways, so we return `None`.
            let Some(transactions) = self.transactions_by_block(number.into())? else {
                return Ok(None)
            };

            let body = self
                .storage
                .reader()
                .read_block_bodies(self, vec![(&header, transactions)])?
                .pop()
                .ok_or(ProviderError::InvalidStorageOutput)?;

            return Ok(Some(Self::Block::new(header, body)))
        }

        Ok(None)
    }

    fn pending_block(&self) -> ProviderResult<Option<Arc<RecoveredBlock<Self::Block>>>> {
        Ok(None)
    }

    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<RecoveredBlockAndExecutionOutput<Self::Block, Self::Receipt>>> {
        Ok(None)
    }

    /// Returns the block with senders with matching number or hash from database.
    ///
    /// **NOTE: The transactions have invalid hashes, since they would need to be calculated on the
    /// spot, and we want fast querying.**
    ///
    /// If the header for this block is not found, this returns `None`.
    /// If the header is found, but the transactions either do not exist, or are not indexed, this
    /// will return None.
    fn recovered_block(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.recovered_block(
            id,
            transaction_kind,
            |block_number| self.header_by_number(block_number),
            |header, body, senders| {
                Self::Block::new(header, body)
                    // Note: we're using unchecked here because we know the block contains valid txs
                    // wrt to its height and can ignore the s value check so pre
                    // EIP-2 txs are allowed
                    .try_into_recovered_unchecked(senders)
                    .map(Some)
                    .map_err(|_| ProviderError::SenderRecoveryError)
            },
        )
    }

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.recovered_block(
            id,
            transaction_kind,
            |block_number| self.sealed_header(block_number),
            |header, body, senders| {
                Self::Block::new_sealed(header, body)
                    // Note: we're using unchecked here because we know the block contains valid txs
                    // wrt to its height and can ignore the s value check so pre
                    // EIP-2 txs are allowed
                    .try_with_senders_unchecked(senders)
                    .map(Some)
                    .map_err(|_| ProviderError::SenderRecoveryError)
            },
        )
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>> {
        self.block_range(
            range,
            |range| self.headers_range(range),
            |header, body, _| Ok(Self::Block::new(header, body)),
        )
    }

    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.block_with_senders_range(
            range,
            |range| self.headers_range(range),
            |header, body, senders| {
                Self::Block::new(header, body)
                    .try_into_recovered_unchecked(senders)
                    .map_err(|_| ProviderError::SenderRecoveryError)
            },
        )
    }

    fn recovered_block_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.block_with_senders_range(
            range,
            |range| self.sealed_headers_range(range),
            |header, body, senders| {
                Self::Block::new_sealed(header, body)
                    .try_with_senders(senders)
                    .map_err(|_| ProviderError::SenderRecoveryError)
            },
        )
    }

    fn block_by_transaction_id(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        Ok(self
            .tx
            .cursor_read::<tables::TransactionBlocks>()?
            .seek(id)
            .map(|b| b.map(|(_, bn)| bn))?)
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> TransactionsProviderExt
    for DatabaseProvider<TX, N>
{
    /// Recovers transaction hashes by walking through `Transactions` table and
    /// calculating them in a parallel manner. Returned unsorted.
    fn transaction_hashes_by_range(
        &self,
        tx_range: Range<TxNumber>,
    ) -> ProviderResult<Vec<(TxHash, TxNumber)>> {
        self.static_file_provider.transaction_hashes_by_range(tx_range)
    }
}

// Calculates the hash of the given transaction
impl<TX: DbTx + 'static, N: NodeTypesForProvider> TransactionsProvider for DatabaseProvider<TX, N> {
    type Transaction = TxTy<N>;

    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.with_rocksdb_snapshot(|rocksdb_ref| {
            let mut reader = EitherReader::new_transaction_hash_numbers(self, rocksdb_ref)?;
            reader.get_transaction_hash_number(tx_hash)
        })
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        self.static_file_provider.transaction_by_id(id)
    }

    fn transaction_by_id_unhashed(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        self.static_file_provider.transaction_by_id_unhashed(id)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
        if let Some(id) = self.transaction_id(hash)? {
            Ok(self.transaction_by_id_unhashed(id)?)
        } else {
            Ok(None)
        }
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
        Ok(self
            .transaction_by_hash_with_meta_and_id(tx_hash)?
            .map(|(transaction, meta, _)| (transaction, meta)))
    }

    fn transaction_by_hash_with_meta_and_sender(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta, Option<Address>)>> {
        let Some((transaction, meta, transaction_id)) =
            self.transaction_by_hash_with_meta_and_id(tx_hash)?
        else {
            return Ok(None)
        };
        Ok(Some((transaction, meta, self.transaction_sender(transaction_id)?)))
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        if let Some(block_number) = self.convert_hash_or_number(id)? &&
            let Some(body) = self.block_body_indices(block_number)?
        {
            let tx_range = body.tx_num_range();
            return if tx_range.is_empty() {
                Ok(Some(Vec::new()))
            } else {
                self.transactions_by_tx_range(tx_range).map(Some)
            }
        }
        Ok(None)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        let range = to_range(range);

        self.block_body_indices_range(range.start..=range.end.saturating_sub(1))?
            .into_iter()
            .map(|body| {
                let tx_num_range = body.tx_num_range();
                if tx_num_range.is_empty() {
                    Ok(Vec::new())
                } else {
                    self.transactions_by_tx_range(tx_num_range)
                }
            })
            .collect()
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        self.static_file_provider.transactions_by_tx_range(range)
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        if EitherWriterDestination::senders(self).is_static_file() {
            self.static_file_provider.senders_by_tx_range(range)
        } else {
            self.cursor_read_collect::<tables::TransactionSenders>(range)
        }
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        if EitherWriterDestination::senders(self).is_static_file() {
            self.static_file_provider.transaction_sender(id)
        } else {
            Ok(self.tx.get::<tables::TransactionSenders>(id)?)
        }
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> DatabaseProvider<TX, N> {
    /// Returns the transaction with the metadata of the block it was mined in, and its id.
    fn transaction_by_hash_with_meta_and_id(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(TxTy<N>, TransactionMeta, TxNumber)>> {
        if let Some(transaction_id) = self.transaction_id(tx_hash)? &&
            let Some(transaction) = self.transaction_by_id_unhashed(transaction_id)? &&
            let Some(block_number) = self.block_by_transaction_id(transaction_id)? &&
            let Some(sealed_header) = self.sealed_header(block_number)?
        {
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
                    base_fee: header.base_fee_per_gas(),
                    excess_blob_gas: header.excess_blob_gas(),
                    timestamp: header.timestamp(),
                };

                return Ok(Some((transaction, meta, transaction_id)))
            }
        }

        Ok(None)
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> ReceiptProvider for DatabaseProvider<TX, N> {
    type Receipt = ReceiptTy<N>;

    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Receipts,
            id,
            |static_file| static_file.receipt(id),
            || Ok(self.tx.get::<tables::Receipts<Self::Receipt>>(id)?),
        )
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
        if let Some(id) = self.transaction_id(hash)? {
            self.receipt(id)
        } else {
            Ok(None)
        }
    }

    fn receipts_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        if let Some(number) = self.convert_hash_or_number(block)? &&
            let Some(body) = self.block_body_indices(number)?
        {
            let tx_range = body.tx_num_range();
            return if tx_range.is_empty() {
                Ok(Some(Vec::new()))
            } else {
                let receipts = self.receipts_by_tx_range(tx_range)?;

                if receipts.len() != body.tx_count as usize {
                    return Ok(None)
                }

                Ok(Some(receipts))
            }
        }
        Ok(None)
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Receipts,
            to_range(range),
            |static_file, range, _| static_file.receipts_by_tx_range(range),
            |range, _| self.cursor_read_collect::<tables::Receipts<Self::Receipt>>(range),
            |_| true,
        )
    }

    fn receipts_by_block_range(
        &self,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Receipt>>> {
        if block_range.is_empty() {
            return Ok(Vec::new());
        }

        // collect block body indices for each block in the range
        let range_len = block_range.end().saturating_sub(*block_range.start()) as usize + 1;
        let mut block_body_indices = Vec::with_capacity(range_len);
        for block_num in block_range {
            if let Some(indices) = self.block_body_indices(block_num)? {
                block_body_indices.push(indices);
            } else {
                // use default indices for missing blocks (empty block)
                block_body_indices.push(StoredBlockBodyIndices::default());
            }
        }

        if block_body_indices.is_empty() {
            return Ok(Vec::new());
        }

        // find blocks with transactions to determine transaction range
        let non_empty_blocks: Vec<_> =
            block_body_indices.iter().filter(|indices| indices.tx_count > 0).collect();

        if non_empty_blocks.is_empty() {
            // all blocks are empty
            return Ok(vec![Vec::new(); block_body_indices.len()]);
        }

        // calculate the overall transaction range
        let first_tx = non_empty_blocks[0].first_tx_num();
        let last_tx = non_empty_blocks[non_empty_blocks.len() - 1].last_tx_num();

        // fetch all receipts in the transaction range
        let all_receipts = self.receipts_by_tx_range(first_tx..=last_tx)?;
        let mut receipts_iter = all_receipts.into_iter();

        // distribute receipts to their respective blocks
        let mut result = Vec::with_capacity(block_body_indices.len());
        for indices in &block_body_indices {
            if indices.tx_count == 0 {
                result.push(Vec::new());
            } else {
                let block_receipts =
                    receipts_iter.by_ref().take(indices.tx_count as usize).collect();
                result.push(block_receipts);
            }
        }

        Ok(result)
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> BlockBodyIndicesProvider
    for DatabaseProvider<TX, N>
{
    fn block_body_indices(&self, num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        Ok(self.tx.get::<tables::BlockBodyIndices>(num)?)
    }

    fn block_body_indices_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<StoredBlockBodyIndices>> {
        self.cursor_read_collect::<tables::BlockBodyIndices>(range)
    }
}
