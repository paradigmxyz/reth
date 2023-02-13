//! Implementation of the [`jsonrpsee`] generated [`reth_rpc_api::EthApiServer`] trait
//! Handles RPC requests for the `eth_` namespace.

use crate::{
    eth::api::EthApi,
    result::{internal_rpc_err, ToRpcResult},
};
use jsonrpsee::core::RpcResult as Result;
use reth_primitives::{
    rpc::{transaction::eip2930::AccessListWithGasUsed, BlockId},
    Address, BlockHashOrNumber, BlockNumber, Bytes, Header, H256, H64, U256, U64,
};
use reth_provider::{BlockProvider, HeaderProvider, StateProviderFactory};
use reth_rpc_api::EthApiServer;
use reth_rpc_types::{
    CallRequest, EIP1186AccountProofResponse, FeeHistory, FeeHistoryCacheItem, Index, RichBlock,
    SyncStatus, TransactionReceipt, TransactionRequest, Work,
};
use reth_transaction_pool::TransactionPool;
use serde_json::Value;
use std::collections::BTreeMap;

use super::EthApiSpec;

#[async_trait::async_trait]
impl<Client, Pool, Network> EthApiServer for EthApi<Client, Pool, Network>
where
    Self: EthApiSpec,
    Pool: TransactionPool + 'static,
    Client: BlockProvider + HeaderProvider + StateProviderFactory + 'static,
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
        block_count: u64,
        newest_block: BlockNumber,
        _reward_percentiles: Option<Vec<f64>>,
    ) -> Result<FeeHistory> {
        if block_count == 0 {
            return Ok(FeeHistory::default())
        }

        let start_block = newest_block - block_count;
        let end_block = newest_block;

        let mut fee_history_cache = self.fee_history_cache.lock();

        let mut fee_history_cache_items = BTreeMap::new();
        let mut first_non_cached_block = None;
        for block in start_block..=end_block {
            // Check if block exists in cache, and move it to the head of the list if so
            if let Some(fee_history_cache_item) = fee_history_cache.get(&block) {
                fee_history_cache_items.insert(block, fee_history_cache_item.clone());
            } else {
                // If block doesn't exist in cache, set it as a first non-cached block to query it
                // from the database
                first_non_cached_block.get_or_insert(block);
            }
        }

        // If we had any cache misses, query the database starting with the first non-cached block
        if let Some(start_block) = first_non_cached_block {
            let headers: Vec<Header> = self
                .inner
                .client
                .headers_range(BlockHashOrNumber::Number(start_block)..(end_block + 1).into())
                .to_rpc_result()?;
            if headers.is_empty() {
                return Ok(FeeHistory::default())
            }

            for header in headers {
                let base_fee_per_gas = header.base_fee_per_gas.
                        unwrap_or_default(). // Zero for pre-EIP-1559 blocks
                        try_into().unwrap(); // u64 -> U256 won't fail
                let gas_used_ratio = header.gas_used as f64 / header.gas_limit as f64;

                let fee_history_cache_item = FeeHistoryCacheItem {
                    base_fee_per_gas,
                    gas_used_ratio,
                    reward: None, // TODO: calculate rewards per transaction
                };

                fee_history_cache_items.insert(header.number, fee_history_cache_item.clone());
                fee_history_cache.push(header.number, fee_history_cache_item);
            }
        }

        Ok(FeeHistory {
            base_fee_per_gas: fee_history_cache_items
                .values()
                .map(|item| item.base_fee_per_gas)
                .collect(),
            gas_used_ratio: fee_history_cache_items
                .values()
                .map(|item| item.gas_used_ratio)
                .collect(),
            oldest_block: U256::from_be_bytes(
                self.inner
                    .client
                    .block_hash((newest_block - block_count).try_into().unwrap())
                    .to_rpc_result()?
                    .unwrap()
                    .0,
            ),
            reward: None,
        })
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
