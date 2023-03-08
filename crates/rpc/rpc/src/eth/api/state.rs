//! Contains RPC handler implementations specific to state.

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_primitives::{Address, BlockId, Bytes, H256, U256};
use reth_provider::{BlockProvider, EvmEnvProvider, StateProvider, StateProviderFactory};

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
{
    pub(crate) fn get_code(&self, address: Address, block_id: Option<BlockId>) -> EthResult<Bytes> {
        let state =
            self.state_at_block_id_or_latest(block_id)?.ok_or(EthApiError::UnknownBlockNumber)?;
        let code = state.account_code(address)?.unwrap_or_default();
        Ok(code.original_bytes().into())
    }

    pub(crate) fn balance(&self, address: Address, block_id: Option<BlockId>) -> EthResult<U256> {
        let state =
            self.state_at_block_id_or_latest(block_id)?.ok_or(EthApiError::UnknownBlockNumber)?;
        let balance = state.account_balance(address)?.unwrap_or_default();
        Ok(balance)
    }

    pub(crate) fn get_transaction_count(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> EthResult<U256> {
        let state =
            self.state_at_block_id_or_latest(block_id)?.ok_or(EthApiError::UnknownBlockNumber)?;
        let nonce = U256::from(state.account_nonce(address)?.unwrap_or_default());
        Ok(nonce)
    }

    pub(crate) fn storage_at(
        &self,
        address: Address,
        index: U256,
        block_id: Option<BlockId>,
    ) -> EthResult<H256> {
        let state =
            self.state_at_block_id_or_latest(block_id)?.ok_or(EthApiError::UnknownBlockNumber)?;
        let storage_key = H256(index.to_be_bytes());
        let value = state.storage(address, storage_key)?.unwrap_or_default();
        Ok(H256(value.to_be_bytes()))
    }
}
