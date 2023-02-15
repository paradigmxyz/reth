//! Implementation of the [`jsonrpsee`] generated [`reth_rpc_api::EthApiServer`] trait
//! Handles RPC requests for the `eth_` namespace.

use crate::{
    eth::api::EthApi,
    result::{internal_rpc_err, ToRpcResult},
};
use jsonrpsee::core::RpcResult as Result;
use reth_primitives::{
    rpc::{transaction::eip2930::AccessListWithGasUsed, BlockId, BlockNumber},
    Address, Bytes, H256, H64, U256, U64,
};
use reth_provider::{BlockProvider, StateProviderFactory};
use reth_rpc_api::EthApiServer;
use reth_rpc_types::{
    CallRequest, EIP1186AccountProofResponse, FeeHistory, Index, RichBlock, SyncStatus,
    TransactionReceipt, TransactionRequest, Work,
};
use reth_transaction_pool::TransactionPool;
use serde_json::Value;

use super::EthApiSpec;

#[async_trait::async_trait]
impl<Client, Pool, Network> EthApiServer for EthApi<Client, Pool, Network>
where
    Self: EthApiSpec,
    Pool: TransactionPool + 'static,
    Client: BlockProvider + StateProviderFactory + 'static,
    Network: 'static,
{
    async fn protocol_version(&self) -> Result<U64> {
        EthApiSpec::protocol_version(self).await.to_rpc_result()
    }

    fn syncing(&self) -> Result<SyncStatus> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn author(&self) -> Result<Address> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn accounts(&self) -> Result<Vec<Address>> {
        Ok(EthApiSpec::accounts(self))
    }

    fn block_number(&self) -> Result<U256> {
        Ok(U256::from(
            EthApiSpec::chain_info(self).with_message("failed to read chain info")?.best_number,
        ))
    }

    async fn chain_id(&self) -> Result<Option<U64>> {
        Ok(Some(EthApiSpec::chain_id(self)))
    }

    async fn block_by_hash(&self, _hash: H256, _full: bool) -> Result<Option<RichBlock>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn block_by_number(
        &self,
        _number: BlockNumber,
        _full: bool,
    ) -> Result<Option<RichBlock>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn block_transaction_count_by_hash(&self, _hash: H256) -> Result<Option<U256>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn block_transaction_count_by_number(
        &self,
        _number: BlockNumber,
    ) -> Result<Option<U256>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn block_uncles_count_by_hash(&self, _hash: H256) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn block_uncles_count_by_number(&self, _number: BlockNumber) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn uncle_by_block_hash_and_index(
        &self,
        _hash: H256,
        _index: Index,
    ) -> Result<Option<RichBlock>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn uncle_by_block_number_and_index(
        &self,
        _number: BlockNumber,
        _index: Index,
    ) -> Result<Option<RichBlock>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn transaction_by_hash(
        &self,
        _hash: H256,
    ) -> Result<Option<reth_rpc_types::Transaction>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn transaction_by_block_hash_and_index(
        &self,
        _hash: H256,
        _index: Index,
    ) -> Result<Option<reth_rpc_types::Transaction>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn transaction_by_block_number_and_index(
        &self,
        _number: BlockNumber,
        _index: Index,
    ) -> Result<Option<reth_rpc_types::Transaction>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn transaction_receipt(&self, _hash: H256) -> Result<Option<TransactionReceipt>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn balance(&self, _address: Address, _block_number: Option<BlockId>) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn storage_at(
        &self,
        _address: Address,
        _index: U256,
        _block_number: Option<BlockId>,
    ) -> Result<H256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn transaction_count(
        &self,
        _address: Address,
        _block_number: Option<BlockId>,
    ) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn get_code(&self, address: Address, block_number: Option<BlockId>) -> Result<Bytes> {
        Ok(EthApi::get_code(self, address, block_number)?)
    }

    async fn call(&self, _request: CallRequest, _block_number: Option<BlockId>) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn create_access_list(
        &self,
        _request: CallRequest,
        _block_number: Option<BlockId>,
    ) -> Result<AccessListWithGasUsed> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn estimate_gas(
        &self,
        _request: CallRequest,
        _block_number: Option<BlockId>,
    ) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn gas_price(&self) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn fee_history(
        &self,
        _block_count: U256,
        _newest_block: BlockNumber,
        _reward_percentiles: Option<Vec<f64>>,
    ) -> Result<FeeHistory> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn max_priority_fee_per_gas(&self) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn is_mining(&self) -> Result<bool> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn hashrate(&self) -> Result<U256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn get_work(&self) -> Result<Work> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn submit_hashrate(&self, _hashrate: U256, _id: H256) -> Result<bool> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn submit_work(&self, _nonce: H64, _pow_hash: H256, _mix_digest: H256) -> Result<bool> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn send_transaction(&self, _request: TransactionRequest) -> Result<H256> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn send_raw_transaction(&self, tx: Bytes) -> Result<H256> {
        Ok(EthApi::send_raw_transaction(self, tx).await?)
    }

    async fn sign(&self, _address: Address, _message: Bytes) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn sign_transaction(&self, _transaction: CallRequest) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn sign_typed_data(&self, _address: Address, _data: Value) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn get_proof(
        &self,
        _address: Address,
        _keys: Vec<H256>,
        _block_number: Option<BlockId>,
    ) -> Result<EIP1186AccountProofResponse> {
        Err(internal_rpc_err("unimplemented"))
    }
}
