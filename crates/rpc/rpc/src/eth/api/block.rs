//! Contains RPC handler implementations specific to blocks.

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};

use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_primitives::BlockId;
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ReceiptProvider,
    StateProviderFactory,
};
use reth_rpc_types::{Header, Index, RichBlock};
use reth_rpc_types_compat::block::{from_block, uncle_block_from_header};
use reth_transaction_pool::TransactionPool;

use crate::eth::api::EthBlocks;

use super::traits::LoadPendingBlock;

impl<Provider, Pool, Network, EvmConfig> EthBlocks for EthApi<Provider, Pool, Network, EvmConfig>
where
    Provider: BlockReaderIdExt + BlockReader + ReceiptProvider,
{
    #[inline]
    fn provider(&self) -> &impl BlockReaderIdExt {
        self.inner.provider()
    }
}

impl<Provider, Pool, Network, EvmConfig> EthApi<Provider, Pool, Network, EvmConfig>
where
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Pool: TransactionPool + Clone + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
    EvmConfig: ConfigureEvm + 'static,
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
        let uncle =
            uncles.into_iter().nth(index).map(|header| uncle_block_from_header(header).into());
        Ok(uncle)
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
    ) -> EthResult<Option<reth_primitives::SealedBlock>>
    where
        Self: LoadPendingBlock,
    {
        self.block_with_senders(block_id)
            .await
            .map(|maybe_block| maybe_block.map(|block| block.block))
    }

    /// Returns the block object for the given block id.
    pub(crate) async fn block_with_senders(
        &self,
        block_id: impl Into<BlockId>,
    ) -> EthResult<Option<reth_primitives::SealedBlockWithSenders>>
    where
        Self: LoadPendingBlock,
    {
        let block_id = block_id.into();

        if block_id.is_pending() {
            // Pending block can be fetched directly without need for caching
            let maybe_pending = self.provider().pending_block_with_senders()?;
            return if maybe_pending.is_some() {
                Ok(maybe_pending)
            } else {
                self.local_pending_block().await
            }
        }

        let block_hash = match self.provider().block_hash_for_id(block_id)? {
            Some(block_hash) => block_hash,
            None => return Ok(None),
        };

        Ok(self.cache().get_sealed_block_with_senders(block_hash).await?)
    }

    /// Returns the populated rpc block object for the given block id.
    ///
    /// If `full` is true, the block object will contain all transaction objects, otherwise it will
    /// only contain the transaction hashes.
    pub(crate) async fn rpc_block(
        &self,
        block_id: impl Into<BlockId>,
        full: bool,
    ) -> EthResult<Option<RichBlock>>
    where
        Self: LoadPendingBlock,
    {
        let block = match self.block_with_senders(block_id).await? {
            Some(block) => block,
            None => return Ok(None),
        };
        let block_hash = block.hash();
        let total_difficulty = self
            .provider()
            .header_td_by_number(block.number)?
            .ok_or(EthApiError::UnknownBlockNumber)?;
        let block = from_block(block.unseal(), total_difficulty, full.into(), Some(block_hash))?;
        Ok(Some(block.into()))
    }

    /// Returns the block header for the given block id.
    pub(crate) async fn rpc_block_header(
        &self,
        block_id: impl Into<BlockId>,
    ) -> EthResult<Option<Header>>
    where
        Self: LoadPendingBlock,
    {
        let header = self.rpc_block(block_id, false).await?.map(|block| block.inner.header);
        Ok(header)
    }
}
