//! Contains RPC handler implementations specific to blocks.

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_primitives::BlockId;
use reth_provider::{BlockProvider, StateProviderFactory};
use reth_rpc_types::{Block, RichBlock};

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + 'static,
{
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
                .ok_or_else(|| EthApiError::UnknownBlockNumber)?;
            let total_difficulty = self
                .client()
                .header_td(&block_hash)?
                .ok_or_else(|| EthApiError::UnknownBlockNumber)?;
            let block = Block::from_block(block, total_difficulty, full.into(), Some(block_hash))?;
            Ok(Some(block.into()))
        } else {
            Ok(None)
        }
    }
}
