//! Contains RPC handler implementations specific to blocks.

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_primitives::{rpc::BlockId, H256};
use reth_provider::{BlockProvider, StateProviderFactory};
use reth_rpc_types::{Block, RichBlock};

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + 'static,
{
    pub(crate) async fn block_by_hash(
        &self,
        hash: H256,
        full: bool,
    ) -> EthResult<Option<RichBlock>> {
        if let Some(block) = self.client().block_by_hash(hash)? {
            let total_difficulty =
                self.client().header_td(&hash)?.ok_or_else(|| EthApiError::UnknownBlockNumber)?;
            let block = Block::from_block(block, total_difficulty, full.into())?;
            Ok(Some(block.into()))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn block_by_number(
        &self,
        number: u64,
        _full: bool,
    ) -> EthResult<Option<RichBlock>> {
        let block = self.client().block(BlockId::Number(number.into()))?;
        if let Some(_block) = block {
            // TODO: GET TD FOR BLOCK - needs block provider? or header provider?
            // let total_difficulty = todo!();
            // let rich_block = Block::from_block_full(block, total_difficulty);
            todo!()
        } else {
            Ok(None)
        }
    }
}
