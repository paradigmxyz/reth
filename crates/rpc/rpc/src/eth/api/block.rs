//! Contains RPC handler implementations specific to blocks.

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_primitives::BlockId;
use reth_provider::{BlockProvider, EvmEnvProvider, StateProviderFactory};
use reth_rpc_types::{Block, RichBlock};

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

    pub(crate) async fn block_transaction_count(
        &self,
        block_id: impl Into<BlockId>,
    ) -> EthResult<Option<usize>> {
        let block_id = block_id.into();
        // TODO support pending block

        if let Some(txs) = self.client().transactions_by_block(block_id)? {
            Ok(Some(txs.len()))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn block(
        &self,
        block_id: impl Into<BlockId>,
        full: bool,
    ) -> EthResult<Option<RichBlock>> {
        let block_id = block_id.into();
        // TODO support pending block

        if let Some(block) = self.client().block(block_id)? {
            let block_hash = self
                .client()
                .block_hash_for_id(block_id)?
                .ok_or(EthApiError::UnknownBlockNumber)?;
            let total_difficulty =
                self.client().header_td(&block_hash)?.ok_or(EthApiError::UnknownBlockNumber)?;
            let block = Block::from_block(block, total_difficulty, full.into(), Some(block_hash))?;
            Ok(Some(block.into()))
        } else {
            Ok(None)
        }
    }
}
