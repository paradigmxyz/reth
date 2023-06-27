//! Contains RPC handler implementations specific to blocks.

use crate::{
    eth::{
        api::transactions::build_transaction_receipt_with_block_receipts,
        error::{EthApiError, EthResult},
    },
    EthApi,
};
use reth_primitives::{BlockId, BlockNumberOrTag, TransactionMeta};
use reth_provider::{BlockReaderIdExt, EvmEnvProvider, StateProviderFactory};
use reth_rpc_types::{Block, Index, RichBlock, TransactionReceipt};

impl<Provider, Pool, Network> EthApi<Provider, Pool, Network>
where
    Provider: BlockReaderIdExt + StateProviderFactory + EvmEnvProvider + 'static,
{
    /// Returns the uncle headers of the given block
    ///
    /// Returns an empty vec if there are none.
    pub(crate) fn ommers(
        &self,
        block_id: impl Into<BlockId>,
    ) -> EthResult<Option<Vec<reth_primitives::Header>>> {
        let block_id = block_id.into();
        Ok(self.provider().ommers_by_id(block_id)?)
    }

    pub(crate) async fn ommer_by_block_and_index(
        &self,
        block_id: impl Into<BlockId>,
        index: Index,
    ) -> EthResult<Option<RichBlock>> {
        let block_id = block_id.into();

        let uncles = if block_id.is_pending() {
            // Pending block can be fetched directly without need for caching
            self.provider().pending_block()?.map(|block| block.ommers)
        } else {
            self.provider().ommers_by_id(block_id)?
        }
        .unwrap_or_default();

        let index = usize::from(index);
        let uncle = uncles
            .into_iter()
            .nth(index)
            .map(|header| Block::uncle_block_from_header(header).into());
        Ok(uncle)
    }

    /// Returns all transaction receipts in the block.
    ///
    /// Returns `None` if the block wasn't found.
    pub(crate) async fn block_receipts(
        &self,
        number: BlockNumberOrTag,
    ) -> EthResult<Option<Vec<TransactionReceipt>>> {
        let mut block_and_receipts = None;

        if number.is_pending() {
            block_and_receipts = self.provider().pending_block_and_receipts()?;
        } else if let Some(block_hash) = self.provider().block_hash_for_id(number.into())? {
            block_and_receipts = self.cache().get_block_and_receipts(block_hash).await?;
        }

        if let Some((block, receipts)) = block_and_receipts {
            let block_number = block.number;
            let base_fee = block.base_fee_per_gas;
            let block_hash = block.hash;
            let receipts = block
                .body
                .into_iter()
                .zip(receipts.clone())
                .enumerate()
                .map(|(idx, (tx, receipt))| {
                    let meta = TransactionMeta {
                        tx_hash: tx.hash,
                        index: idx as u64,
                        block_hash,
                        block_number,
                        base_fee,
                    };
                    build_transaction_receipt_with_block_receipts(tx, meta, receipt, &receipts)
                })
                .collect::<EthResult<Vec<_>>>();
            return receipts.map(Some)
        }

        Ok(None)
    }

    /// Returns the number transactions in the given block.
    ///
    /// Returns `None` if the block does not exist
    pub(crate) async fn block_transaction_count(
        &self,
        block_id: impl Into<BlockId>,
    ) -> EthResult<Option<usize>> {
        let block_id = block_id.into();

        if block_id.is_pending() {
            // Pending block can be fetched directly without need for caching
            return Ok(self.provider().pending_block()?.map(|block| block.body.len()))
        }

        let block_hash = match self.provider().block_hash_for_id(block_id)? {
            Some(block_hash) => block_hash,
            None => return Ok(None),
        };

        Ok(self.cache().get_block_transactions(block_hash).await?.map(|txs| txs.len()))
    }

    /// Returns the block object for the given block id.
    pub(crate) async fn block(
        &self,
        block_id: impl Into<BlockId>,
    ) -> EthResult<Option<reth_primitives::SealedBlock>> {
        let block_id = block_id.into();

        if block_id.is_pending() {
            // Pending block can be fetched directly without need for caching
            return Ok(self.provider().pending_block()?)
        }

        let block_hash = match self.provider().block_hash_for_id(block_id)? {
            Some(block_hash) => block_hash,
            None => return Ok(None),
        };

        Ok(self.cache().get_sealed_block(block_hash).await?)
    }

    /// Returns the populated rpc block object for the given block id.
    ///
    /// If `full` is true, the block object will contain all transaction objects, otherwise it will
    /// only contain the transaction hashes.
    pub(crate) async fn rpc_block(
        &self,
        block_id: impl Into<BlockId>,
        full: bool,
    ) -> EthResult<Option<RichBlock>> {
        let block = match self.block(block_id).await? {
            Some(block) => block,
            None => return Ok(None),
        };
        let block_hash = block.hash;
        let total_difficulty = self
            .provider()
            .header_td_by_number(block.number)?
            .ok_or(EthApiError::UnknownBlockNumber)?;
        let block =
            Block::from_block(block.into(), total_difficulty, full.into(), Some(block_hash))?;
        Ok(Some(block.into()))
    }
}
