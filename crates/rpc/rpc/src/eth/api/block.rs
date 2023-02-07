//! Contains RPC handler implementations specific to blocks.

use crate::{eth::error::EthResult, EthApi};
use reth_primitives::{rpc::BlockId, BlockNumber, H256};
use reth_provider::{BlockProvider, StateProviderFactory};
use reth_rpc_types::RichBlock;

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + 'static,
{
    pub(crate) async fn block_by_hash(
        &self,
        hash: H256,
        _full: bool,
    ) -> EthResult<Option<RichBlock>> {
        let block = self.client().block(BlockId::Hash(hash.0.into()))?;
        todo!()
    }

    pub(crate) async fn block_by_number(
        &self,
        number: BlockNumber,
        _full: bool,
    ) -> EthResult<Option<RichBlock>> {
        let block = self.client().block(BlockId::Number(number.into()))?;
        todo!()
    }
}
