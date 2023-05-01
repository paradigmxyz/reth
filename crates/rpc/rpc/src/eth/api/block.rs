//! Contains RPC handler implementations specific to blocks.

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_primitives::BlockId;
use reth_provider::{BlockProvider, EvmEnvProvider, StateProviderFactory};
use reth_rpc_types::{Block, Index, RichBlock};

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
{
    /// Returns the uncle headers of the given block
    ///
    /// Returns an empty vec if there are none.
    pub(crate) fn ommers(
        &self,
        block_id: impl Into<BlockId>,
    ) -> EthResult<Option<Vec<reth_primitives::Header>>> {
        let block_id = block_id.into();
        Ok(self.client().ommers(block_id)?)
    }

    pub(crate) async fn ommer_by_block_and_index(
        &self,
        block_id: impl Into<BlockId>,
        index: Index,
    ) -> EthResult<Option<RichBlock>> {
        let block_id = block_id.into();
        let index = usize::from(index);
        let uncles = self.client().ommers(block_id)?.unwrap_or_default();
        let uncle = uncles
            .into_iter()
            .nth(index)
            .map(|header| Block::uncle_block_from_header(header).into());
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
        // TODO support pending block

        let block_hash = match self.client().block_hash_for_id(block_id)? {
            Some(block_hash) => block_hash,
            None => return Ok(None),
        };

        Ok(self.cache().get_block_transactions(block_hash).await?.map(|txs| txs.len()))
    }

    /// Returns the block header for the given block id.
    pub(crate) async fn header(
        &self,
        block_id: impl Into<BlockId>,
    ) -> EthResult<Option<reth_primitives::SealedHeader>> {
        Ok(self.block(block_id).await?.map(|block| block.header))
    }

    /// Returns the block object for the given block id.
    pub(crate) async fn block(
        &self,
        block_id: impl Into<BlockId>,
    ) -> EthResult<Option<reth_primitives::SealedBlock>> {
        let block_id = block_id.into();
        // TODO support pending block
        let block_hash = match self.client().block_hash_for_id(block_id)? {
            Some(block_hash) => block_hash,
            None => return Ok(None),
        };

        Ok(self.cache().get_block(block_hash).await?.map(|block| block.seal(block_hash)))
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
        let total_difficulty =
            self.client().header_td(&block_hash)?.ok_or(EthApiError::UnknownBlockNumber)?;
        let block =
            Block::from_block(block.into(), total_difficulty, full.into(), Some(block_hash))?;
        Ok(Some(block.into()))
    }
}
