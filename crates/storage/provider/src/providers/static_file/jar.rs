use super::{
    metrics::{StaticFileProviderMetrics, StaticFileProviderOperation},
    LoadedJarRef,
};
use crate::{
    to_range, BlockHashReader, BlockNumReader, HeaderProvider, ReceiptProvider,
    TransactionsProvider,
};
use alloy_consensus::transaction::TransactionMeta;
use alloy_eips::{eip2718::Encodable2718, BlockHashOrNumber};
use alloy_primitives::{Address, BlockHash, BlockNumber, TxHash, TxNumber, B256, U256};
use reth_chainspec::ChainInfo;
use reth_db::{
    static_file::{
        BlockHashMask, HeaderMask, HeaderWithHashMask, ReceiptMask, StaticFileCursor,
        TDWithHashMask, TotalDifficultyMask, TransactionMask,
    },
    table::{Decompress, Value},
};
use reth_node_types::NodePrimitives;
use reth_primitives::{transaction::recover_signers, SealedHeader};
use reth_primitives_traits::SignedTransaction;
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use std::{
    fmt::Debug,
    ops::{Deref, RangeBounds},
    sync::Arc,
};

/// Provider over a specific `NippyJar` and range.
#[derive(Debug)]
pub struct StaticFileJarProvider<'a, N> {
    /// Main static file segment
    jar: LoadedJarRef<'a>,
    /// Another kind of static file segment to help query data from the main one.
    auxiliary_jar: Option<Box<Self>>,
    /// Metrics for the static files.
    metrics: Option<Arc<StaticFileProviderMetrics>>,
    /// Node primitives
    _pd: std::marker::PhantomData<N>,
}

impl<'a, N: NodePrimitives> Deref for StaticFileJarProvider<'a, N> {
    type Target = LoadedJarRef<'a>;
    fn deref(&self) -> &Self::Target {
        &self.jar
    }
}

impl<'a, N: NodePrimitives> From<LoadedJarRef<'a>> for StaticFileJarProvider<'a, N> {
    fn from(value: LoadedJarRef<'a>) -> Self {
        StaticFileJarProvider {
            jar: value,
            auxiliary_jar: None,
            metrics: None,
            _pd: Default::default(),
        }
    }
}

impl<'a, N: NodePrimitives> StaticFileJarProvider<'a, N> {
    /// Provides a cursor for more granular data access.
    pub fn cursor<'b>(&'b self) -> ProviderResult<StaticFileCursor<'a>>
    where
        'b: 'a,
    {
        let result = StaticFileCursor::new(self.value(), self.mmap_handle())?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                self.segment(),
                StaticFileProviderOperation::InitCursor,
                None,
            );
        }

        Ok(result)
    }

    /// Adds a new auxiliary static file to help query data from the main one
    pub fn with_auxiliary(mut self, auxiliary_jar: Self) -> Self {
        self.auxiliary_jar = Some(Box::new(auxiliary_jar));
        self
    }

    /// Enables metrics on the provider.
    pub fn with_metrics(mut self, metrics: Arc<StaticFileProviderMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }
}

impl<N: NodePrimitives<BlockHeader: Value>> HeaderProvider for StaticFileJarProvider<'_, N> {
    type Header = N::BlockHeader;

    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Self::Header>> {
        Ok(self
            .cursor()?
            .get_two::<HeaderWithHashMask<Self::Header>>(block_hash.into())?
            .filter(|(_, hash)| hash == block_hash)
            .map(|(header, _)| header))
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Self::Header>> {
        self.cursor()?.get_one::<HeaderMask<Self::Header>>(num.into())
    }

    fn header_td(&self, block_hash: &BlockHash) -> ProviderResult<Option<U256>> {
        Ok(self
            .cursor()?
            .get_two::<TDWithHashMask>(block_hash.into())?
            .filter(|(_, hash)| hash == block_hash)
            .map(|(td, _)| td.into()))
    }

    fn header_td_by_number(&self, num: BlockNumber) -> ProviderResult<Option<U256>> {
        Ok(self.cursor()?.get_one::<TotalDifficultyMask>(num.into())?.map(Into::into))
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        let range = to_range(range);

        let mut cursor = self.cursor()?;
        let mut headers = Vec::with_capacity((range.end - range.start) as usize);

        for num in range {
            if let Some(header) = cursor.get_one::<HeaderMask<Self::Header>>(num.into())? {
                headers.push(header);
            }
        }

        Ok(headers)
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        Ok(self
            .cursor()?
            .get_two::<HeaderWithHashMask<Self::Header>>(number.into())?
            .map(|(header, hash)| SealedHeader::new(header, hash)))
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        mut predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        let range = to_range(range);

        let mut cursor = self.cursor()?;
        let mut headers = Vec::with_capacity((range.end - range.start) as usize);

        for number in range {
            if let Some((header, hash)) =
                cursor.get_two::<HeaderWithHashMask<Self::Header>>(number.into())?
            {
                let sealed = SealedHeader::new(header, hash);
                if !predicate(&sealed) {
                    break
                }
                headers.push(sealed);
            }
        }
        Ok(headers)
    }
}

