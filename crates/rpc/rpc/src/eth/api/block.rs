//! Contains RPC handler implementations specific to blocks.

use crate::{eth::error::EthResult, EthApi};
use reth_primitives::H256;
use reth_provider::{BlockProvider, StateProviderFactory};
use reth_rpc_types::RichBlock;

impl<Pool, Client, Network> EthApi<Pool, Client, Network>
where
    Client: BlockProvider + StateProviderFactory + 'static,
{
    pub(crate) async fn block_by_hash(
        &self,
        _hash: H256,
        _full: bool,
    ) -> EthResult<Option<RichBlock>> {
        todo!()
    }
}
