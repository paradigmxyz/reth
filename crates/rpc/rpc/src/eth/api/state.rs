//! Contains RPC handler implementations specific to state.

use crate::EthApi;
use reth_interfaces::Result;
use reth_primitives::{rpc::BlockId, Address, H256, U256};
use reth_provider::{BlockProvider, StateProviderFactory};

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + 'static,
{
    async fn storage_at(
        &self,
        _address: Address,
        _index: U256,
        _block_number: Option<BlockId>,
    ) -> Result<H256> {
        todo!()
    }
}