impl<N: NodePrimitives> BlockHashReader for StaticFileJarProvider<'_, N> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.cursor()?.get_one::<BlockHashMask>(number.into())
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        let mut cursor = self.cursor()?;
        let mut hashes = Vec::with_capacity((end - start) as usize);

        for number in start..end {
            if let Some(hash) = cursor.get_one::<BlockHashMask>(number.into())? {
                hashes.push(hash)
            }
        }
        Ok(hashes)
    }
}

impl<N: NodePrimitives> BlockNumReader for StaticFileJarProvider<'_, N> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        // Information on live database
        Err(ProviderError::UnsupportedProvider)
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        // Information on live database
        Err(ProviderError::UnsupportedProvider)
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        // Information on live database
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        let mut cursor = self.cursor()?;

        Ok(cursor
            .get_one::<BlockHashMask>((&hash).into())?
            .and_then(|res| (res == hash).then(|| cursor.number()).flatten()))
    }
}

impl<N: NodePrimitives<SignedTx: Decompress + SignedTransaction>> TransactionsProvider
    for StaticFileJarProvider<'_, N>
{
    type Transaction = N::SignedTx;

    fn transaction_id(&self, hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        let mut cursor = self.cursor()?;

        Ok(cursor
            .get_one::<TransactionMask<Self::Transaction>>((&hash).into())?
            .and_then(|res| (res.trie_hash() == hash).then(|| cursor.number()).flatten()))
    }

    fn transaction_by_id(&self, num: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        self.cursor()?.get_one::<TransactionMask<Self::Transaction>>(num.into())
    }

    fn transaction_by_id_unhashed(
        &self,
        num: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        self.cursor()?.get_one::<TransactionMask<Self::Transaction>>(num.into())
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
        self.cursor()?.get_one::<TransactionMask<Self::Transaction>>((&hash).into())
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
        // Information required on indexing table [`tables::TransactionBlocks`]
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_block(&self, _id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        // Information on indexing table [`tables::TransactionBlocks`]
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_block(
        &self,
        _block_id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        // Related to indexing tables. Live database should get the tx_range and call static file
        // provider with `transactions_by_tx_range` instead.
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        // Related to indexing tables. Live database should get the tx_range and call static file
        // provider with `transactions_by_tx_range` instead.
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        let range = to_range(range);
        let mut cursor = self.cursor()?;
        let mut txes = Vec::with_capacity((range.end - range.start) as usize);

        for num in range {
            if let Some(tx) = cursor.get_one::<TransactionMask<Self::Transaction>>(num.into())? {
                txes.push(tx)
            }
        }
        Ok(txes)
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        let txs = self.transactions_by_tx_range(range)?;
        recover_signers(&txs, txs.len()).ok_or(ProviderError::SenderRecoveryError)
    }

    fn transaction_sender(&self, num: TxNumber) -> ProviderResult<Option<Address>> {
        Ok(self
            .cursor()?
            .get_one::<TransactionMask<Self::Transaction>>(num.into())?
            .and_then(|tx| tx.recover_signer()))
    }
}

impl<N: NodePrimitives<SignedTx: Decompress + SignedTransaction, Receipt: Decompress>>
    ReceiptProvider for StaticFileJarProvider<'_, N>
{
    type Receipt = N::Receipt;

    fn receipt(&self, num: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
        self.cursor()?.get_one::<ReceiptMask<Self::Receipt>>(num.into())
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
        if let Some(tx_static_file) = &self.auxiliary_jar {
            if let Some(num) = tx_static_file.transaction_id(hash)? {
                return self.receipt(num)
            }
        }
        Ok(None)
    }

    fn receipts_by_block(
        &self,
        _block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        // Related to indexing tables. StaticFile should get the tx_range and call static file
        // provider with `receipt()` instead for each
        Err(ProviderError::UnsupportedProvider)
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>> {
        let range = to_range(range);
        let mut cursor = self.cursor()?;
        let mut receipts = Vec::with_capacity((range.end - range.start) as usize);

        for num in range {
            if let Some(tx) = cursor.get_one::<ReceiptMask<Self::Receipt>>(num.into())? {
                receipts.push(tx)
            }
        }
        Ok(receipts)
    }
}
