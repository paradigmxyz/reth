//! Contains RPC handler implementations specific to state.

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_interfaces::Result;
use reth_primitives::{rpc::BlockId, Address, Bytes, H256, U256};
use reth_provider::{BlockProvider, StateProvider, StateProviderFactory};

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + 'static,
{
    pub(crate) fn get_code(&self, address: Address, block_id: Option<BlockId>) -> EthResult<Bytes> {
        let state =
            self.state_at_block_id_or_latest(block_id)?.ok_or(EthApiError::UnknownBlockNumber)?;
        let code = state.account_code(address)?.unwrap_or_default();
        Ok(code)
    }

    async fn storage_at(
        &self,
        _address: Address,
        _index: U256,
        _block_number: Option<BlockId>,
    ) -> Result<H256> {
        todo!()
    }
}
